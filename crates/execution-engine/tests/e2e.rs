//! Testes E2E do `RunExecutor` (Fase 3, Etapa 4).
//!
//! Estes testes usam um [`ScriptedAdapter`] local — um `ProviderAdapter`
//! fake que devolve um script de eventos por chamada de `stream()`.
//! Diferente do `FakeProviderAdapter` do `provider-engine::fake` (que
//! sempre devolve o mesmo script), o `ScriptedAdapter` permite
//! programar **múltiplos rounds** com eventos diferentes, que é
//! exatamente o que o `RunExecutor` precisa: round 1 = `Delta +
//! ToolCall + Done{ToolCalls}`, round 2 = `Delta + Done{Stop}`.
//!
//! O teste é o "Fluxo vertical 1" do `PROMPT MESTRE` §33: o modelo
//! emite um `tool_call` pra `files.read`, o executor valida e
//! executa, devolve o resultado via `ToolResult`, e o modelo encerra
//! a resposta.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use frederico_agent_engine::Budget;
use frederico_core::{MessageId, ModelId, ProviderId, RunId, ToolId};
use frederico_execution_engine::{RunExecutor, RunOutcome};
use frederico_provider_engine::provider::{AdapterCapabilities, CostModel, RunHandle};
use frederico_provider_engine::{
    ChatMessage, ChatRequest, ChatResponse, ProviderAdapter, ProviderError, Role, StopReason,
    StreamEvent,
};
use frederico_storage::{
    ConversationRepo, Database, MessageEventRepo, MessageRepo, MessageStatus, RunRepo, RunStatus,
};
use frederico_tool_registry::{
    FilesReadTool, Jail, PermissionSet, Tool, ToolCategory, ToolManifest, ToolManifestBuilder,
    ToolRegistry,
};

use futures::stream::{self, BoxStream, StreamExt};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// ScriptedAdapter — fake local com fila de scripts
// ---------------------------------------------------------------------------

/// Adapter que devolve um script de eventos por chamada de `stream()`.
struct ScriptedAdapter {
    id: ProviderId,
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    call_count: Mutex<u32>,
}

impl ScriptedAdapter {
    fn new(id: impl Into<ProviderId>, scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            id: id.into(),
            scripts: Mutex::new(scripts.into_iter().collect()),
            call_count: Mutex::new(0),
        }
    }

    fn call_count(&self) -> u32 {
        *self.call_count.lock().unwrap()
    }
}

#[async_trait]
impl ProviderAdapter for ScriptedAdapter {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_stream: true,
            supports_tools: true,
            supports_usage_in_stream: false,
        }
    }

    fn cost_model(&self) -> CostModel {
        CostModel::default()
    }

    async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        unimplemented!("ScriptedAdapter só faz stream")
    }

    fn stream(
        &self,
        _request: ChatRequest,
    ) -> Result<BoxStream<'static, StreamEvent>, ProviderError> {
        *self.call_count.lock().unwrap() += 1;
        let next = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ProviderError::network("ScriptedAdapter: sem mais scripts"))?;
        Ok(Box::pin(stream::iter(next)))
    }

    fn cancel(&self, _handle: RunHandle) -> Result<(), ProviderError> {
        Ok(())
    }

    fn known_models(&self) -> Vec<(ModelId, &'static str)> {
        vec![(ModelId::new("fake-model-v1"), "Fake Model v1")]
    }
}

// ---------------------------------------------------------------------------
// Helpers de setup
// ---------------------------------------------------------------------------

struct Tempdir(PathBuf);

/// Contador atômico para garantir nomes únicos mesmo entre testes que
/// rodem no mesmo nanosegundo (testes paralelos em `cargo test`).
static TEMPDIR_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Tempdir {
    fn new(label: &str) -> Self {
        let base = std::env::temp_dir();
        let counter = TEMPDIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let unique = format!(
            "frederico-execution-engine-e2e-{label}-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            counter,
        );
        let dir = base.join(&unique);
        // `create_dir` (não `create_dir_all`) — falha rápido se já
        // existe (defesa contra race conditions).
        if let Err(e) = std::fs::create_dir(&dir) {
            // Se for AlreadyExists, tenta com outro counter (caminho
            // improvável mas defensivo).
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                // O retry precisa ficar sob `base` como o caminho normal:
                // `create_dir(&unique2)` criaria o diretório relativo ao
                // cwd do processo de teste e devolveria um caminho relativo.
                let dir2 = base.join(format!("{unique}-retry"));
                std::fs::create_dir(&dir2).expect("criou retry");
                return Self(dir2);
            }
            panic!("criou tempdir: {e}");
        }
        Self(dir)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Tempdir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    _workspace: Tempdir,
    db_path: PathBuf,
}

async fn setup_workspace() -> Fixture {
    let workspace = Tempdir::new("ws");
    // Cria um arquivo pra files.read.
    std::fs::write(workspace.path().join("hello.txt"), "Hello from fixture!").unwrap();
    std::fs::create_dir(workspace.path().join("sub")).unwrap();
    std::fs::write(workspace.path().join("sub/inner.txt"), "Inner file content").unwrap();
    let db_path = workspace.path().join("test.db");
    Fixture {
        _workspace: workspace,
        db_path,
    }
}

fn make_files_read_manifest() -> ToolManifest {
    ToolManifestBuilder::new(ToolId::new("files.read"), "files")
        .version("0.1.0")
        .display_name("Ler arquivo")
        .description("Lê o conteúdo de um arquivo dentro do workspace.")
        .category(ToolCategory::Files)
        .requires_file_read(true)
        .input_schema(frederico_tool_registry::JsonSchema(serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "max_bytes": {"type": "integer", "minimum": 1, "maximum": 52428800}
            },
            "required": ["path"],
            "additionalProperties": false
        })))
        .output_schema(frederico_tool_registry::JsonSchema(serde_json::json!({
            "type": "object",
            "properties": {
                "content": {"type": "string"},
                "truncated": {"type": "boolean"},
                "bytes_read": {"type": "integer"}
            },
            "required": ["content", "truncated", "bytes_read"]
        })))
        .build()
        .expect("manifesto bem-formado")
}

async fn setup_executor() -> (
    Fixture,
    Database,
    ToolRegistry,
    Arc<dyn frederico_tool_registry::JailResolver>,
    MessageId,
    RunId,
) {
    let fx = setup_workspace().await;
    let db = Database::open(&fx.db_path).await.expect("abre db");
    let mut registry = ToolRegistry::new();
    registry.register(make_files_read_manifest());
    let jail = Jail::new(fx._workspace.path()).expect("jail");
    let jail_resolver = frederico_tool_registry::static_jail_resolver(jail);
    let conv = ConversationRepo::new(&db)
        .create(
            &ProviderId::new("simulated"),
            &ModelId::new("fake-model-v1"),
            None,
        )
        .await
        .unwrap();
    let asst = MessageRepo::new(&db)
        .create(&conv.id, "assistant", "", None)
        .await
        .unwrap();
    let run_repo = RunRepo::new(&db);
    let run = run_repo.create(&conv.id, &asst.id).await.unwrap();
    // Fase 6, Etapa 2: portão único exige `CallingModel` para
    // o `Delta` virar `Streaming`. Em produção, a transição
    // `Created → ... → CallingModel` é feita pelo orquestrador
    // antes de chamar o executor (Etapa 4 da Fase 3). Aqui no
    // teste, simulamos manualmente.
    run_repo
        .set_state_unchecked(&run.id, frederico_agent_engine::RunState::CallingModel)
        .await
        .unwrap();
    (fx, db, registry, jail_resolver, asst.id, run.id)
}

// ---------------------------------------------------------------------------
// Testes
// ---------------------------------------------------------------------------

/// "Fluxo vertical 1" do `PROMPT MESTRE` §33: o executor consome o
/// stream, valida o `ToolCall` (`files.read`), executa a ferramenta
/// (lê `hello.txt`), emite o `ToolResult` (com `ok: true` e o
/// conteúdo), adiciona o resultado ao contexto, e chama o modelo de
/// novo. No segundo round, o modelo termina.
///
/// **Nota sobre permissões** (spec
/// `tool-registry-specification.md` §7.7): o Passo 5 (permissões)
/// roda antes do Passo 7 (jail). O `PermissionSet` precisa ter
/// `file_read: WorkspaceOnly` senão o `files.read` é rejeitado com
/// `TOOL_PERMISSION_DENIED`. A Etapa 6 carrega o `PermissionSet`
/// real; aqui usamos o mínimo que deixa o `files.read` passar.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn executor_closes_tool_call_loop_with_files_read() {
    use frederico_tool_registry::{FileReadPermission, PermissionSet as Perms};

    let (_fx, db, registry, jail_resolver, message_id, run_id) = setup_executor().await;

    // Round 1: Delta + ToolCall + Done{ToolCalls}
    // Round 2: Delta + Done{Stop}
    let adapter = Arc::new(ScriptedAdapter::new(
        "simulated",
        vec![
            vec![
                StreamEvent::Delta {
                    content: "olá ".to_string(),
                },
                StreamEvent::ToolCall {
                    id: "call-1".to_string(),
                    name: "files.read".to_string(),
                    arguments_json: r#"{"path": "hello.txt"}"#.to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::ToolCalls,
                },
            ],
            vec![
                StreamEvent::Delta {
                    content: "mundo".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ],
        ],
    ));

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FilesReadTool::new())];

    let permissions = Perms {
        file_read: FileReadPermission::WorkspaceOnly,
        ..Default::default()
    };

    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        jail_resolver.clone(),
        db.clone(),
        permissions,
        vec![ToolId::new("files.read")],
        tools,
        Budget::default(),
        CancellationToken::new(),
    );

    let initial = vec![ChatMessage::user("lê o hello.txt")];
    let outcome: RunOutcome = executor
        .run(message_id, run_id, ModelId::new("fake-model-v1"), initial)
        .await
        .expect("executa");

    // === Asserções sobre o RunOutcome ===
    assert_eq!(outcome.accumulated_content, "olá mundo");
    assert_eq!(outcome.tool_calls.len(), 1);
    assert_eq!(outcome.tool_calls[0].id, "call-1");
    assert_eq!(outcome.tool_calls[0].tool_id, ToolId::new("files.read"));
    assert!(outcome.tool_calls[0].ok, "files.read deveria ter sucesso");
    let content = outcome.tool_calls[0]
        .output
        .get("content")
        .and_then(|v| v.as_str())
        .expect("content é string");
    assert_eq!(content, "Hello from fixture!");
    assert_eq!(outcome.stop_reason, StopReason::Stop);
    assert_eq!(
        outcome.final_state,
        frederico_agent_engine::RunState::Completed
    );

    // === Asserções sobre o adapter ===
    assert_eq!(
        adapter.call_count(),
        2,
        "duas chamadas de stream (2 rounds)"
    );

    // === Asserções sobre o DB ===
    // 6 eventos: delta, tool_call, done, tool_result, delta, done
    let events = MessageEventRepo::new(&db)
        .list_for_message(&message_id, 0)
        .await
        .unwrap();
    assert_eq!(events.len(), 6, "6 eventos no journal");
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "delta", // round 1
            "tool_call",
            "done",
            "tool_result", // resposta da files.read
            "delta",       // round 2
            "done",
        ]
    );

    // tool_result tem `ok: true` e o conteúdo lido.
    let tool_result_event = &events[3];
    assert_eq!(tool_result_event.kind, "tool_result");
    assert_eq!(
        tool_result_event.data.get("ok"),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        tool_result_event.data.get("id"),
        Some(&serde_json::json!("call-1"))
    );

    // Mensagem e Run fechados como Completed.
    let msg = MessageRepo::new(&db).get(&message_id).await.unwrap();
    assert_eq!(msg.status, MessageStatus::Completed.as_str());
    assert_eq!(msg.content, "olá mundo");
    let run = RunRepo::new(&db).get(&run_id).await.unwrap();
    assert_eq!(run.status, RunStatus::Completed.as_str());
}

/// Tool_call rejeitado pelo validador (Passo 4: `files.read` não
/// está no `allowed_for_run` desta execução) emite `ToolResult` com
/// `ok: false` e `TOOL_NOT_IN_INVENTORY` — o modelo recebe o erro.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn executor_emits_tool_result_with_error_when_rejected() {
    let (_fx, db, registry, jail_resolver, message_id, run_id) = setup_executor().await;

    // Uma única chamada: ToolCall + Done{ToolCalls}. O executor
    // valida, rejeita (Passo 4), emite ToolResult, mas como o
    // adapter não tem mais scripts, o próximo `stream()` falha.
    let adapter = Arc::new(ScriptedAdapter::new(
        "simulated",
        vec![
            vec![StreamEvent::ToolCall {
                id: "call-bad".to_string(),
                name: "files.read".to_string(),
                arguments_json: r#"{"path": "hello.txt"}"#.to_string(),
            }],
            // O executor não chega aqui: o validador rejeita e a
            // chamada `stream()` subsequente falha. Mas a Etapa 4
            // espera 1 round só nesse caso — então este segundo
            // script nunca é consumido. Em vez disso, vamos
            // configurar 1 script e verificar que o executor
            // ERROU (porque o `Done` com ToolCalls exige um 2º
            // round). Hmm — o test atual abaixo cobre o caso
            // "approval_required" de outra forma. Vou simplificar
            // e usar o `allowed_for_run` vazio.
        ],
    ));

    // allowed_for_run vazio → Passo 4 rejeita com
    // `TOOL_NOT_IN_INVENTORY`.
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FilesReadTool::new())];

    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        jail_resolver.clone(),
        db.clone(),
        PermissionSet::default(),
        vec![], // <-- allowed_for_run vazio rejeita o files.read
        tools,
        Budget::default(),
        CancellationToken::new(),
    );

    let initial = vec![ChatMessage::user("teste")];
    // O executor vai emitir `ToolResult{ok:false, error_code: TOOL_NOT_IN_INVENTORY}`
    // e voltar a chamar o modelo. Como o adapter não tem mais
    // scripts, o segundo `stream()` retorna `ProviderError`.
    let result = executor
        .run(message_id, run_id, ModelId::new("fake-model-v1"), initial)
        .await;
    assert!(result.is_err(), "esperava erro (sem mais scripts)");

    // Mas o ToolResult de erro deve ter sido persistido.
    let events = MessageEventRepo::new(&db)
        .list_for_message(&message_id, 0)
        .await
        .unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    // Esperado: tool_call, tool_result (erro), e o erro do provider
    // não é persistido (a falha na 2ª chamada aborta antes de qualquer
    // `Error` virar evento).
    assert!(
        kinds.contains(&"tool_call"),
        "tool_call foi persistido: {kinds:?}"
    );
    assert!(
        kinds.contains(&"tool_result"),
        "tool_result (rejeitado) foi persistido: {kinds:?}"
    );
    let tool_result = events
        .iter()
        .find(|e| e.kind == "tool_result")
        .expect("tool_result presente");
    assert_eq!(tool_result.data.get("ok"), Some(&serde_json::json!(false)));
    assert_eq!(
        tool_result
            .data
            .get("output")
            .and_then(|o| o.get("error_code")),
        Some(&serde_json::json!("TOOL_NOT_IN_INVENTORY"))
    );
}

/// Caso simples: provider emite só `Delta + Done{Stop}` (sem
/// `tool_call`). O executor finaliza como `Completed` e
/// `MessageStatus::Completed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn executor_completes_when_no_tool_call() {
    let (_fx, db, registry, jail_resolver, message_id, run_id) = setup_executor().await;

    let adapter = Arc::new(ScriptedAdapter::new(
        "simulated",
        vec![vec![
            StreamEvent::Delta {
                content: "olá".to_string(),
            },
            StreamEvent::Done {
                stop_reason: StopReason::Stop,
            },
        ]],
    ));

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FilesReadTool::new())];

    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        jail_resolver.clone(),
        db.clone(),
        PermissionSet::default(),
        vec![ToolId::new("files.read")],
        tools,
        Budget::default(),
        CancellationToken::new(),
    );

    let outcome = executor
        .run(
            message_id,
            run_id,
            ModelId::new("fake-model-v1"),
            vec![ChatMessage::user("oi")],
        )
        .await
        .expect("executa");

    assert_eq!(outcome.accumulated_content, "olá");
    assert_eq!(outcome.tool_calls.len(), 0);
    assert_eq!(outcome.stop_reason, StopReason::Stop);
    assert_eq!(
        outcome.final_state,
        frederico_agent_engine::RunState::Completed
    );

    // 2 eventos: delta + done.
    let events = MessageEventRepo::new(&db)
        .list_for_message(&message_id, 0)
        .await
        .unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, vec!["delta", "done"]);

    let msg = MessageRepo::new(&db).get(&message_id).await.unwrap();
    assert_eq!(msg.status, MessageStatus::Completed.as_str());
}

/// `Error` do provider fecha o `Run` como `Failed` e persiste o erro
/// na mensagem.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn executor_fails_on_provider_error() {
    let (_fx, db, registry, jail_resolver, message_id, run_id) = setup_executor().await;

    let adapter = Arc::new(ScriptedAdapter::new(
        "simulated",
        vec![vec![StreamEvent::Error(ProviderError::auth())]],
    ));

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FilesReadTool::new())];

    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        jail_resolver.clone(),
        db.clone(),
        PermissionSet::default(),
        vec![ToolId::new("files.read")],
        tools,
        Budget::default(),
        CancellationToken::new(),
    );

    let outcome = executor
        .run(
            message_id,
            run_id,
            ModelId::new("fake-model-v1"),
            vec![ChatMessage::user("oi")],
        )
        .await
        .expect("executa (sem panic, retorna RunOutcome com Failed)");

    assert_eq!(
        outcome.final_state,
        frederico_agent_engine::RunState::Failed
    );

    let msg = MessageRepo::new(&db).get(&message_id).await.unwrap();
    assert_eq!(msg.status, MessageStatus::Failed.as_str());
    let run = RunRepo::new(&db).get(&run_id).await.unwrap();
    assert_eq!(run.status, RunStatus::Failed.as_str());
}

/// `files.read` com jail rejeita path traversal antes de executar
/// (`TOOL_JAIL_VIOLATION`). O `ToolResult` carrega o erro.
///
/// **Nota sobre a ordem da validação** (spec
/// `tool-registry-specification.md` §7.7): o Passo 5 (permissões)
/// roda **antes** do Passo 7 (jail). Por isso o teste passa
/// `PermissionSet { file_read: FileReadPermission::WorkspaceOnly, .. }`
/// explicitamente — caso contrário o Passo 5 rejeita com
/// `TOOL_PERMISSION_DENIED` antes do jail ser consultado.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn executor_rejects_path_traversal_in_tool_call() {
    use frederico_tool_registry::{FileReadPermission, PermissionSet as Perms};

    let (_fx, db, registry, jail_resolver, message_id, run_id) = setup_executor().await;

    let adapter = Arc::new(ScriptedAdapter::new(
        "simulated",
        vec![
            vec![
                StreamEvent::ToolCall {
                    id: "call-evil".to_string(),
                    name: "files.read".to_string(),
                    arguments_json: r#"{"path": "../etc/passwd"}"#.to_string(),
                },
                // O executor valida (rejeita com TOOL_JAIL_VIOLATION
                // no Passo 7), emite ToolResult, volta a chamar.
                // O 2º round devolve Done{Stop} pra fechar.
            ],
            vec![StreamEvent::Done {
                stop_reason: StopReason::Stop,
            }],
        ],
    ));

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FilesReadTool::new())];

    let permissions = Perms {
        file_read: FileReadPermission::WorkspaceOnly,
        ..Default::default()
    };

    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        jail_resolver.clone(),
        db.clone(),
        permissions,
        vec![ToolId::new("files.read")],
        tools,
        Budget::default(),
        CancellationToken::new(),
    );

    let result = executor
        .run(
            message_id,
            run_id,
            ModelId::new("fake-model-v1"),
            vec![ChatMessage::user("lê /etc/passwd")],
        )
        .await;
    let outcome = result.expect("executa");
    assert_eq!(
        outcome.final_state,
        frederico_agent_engine::RunState::Completed
    );

    let events = MessageEventRepo::new(&db)
        .list_for_message(&message_id, 0)
        .await
        .unwrap();
    let tool_result = events
        .iter()
        .find(|e| e.kind == "tool_result")
        .expect("tool_result presente");
    assert_eq!(tool_result.data.get("ok"), Some(&serde_json::json!(false)));
    assert_eq!(
        tool_result
            .data
            .get("output")
            .and_then(|o| o.get("error_code")),
        Some(&serde_json::json!("TOOL_JAIL_VIOLATION"))
    );
    // O tool_call foi logado no RunOutcome.
    assert_eq!(outcome.tool_calls.len(), 1);
    assert!(!outcome.tool_calls[0].ok);
}

// ---------------------------------------------------------------------------
// Sanidade do adapter (não E2E, mas útil pro setup)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scripted_adapter_returns_one_script_per_call() {
    let adapter = ScriptedAdapter::new(
        "simulated",
        vec![
            vec![StreamEvent::Delta {
                content: "a".to_string(),
            }],
            vec![StreamEvent::Delta {
                content: "b".to_string(),
            }],
        ],
    );
    let req = ChatRequest::new(
        ProviderId::new("simulated"),
        ModelId::new("fake-model-v1"),
        vec![ChatMessage::user("oi")],
    );
    let s1 = adapter.stream(req.clone()).unwrap();
    let e1: Vec<StreamEvent> = s1.collect().await;
    assert_eq!(e1.len(), 1);
    let s2 = adapter.stream(req).unwrap();
    let e2: Vec<StreamEvent> = s2.collect().await;
    assert_eq!(e2.len(), 1);
    assert_eq!(adapter.call_count(), 2);
}

#[allow(dead_code)]
fn _silence_unused_for_clarity() {
    // Mantém imports de tipos que aparecem em `match` patterns e
    // não são usados em nenhum lugar se a feature for desabilitada.
    let _ = (
        std::marker::PhantomData::<Role>,
        std::marker::PhantomData::<Duration>,
    );
}

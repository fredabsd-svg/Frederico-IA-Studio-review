//! `WorkerInvoker` — contrato genérico de invocação de worker
//! sidecar. ADR-0024 (Etapa 2.B da Fase de Ligação).
//!
//! ## Por que existe
//!
//! A Etapa 2.A da fase-ligação introduziu o
//! [`frederico_app::launcher::DocumentWorkerLauncher`] (ADR-0023):
//! o owner do ciclo de vida do `document-worker` Python, com
//! lazy start + restart on death com teto + kill tree no app
//! exit. **O launcher é lazy**: não há `WorkerHandle`
//! concreto até a primeira `invoke`.
//!
//! Os 3 kits do `frederico-document-kits` (WordPro, ExcelPro,
//! PdfPro) e o `WorkerToolDispatcher` do `frederico-tool-registry`
//! recebem `Arc<WorkerHandle>` no construtor (Fase 5 fechada,
//! Etapa 3). Pra integrar o launcher no caminho do modelo, é
//! preciso um **contrato abstrato** que tanto o `WorkerHandle`
//! (Fase 5) quanto o `DocumentWorkerLauncher` (Etapa 2.A)
//! implementem.
//!
//! ## Onde mora
//!
//! **No `frederico-core`, não no `frederico-tool-registry`.** O
//! `WorkerInvoker` é um contrato genérico de invocação de
//! worker — não tem nada a ver com registro de ferramentas.
//! Colocá-lo no `tool-registry` criaria uma dependência
//! "errada" do `document-kits` em `tool-registry` (que já
//! existe por outras razões, mas o ponto do user foi manter
//! o grafo limpo: contratos neutros no `core`, implementações
//! específicas nos crates de feature). O `core` é o lugar dos
//! tipos compartilhados (`ToolId`, `ConversationId`, `MessageId`,
//! `RunId`).
//!
//! ## O erro
//!
//! O trait **não retorna `ProcessError`** (tipo do
//! `frederico-process-architecture`) — senão o `core` passaria
//! a depender do `process-architecture` (que é a direção que
//! o ADR-0023 disse pra evitar). O `InvokeError` é definido
//! aqui, com as 6 categorias que o `ProcessError` cobre mais
//! `PermanentlyDead` (específico do launcher). As
//! implementações convertem de `ProcessError` (1:1) e de
//! `WorkerError` (mapeamento documentado no
//! `app/src/launcher.rs::WorkerInvoker` impl).

use async_trait::async_trait;
use serde_json::Value;

/// Trait genérico de invocação de worker sidecar. Uma
/// implementação é um "invoker": recebe um `payload` JSON,
/// despacha pra um worker (real ou lazy), e devolve o
/// resultado JSON ou um [`InvokeError`].
///
/// **Send + Sync + 'static** são obrigatórios porque o
/// `RunExecutor` (que chama `Tool::execute` que chama
/// `invoker.invoke`) carrega o invoker por longos períodos em
/// `Arc<dyn WorkerInvoker>`.
#[async_trait]
pub trait WorkerInvoker: Send + Sync + 'static {
    /// Executa uma `tool.invoke` e devolve o payload de
    /// resposta (ou erro).
    ///
    /// # Erros
    /// - [`InvokeError::Protocol`] se a response for
    ///   malformada (JSON inválido, opcode desconhecido,
    ///   request_id duplicado).
    /// - [`InvokeError::Transport`] se o canal pro worker
    ///   caiu (worker já terminou — provavelmente `shutdown`
    ///   foi chamado).
    /// - [`InvokeError::Timeout`] se o worker não respondeu
    ///   dentro do `timeout` (default 30s).
    /// - [`InvokeError::Unhealthy`] se o worker está morto
    ///   (saúde degradada, ou launcher excedeu o teto de
    ///   restarts).
    /// - [`InvokeError::Platform`] se o executável está
    ///   faltando, sem permissão, OS incompatível, ou
    ///   plataforma não suportada.
    /// - [`InvokeError::PermanentlyDead`] se o launcher já
    ///   excedeu o teto de tentativas de restart e está em
    ///   estado `Dead` permanente (a UI precisa chamar
    ///   `reset` antes).
    async fn invoke(&self, payload: Value) -> Result<Value, InvokeError>;
}

/// Erro genérico de invocação de worker. Categorias 1:1 com o
/// `ProcessError` do `process-architecture` (que é o tipo
/// "real" retornado pelo `WorkerHandle::invoke`), mais a
/// variante `PermanentlyDead` (específica do launcher).
///
/// **Por que o `core` define o seu próprio erro:** o `core`
/// não pode depender de `process-architecture` (regra de
/// pureza: `core` importa só `serde`, `uuid`, `thiserror`,
/// `chrono`, `async-trait`). O `ProcessError` vive no
/// `process-architecture` e referencia o
/// `WorkerHealthSnapshot` que mora lá. **Definir o erro aqui**
/// é o que mantém o `core` puro e o grafo de dependências
/// limpo.
#[derive(Debug, thiserror::Error)]
pub enum InvokeError {
    /// Erro de protocolo IPC — JSON malformado, opcode
    /// desconhecido, manifesto inválido, request_id
    /// duplicado.
    #[error("erro de protocolo: {message}")]
    Protocol {
        /// Mensagem curta do que falhou.
        message: String,
    },

    /// Erro de transporte (pipe / conexão).
    #[error("erro de transporte: {message}")]
    Transport {
        /// Mensagem curta do que falhou.
        message: String,
    },

    /// Worker demorou mais que o `timeout_ms` declarado.
    #[error("invoke excedeu o timeout")]
    Timeout,

    /// Worker foi morto pelo watchdog, ou saúde degradada,
    /// ou launcher excedeu o teto de restarts.
    #[error("worker não saudável: {message}")]
    Unhealthy {
        /// Mensagem curta do motivo.
        message: String,
    },

    /// Plataforma — executável faltando, sem permissão, OS
    /// incompatível, ou plataforma não suportada
    /// (ex.: `WorkerError::PlatformNotSupported`).
    #[error("erro de plataforma: {message}")]
    Platform {
        /// Mensagem curta do que falhou.
        message: String,
    },

    /// Específica do `DocumentWorkerLauncher` (não existe no
    /// `ProcessError`): o launcher já excedeu o teto de
    /// tentativas de restart e está em estado `Dead`
    /// permanente. A UI precisa chamar `reset()` antes de
    /// tentar de novo. Mapeia pro `ProcessError::Unhealthy`
    /// se o caller não conhece o launcher.
    #[error("worker permanentemente morto após {attempts} tentativas (limite: {max})")]
    PermanentlyDead {
        /// Número de tentativas que falharam.
        attempts: u8,
        /// Teto configurado no launcher.
        max: u8,
    },
}

impl InvokeError {
    /// Helper pra converter [`InvokeError`] em
    /// `ProcessError` (1:1, exceto `PermanentlyDead` que
    /// vira `Unhealthy`). Usado pelos adapters do
    /// `process-architecture` que precisam expor o erro do
    /// `WorkerHandle` na interface `WorkerInvoker`.
    ///
    /// **Não pode** ficar aqui porque dependeria de
    /// `process-architecture` (vai contra a regra de
    /// pureza do `core`). O adapter no `tool-registry` ou no
    /// `process-architecture` implementa esse `From` lá.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

/// Resultado padrão de uma invocação.
pub type InvokeResult<T> = Result<T, InvokeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoke_error_message_cites_kind() {
        let proto = InvokeError::Protocol {
            message: "JSON malformado".to_string(),
        };
        assert!(proto.to_string().contains("erro de protocolo"));
        assert!(proto.to_string().contains("JSON malformado"));

        let plat = InvokeError::Platform {
            message: "python.exe faltando".to_string(),
        };
        assert!(plat.to_string().contains("erro de plataforma"));
        assert!(plat.to_string().contains("python.exe faltando"));

        let perm = InvokeError::PermanentlyDead {
            attempts: 3,
            max: 3,
        };
        assert!(perm.to_string().contains("permanentemente morto"));
        assert!(perm.to_string().contains("3 tentativas"));
    }

    #[test]
    fn invoke_error_message_helper_returns_to_string() {
        let err = InvokeError::Timeout;
        assert_eq!(err.message(), "invoke excedeu o timeout");
    }
}

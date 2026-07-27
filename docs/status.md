# Estado Real por Fase

Primeiro arquivo a ser lido por qualquer sessão nova de IA, **depois de** `REGRAS-DO-PROJETO.md`.

## Estados possíveis

- `não iniciada` — fase planejada, nenhum trabalho começou.
- `em andamento` — código está sendo escrito; testes da fase ainda não todos verdes.
- `concluída` — todos os testes da fase passam; specs correspondentes promovidos a `parcialmente implementado` ou `implementado`; changelog atualizado.
- `bloqueada` — algo impede progredir; motivo documentado na coluna "Pendências".

## Regra de promoção

Promover uma fase de `em andamento` para `concluída` exige, simultaneamente:

1. Suíte de testes da fase 100% verde.
2. Specs correspondentes com `Estado` atualizado para `parcialmente implementado` ou `implementado`, com carimbo de verificação recente.
3. Entrada em `CHANGELOG.md` descrevendo o efeito para o usuário.
4. Referência ao PR / commit que consolidou a fase.

## Tabela

| Fase | Nome | Estado | Evidência | Pendências |
|------|------|--------|-----------|------------|
| 0 | Fundação documental | concluída | este `status.md`; PR de fundação documental; 9 specs em `docs/architecture/`; 4 ADRs em `docs/decisions/`; `REGRAS-DO-PROJETO.md` com §1.13 | — |
| 1 | Fundação (Tauri + Rust + SQLite) | concluída | suíte workspace 15/15 verde (`cargo test`); `cargo clippy --workspace -- -D warnings` limpo; `npm run build` verde; `scripts/check-core-purity.ps1` OK; `cargo tauri build` produz `target/release/bundle/nsis/Frederico IA Studio_0.1.0_x64-setup.exe` (3,0 MB) | — |
| 2 | Chat e provedores | concluída | Etapas 1, 2, Leva 3 e Etapa 5 (UI) fechadas; **Hardening 1 (DPAPI real) fechado**: `WindowsCredentialStore` implementado via `CredWriteW`/`CredReadW`/`CredDeleteW`/`CredEnumerateW` (crate `windows` v0.58), `TargetName` `Frederico-IA-Studio:provider:<id>`, mapeamento correto de HRESULT→win32 (`& 0xFFFF`), `CredFree` chamado uma única vez por alocação (evitando double-free); 6 testes de integração (`tests/windows_credential_store.rs`) com mutex global de serialização e IDs únicos por run: set/get roundtrip, get-nonexistent→None, delete idempotente, list filtra prefixo Frederico, list filtra prefixo + valida que credencial de outro app não vaza, overwrite de credencial existente. Casca Tauri: `AppState.credentials: Arc<WindowsCredentialStore>` único no processo; `ProviderSetCredential`/`ProviderDeleteCredential` agora chamam DPAPI real (não mais `FakeCredentialStore` ad-hoc). `Cargo.toml` de `frederico-security`: `unsafe_code = "deny"` (era `forbid`); `#![allow(unsafe_code)]` apenas no módulo `windows.rs` e nos tests. `scripts/check-core-purity.ps1` reconhece `crates/security/src/windows*` e `crates/security/tests*` como exceções legítimas. **Hardening 3 (`provider-recorder`) fechado**: novo binário `recorder` em `crates/provider-engine/src/bin/recorder.rs` que faz chamada real a um provedor OpenAI-compat (OpenAI/OpenRouter/DeepSeek/Mistral/NVIDIA NIM/Ollama/LM Studio), captura os bytes brutos do stream SSE e grava em `fixtures/<provider>/<scenario>.jsonl` com header `# recorded_at=... provider=... model=... base_url=... format=openai-compat-sse`. **Sanitização obrigatória**: módulo `frederico_provider_engine::sanitize` (regex case-insensitive bloqueando `Authorization`/`api_key`/`x-api-key`/`Bearer `/`sk-`/`sk-ant-`/`gsk_`/`or-`); o recorder chama `sanitize::check` antes do flush e, se falhar, **deleta o arquivo** e aborta com exit 1. Integration test `tests/fixtures_sanitize.rs` varre `fixtures/**/*.jsonl` no CI e quebra o build se algum estiver contaminado. Leitura da chave via `--api-key-env VAR` (default `OPENAI_API_KEY`) — nunca via `CredentialStore` (a chave do recorder é descartável, do CI/dev, não do app). **Hardening 4 (contract tests off-PR) fechado**: 3 integration tests em `crates/provider-engine/tests/openai_compat_contract.rs` (OpenAI + OpenRouter) e `crates/provider-engine/tests/anthropic_contract.rs` (Anthropic). Cada um gateado em env var (`OPENAI_API_KEY`/`OPENROUTER_API_KEY`/`ANTHROPIC_API_KEY`); se ausente, o test **pula** com mensagem clara em vez de falhar — assim a suíte roda limpa no PR (sem chave) e roda de verdade no CI noturno (com chave). Os tests validam que o adapter produz pelo menos 1 `Delta` e exatamente 1 `Done` com `StopReason` válido, contra a API real. Helper compartilhado em `tests/common/mod.rs` (`require_env`, `openai_adapter_or_skip`, `drain_events`). Runner dedicado: `scripts/run-contract-tests.ps1` (mostra quais env vars estão setadas, mascarando a chave; roda `cargo test --test openai_compat_contract --test anthropic_contract`). **Hardening 5 (recovery E2E) fechado**: 3 integration tests em `crates/provider-engine/tests/recovery.rs` que validam a regra do **"Journal de eventos"** — o SQLite é a fonte de verdade, sobrevive a drop do orquestrador. `journal_persists_events_across_orchestrator_drop` dispara run com 5 deltas + Done, espera completar, dropar orquestrador A, reabre db com orquestrador B, valida que `MessageEventRepo::list_for_message` retorna os mesmos 5 deltas + Done. `cancel_idempotent_and_status_persists` valida que `cancel_run` é idempotente e o status final (Completed/Cancelled/etc.) bate entre sink e db após recovery. `user_message_persisted_before_run_starts` valida que a `Message.role=user` está no db imediatamente após `send_message` retornar (regra do "Journal-then-emit" aplicada também à user msg). Suíte workspace **133/133 verde** (era 130/130; +3 do recovery); `cargo clippy --workspace --all-targets -- -D warnings` limpo; `check-core-purity.ps1` verde; `tsc --noEmit` + `npm run build` verdes | — |
| 3 | Motor de execução e ferramentas | não iniciada | — | depende da Fase 2 |
| 4 | Memória e continuidade | não iniciada | — | depende da Fase 3 |
| 5 | Documentos | não iniciada | — | depende da Fase 3 |
| 6 | Multimodelo e subagentes | não iniciada | — | depende da Fase 3 + 4 |
| 7 | Modo desenvolvedor | não iniciada | — | depende da Fase 3 |
| 8 | Copiloto, tarefas e refinamento | não iniciada | — | depende de 3 + 4 + 6 + 7 |
| 9 | Produção | não iniciada | — | depende de todas |

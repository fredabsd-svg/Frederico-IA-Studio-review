# `frederico-shared-contracts`

## O que faz

Define o contrato de IPC entre o núcleo em Rust e a casca Tauri (e, no futuro, um servidor). É o único lugar onde o formato das mensagens que atravessam essa fronteira está escrito: o envelope `IpcRequest` / `IpcResponse`, o enum `AppOp` com todas as operações que a UI pode pedir, e as *views* serializáveis de cada tipo de domínio.

A separação existe por causa do [ADR-0003](../decisions/0003-nucleo-desacoplado-da-casca-tauri.md): o núcleo não conhece Tauri, e a casca não conhece SQLite. Este pacote é a costura entre os dois, e não depende de nenhum dos lados.

## O que expõe

**Envelope**

- `IpcRequest { op: AppOp }` — tudo que a UI pede entra por aqui.
- `IpcResponse { ok, payload, error }` com construtores auxiliares.
- `ContractError` — erro serializável devolvido à UI.

**`AppOp`** — a superfície inteira do app, agrupada por área:

| Área | Operações |
|---|---|
| Diagnóstico | `GetAppInfo`, `Ping` |
| Provedores | `ProviderList`, `ProviderSetCredential`, `ProviderDeleteCredential` |
| Catálogo | `ModelCatalogList`, `ModelCatalogForProvider` |
| Conversas | `ConversationCreate`, `List`, `Get`, `Rename`, `SetModel`, `Delete` |
| Chat | `MessageSend`, `RunGetEvents`, `RunCancel` |
| Aprovações | `ApprovalList`, `ApprovalRespond` |
| Memória | `MemoryList`, `MemoryRetrieve`, `MemoryApplyCorrection`, `MemoryConfirmPending`, `MemoryRejectPending`, `MemoryPurgeExpired` |

**Views** — `ProviderConfigView`, `ModelDescriptorView`, `ConversationView`, `MessageView`, `MessageEventView`, `MessageSendResult`, `ApprovalEntryView`, `MemoryView`, `ScoreBreakdownView`, `MemoryHitView`, `CorrectionResultView`, `NewMemoryInputView`.

## Do que depende e quem depende dele

- **Depende de:** `serde`/`serde_json`, `frederico-core` (identificadores e `CoreError`), `secrecy` (credencial nunca trafega como `String` nua), `thiserror`. Nada de banco, nada de rede, nada de Tauri.
- **Depende dele:** só a casca `frederico-desktop` (`apps/desktop/src-tauri`). Do lado do TypeScript, os serviços em `apps/desktop/src/services/` espelham este contrato à mão.

## Decisões não óbvias e armadilhas

- **As views não são os tipos de domínio.** `Conversation`, `Message` e `MessageEvent` vivem no `frederico-storage`; aqui existem cópias achatadas e serializáveis. É duplicação deliberada: impede que uma mudança de schema do banco vaze direto para a UI. O custo é ter de atualizar os dois lados.
- **O espelho em TypeScript é manual.** Nada gera `services/*.ts` a partir daqui, então um campo adicionado no `AppOp` sem o par no TS compila de um lado e quebra no outro em runtime. Este é o candidato natural à regra "gerado vence manual" (`REGRAS §1.9`) e ainda não foi feito.
- **Credencial usa `SecretString`.** Não trocar por `String` para "facilitar" a serialização — o tipo existe para impedir que a chave apareça em log ou em `Debug`.
- **`AppOp` cresce a cada fase.** Adicionar variante é mudança de contrato: exige atualizar a casca, o serviço TS correspondente e os testes de roundtrip.

## Como testar isoladamente

```bash
cargo test -p frederico-shared-contracts
```

Os testes ficam em `#[cfg(test)] mod tests` no próprio `src/lib.rs` e são de roundtrip: serializam cada `AppOp` e cada view, desserializam de volta e comparam. É o que pega renomeação acidental de campo ou mudança de `#[serde(rename_all)]`.

## O que ele não faz

- Não executa nada: é só formato. Quem trata cada `AppOp` é a casca em `apps/desktop/src-tauri/src/main.rs`.
- Não fala com o banco, com provedor, nem com o sistema de arquivos.
- Não valida regra de negócio — validação é de quem executa a operação.
- Não gera o cliente TypeScript.
- Não versiona o contrato: hoje núcleo e casca sobem sempre juntos, no mesmo binário.

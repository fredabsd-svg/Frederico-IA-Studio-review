# 0003 — Núcleo desacoplado da casca Tauri

## Contexto

O `PROMPT MESTRE` §5.5 é explícito: "o aplicativo será entregue como desktop Windows, mas o núcleo deverá nascer desacoplado da casca Tauri, para que uma versão servidor (VPS Linux, acesso via navegador, multiusuário) possa existir no futuro sem reescrever o motor". Sem essa separação arquitetural desde a primeira versão, qualquer migração futura exigiria reescrever boa parte do código de negócio, com o risco de introduzir regressões em um produto já em produção.

## Decisão

Toda a lógica de negócio do Frederico IA Studio — motor de execução, Tool Registry, provider engine, memória, documentos, multimodelo, subagentes, persistência — vive em **crates Rust do núcleo** (diretório `crates/`) que **não podem importar** `tauri`, `tauri-runtime`, APIs específicas de Windows, caminhos absolutos do sistema, ou fazer chamadas diretas à interface.

Dependências de plataforma (acesso a credenciais, sandbox, diretórios, notificações, paths do sistema) entram via **traits** com implementação Windows injetada pela casca. A casca Tauri implementa essas traits e injeta via composição na inicialização do app.

O contrato entre casca e núcleo (requisições, eventos de execução com número sequencial, replay do `PROMPT MESTRE` §12.6) vive em um pacote compartilhado (`packages/shared-contracts/`) serializável em JSON — o mesmo contrato serve para IPC hoje e para HTTP/WebSocket amanhã.

O frontend React **nunca** chama `invoke` do Tauri espalhado pelo código. Todo acesso passa por uma camada `apps/desktop/src/services/` única, trocável por um cliente HTTP/WebSocket sem alterar componentes.

**Critério de verificação contínuo:** os testes de integração do motor rodam **sem a casca Tauri** (núcleo puro + implementações de plataforma simuladas em `tests/integration/`). Se o motor só funcionar dentro do aplicativo desktop, esta decisão foi violada.

## Alternativas descartadas

- **Tudo dentro do crate Tauri.** Descartada: bloqueia diretamente o modo servidor futuro e mistura camadas de abstração.
- **Camada de abstração sobre todas as dependências** (storage, crypto, rede, fs). Descartada: YAGNI clássico — começamos com traits só para o que é genuinamente dependente de plataforma, e adicionamos quando aparecer uma segunda implementação real.
- **Núcleo em outra linguagem** (TypeScript/Node compartilhado entre casca e servidor). Descartada: o ecossistema de bibliotecas Python para documentos (openpyxl, python-docx, reportlab) e o de bibliotecas Rust para binários sidecar e sandbox apontam para Rust como núcleo. Manter a coerência de tipos ponta a ponta (Rust → IPC → TS) é mais valioso que uma segunda linguagem no servidor.
- **Reescrever o motor quando o servidor chegar.** Descartada: a história do projeto anterior mostra exatamente como isso termina — produtos com bases reescritas raramente sobrevivem à reescrita.

## Consequências

**Mais fácil:**
- A v1 desktop e um eventual servidor futuro compartilham o mesmo motor, com implementações de plataforma diferentes.
- Testes do motor rodam rápido, sem GUI, paralelizáveis.
- Adoção de uma nova plataforma no futuro (ex: macOS) vira "escrever um novo adapter de `Platform`", não reescrita.

**Mais difícil:**
- Curva de aprendizado para o time: todo código novo precisa pensar "isso depende de plataforma? então entra por trait".
- Overhead de manter dois lados do contrato (Rust ↔ JSON ↔ TS) consistente; `packages/shared-contracts/` precisa ter testes próprios.
- Implementações simuladas de `Platform` para testes (`FakeCredentialStore`, `FakeSandbox`, etc.) precisam ser mantidas — vira um pequeno framework de mocks que envelhece.
- Risco real: alguém vai ser tentado a importar `tauri` direto de um crate para "pular" uma abstração. A regra precisa de guard no CI (lint / `cargo deny` / script de varredura).

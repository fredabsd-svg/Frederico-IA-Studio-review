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
| 2 | Chat e provedores | não iniciada | — | depende da Fase 1 |
| 3 | Motor de execução e ferramentas | não iniciada | — | depende da Fase 2 |
| 4 | Memória e continuidade | não iniciada | — | depende da Fase 3 |
| 5 | Documentos | não iniciada | — | depende da Fase 3 |
| 6 | Multimodelo e subagentes | não iniciada | — | depende da Fase 3 + 4 |
| 7 | Modo desenvolvedor | não iniciada | — | depende da Fase 3 |
| 8 | Copiloto, tarefas e refinamento | não iniciada | — | depende de 3 + 4 + 6 + 7 |
| 9 | Produção | não iniciada | — | depende de todas |

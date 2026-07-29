<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 1-9
-->

# Estratégia de Testes

Cinco camadas (`PROMPT MESTRE` §28). Cada invariante declarada nos specs de `docs/architecture/` tem pelo menos um teste que a prova. O CI fecha o ciclo (`REGRAS §1.10` e `§1.13`).

## Camadas

### 1. Unit

- Roda sem I/O, sem rede, sem worker, sem banco.
- Cobertura: tipos, máquina de estados (`agent-state-machine.md` §6.1), permissões (`tool-permission-model.md` §8), schemas, validação de tool calls, normalização de caminhos.
- Framework: `cargo test` no Rust; `vitest` no TS.
- Localização: `#[cfg(test)]` em `crates/*/src/**.rs` e `*.test.ts` em `apps/desktop/src/`.

### 2. Integration

- Roda contra banco SQLite em modo WAL, com workers **simulados em processo** (mesma crate, mas com spawn desabilitado e adapter mockado).
- Cobertura: IPC entre núcleo e workers (com adapter mockado), persistência de runs e checkpoints, recuperação após crash, replay de eventos (`PROMPT MESTRE` §12.6), interseção de inventário.
- Localização: `tests/integration/`.

### 3. E2E

- Roda o app Tauri em modo headless (`tauri-driver`), contra workers reais ou simulados conforme o teste.
- Cobertura: **fluxos verticais do `PROMPT MESTRE` §33** (mensagem → execução → tool call → persistência → recuperação; planilha → revisão multimodelo); medições de desempenho (`PROMPT MESTRE` §23.7); recarga de janela durante execução (`PROMPT MESTRE` §12.6); LGPD (exportar e excluir conta).
- Localização: `tests/e2e/`.

### 4. Caos e recuperação (`PROMPT MESTRE` §28.4)

- Encerra forçadamente: app, modelo (corta conexão), stream, worker, ferramenta, document worker, sandbox, pipeline, banco (durante transação controlada).
- Verifica: integridade dos checkpoints, estado recuperável, ausência de duplicidade, arquivos preservados, processos encerrados, **zero processos órfãos**.
- Localização: `tests/recovery/`.

### 5. Máquina limpa (`PROMPT MESTRE` §28.5)

- Windows 10/11 64 bits **sem** Docker, WSL, Node, Python, PostgreSQL, Git, Office, compiladores.
- Apenas o instalador é executado. App abre, executa fluxo 1 e fluxo 2.
- Ambiente provisionado por CI runner descartável (imagem limpa a cada execução).
- Localização: `tests/installer/` (testa o instalador) e pipeline dedicada.

## Mapeamento invariante → teste

Cada spec em `docs/architecture/` lista invariantes com o rótulo "verificável em teste". Cada invariante gera pelo menos um teste na camada apropriada. Tabela mantida em `docs/testing/invariant-coverage.md` (a ser criado quando o primeiro spec for promovido a `parcialmente implementado`).

**Exemplos:**

- "Subagente nunca tem mais permissão que o pai" (`tool-permission-model.md`) → teste unit parametrizado: para todo par `(pai, filho)` válido, `perm(filho) ⊆ perm(pai)`.
- "Toda execução persiste no estado `created` antes de qualquer outra coisa" (`agent-state-machine.md`) → teste de integração que mata o processo imediatamente após submeter mensagem e verifica o `Run` no banco.
- "Path traversal é bloqueado" (`security-threat-model.md` I3) → teste E2E que tenta `..\..\etc\passwd` em cada tool de arquivo.

## Dados de teste

- Fixtures versionadas em `packages/testing-fixtures/`.
- **Provedor simulado**: `crates/provider-engine/src/fake/` — implementa `ProviderAdapter` retornando respostas determinísticas baseadas em fitas (golden files) versionadas.
- **Ferramenta simulada**: `crates/tool-registry/src/fake/` — implementa ferramentas triviais para testes de fluxo (eco, gerador de erro, gerador de arquivo).
- **Worker simulado**: `crates/process-architecture/src/fake/` — implementa o envelope IPC em processo, sem spawn real.

## Desempenho (`PROMPT MESTRE` §23.7)

- Testes E2E medem: tempo até janela visível (< 2 s), tempo até digitar (< 4 s), latência de digitação em conversa longa (< 50 ms).
- **Máquina de referência declarada**: i5-3570, 16 GB, Windows 10 64 bits. CI fixa essa máquina como gate.
- Medições ficam registradas em `docs/testing/perf-baseline.md` por versão, com tolerância de ±10% para evitar fragilidade.

## CI (REGRAS §1.10 + §1.13 + REGRA 2)

O que o pipeline verifica está abaixo. **Quando ele fica vermelho, quem manda é a REGRA 2**: `main` verde é pré-condição para mesclar, promover fase, promover spec, iniciar a fase seguinte ou publicar release; re-run diagnostica mas não absolve; teste instável é defeito bloqueante com prazo.

O pipeline falha em:

- qualquer teste falhando;
- cobertura de invariante nova sem teste;
- quebra de orçamento de desempenho;
- link interno de doc quebrado (`markdown-link-check` ou similar);
- spec com `Estado: implementado` e carimbo de verificação vencido;
- spec com `Estado: especificado` cuja fase está "em andamento" no `status.md` (§1.13);
- fase marcada "concluída" no `status.md` sem a suíte da fase verde (§1.10);
- arquivo gerado divergente da fonte (`REGRAS §1.9`).

### O que o pipeline cobra hoje

`scripts/check-docs.mjs` (passo "Docs guard") e `scripts/check-doc-impact.mjs` (passo "Doc-impact guard") implementam:

| Verificação | Situação |
|---|---|
| Cabeçalho de spec ausente, malformado ou com `Estado` fora da lista | cobrado |
| Carimbo de verificação vencido (60 dias) nos estados implementados | cobrado |
| Trava do §1.13, com a isenção de escopo global | cobrado |
| Crate/pacote sem o documento do §1.4 | cobrado |
| Link interno ou âncora quebrada | cobrado |
| PR que mexe em migrações / tool-registry / contratos sem tocar docs | cobrado, com a válvula do §1.3 |
| Fase "concluída" sem a suíte verde | implícito: `cargo test --workspace` roda antes, no mesmo job — job vermelho reprova o PR inteiro |
| Arquivo gerado divergente da fonte (§1.9) | **não cobrado** — não existe script de geração no repositório para comparar. Quando o primeiro existir, o check entra junto |

## Não-objetivos

- 100% de cobertura de linha cego (cobertura é meio, não fim; o que importa é invariante coberto).
- Testes flaky tolerados (qualquer teste flaky é bloqueante até estabilizar ou ser substituído).
- Testes de UI com snapshot pixel-a-pixel (testa-se comportamento e acessibilidade, não renderização exata).
- Mutation testing na v1 (caríssimo, retorno incerto).
- Teste manual de UI como substituto de E2E automatizado.

## Decisões

Nenhuma nova nesta versão. Decisões a tomar na Fase 1 (com ADR próprio):

- Stack de E2E: `tauri-driver` vs. Playwright vs. custom.
- Provedor simulado: replay de fita (golden files) vs. gerador determinístico (estado em memória).
- Onde rodar testes de "máquina limpa": runner self-hosted, GitHub Actions, Buildkite, ou outro.

## Referências

- `PROMPT MESTRE` §23.7 (desempenho), §28 (testes), §32 (critérios de aceite)
- `REGRAS-DO-PROJETO.md` §1.10, §1.13
- [`security-threat-model.md`](./security-threat-model.md) — testes de segurança
- [`docs/status.md`](../status.md) — fases e critério de promoção

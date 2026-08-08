# 0035 — Semântica de sobrescrita de `files.write` e `files.edit`

## Contexto

A Fase 7 introduz `files.write` (cria ou sobrescreve arquivo) e `files.edit` (substitui trecho com find/replace) no `ToolRegistry`. A Fase 6 fechou o `Jail` como barreira primária de path safety (Etapa 5.X, PR #25), o que cobre **onde** se escreve. Este ADR fecha **o que** acontece com o conteúdo que já está lá — e a regra não é trivial.

Três classes de bug que a decisão errada introduz:

1. **Sobrescrita silenciosa**: o agente lê `config.toml`, edita uma linha, salva — mas a API do `toml` re-serializa **todo o arquivo**, com mudanças de ordem de chaves, comentários removidos, valores em outro formato. O usuário perde config legível sem aviso. Mesmo problema com YAML, JSON reformatado, Markdown com frontmatter.
2. **Escrita parcial em crash**: o agente escreve 4 KB de um arquivo de 10 KB e o app morre (ou o filho do sandbox morre). O arquivo fica corrompido, e a próxima leitura falha em parser — sem estado "sabe-se que está corrompido". Sem atomicidade, o usuário perde o arquivo a cada crash de meio de escrita.
3. **Perda silenciosa de histórico**: o usuário tinha `main.py` funcionando; o agente sobrescreve com versão bugada; o usuário não tem como recuperar. A regra "git é a fonte de verdade de histórico" não vale aqui (o usuário pode não ter `git` no projeto; e o `git` da Fase 8 não existe ainda na Fase 7).

A Etapa 5 da Fase 7 implementa `files.write` e `files.edit`. Sem a política de sobrescrita fechada na Etapa 1, a Etapa 5 vai escolher default no código (o que é exatamente o que o §1.1 da REGRA 1 proíbe: "Documento que descreve intenção, funcionalidade futura ou comportamento que não existe mais é um defeito da mesma gravidade de um bug em produção").

A regra do `PROMPT MESTRE` §22.3 ("Acesso externo ao workspace só com: seleção pelo usuário, concessão de permissão, definição de leitura/escrita, registro, possibilidade de revogação") não fala de sobrescrita explicitamente. A decisão é da Fase 7.

## Decisões

### D1 — `files.write` é atômico: escreve em `.tmp.<uuid>`, fsync, rename

Toda escrita de `files.write` segue o protocolo atômico de 3 passos:

1. Resolve o `path` final via `Jail` (D2 do ADR-0031) — `Jail::resolve(path)` devolve o `CanonicalPath` ou rejeita.
2. Gera um `temp_path = <path>.tmp.<uuid_v4>` (uuid v4 random, 16 bytes) **dentro do mesmo diretório** (rename atômico só funciona no mesmo filesystem, então o `temp_path` tem que estar no mesmo dir que `path`).
3. `std::fs::write(temp_path, content)` + `temp_path.sync_all()` (fsync do arquivo) + `dir.sync_all()` (fsync do diretório, garante que o rename é durável).
4. `std::fs::rename(temp_path, path)` — atômico em Windows (POSIX `rename(2)` é atômico; `MoveFileEx` com `MOVEFILE_REPLACE_EXISTING` é atômico no mesmo volume).
5. Se qualquer passo falha, o `temp_path` é deletado (cleanup) e o `path` original fica intacto.

A regra "rename atômico" é o que fecha o caso "crash de meio de escrita" — ou o arquivo é o antigo, ou é o novo, nunca corrompido.

`O_NOFOLLOW` no `open` do `temp_path` (Linux) e `FILE_FLAG_OPEN_NO_RECALL` + `FILE_ATTRIBUTE_TEMPORARY` (Windows) são camadas adicionais contra symlink attacks. A Etapa 5 da Fase 7 implementa; a Etapa 5.X da Fase de Ligação (PR #25) já tinha `O_NOFOLLOW` no `document-worker` Python — a Etapa 5 da Fase 7 transpõe para o executor Rust.

**Teste de regressão** (regra do user: "teste de negação"): `crates/e2e/tests/e2e_atomic_write.rs::crash_between_write_and_rename_leaves_original_intact` simula crash entre passo 3 e passo 4 (via `temp_path` pré-existente que o teste injeta para forçar a falha), afirma que `path` original tem o conteúdo antigo e `temp_path` foi limpo. Sem isso, a garantia "atômico" é só palavras no spec.

### D2 — `files.write` com `overwrite: false` (default) recusa se existe

Default do parâmetro `overwrite` em `files.write` é `false`. Se o `path` final já existe, o tool_call retorna `Err(OverwriteRequired)` com mensagem clara ("arquivo já existe; passe `overwrite: true` para substituir"). O conteúdo não é tocado.

A escolha do `overwrite: false` por default é a regra "default deny" da Fase 7 (D1 do ADR-0034): o tool_call mais comum (criar arquivo novo) **funciona sem pergunta**; o tool_call destrutivo (sobrescrever) **pede confirmação explícita** via parâmetro.

Para sobrescrever: o usuário (via UI) ou o modelo (via tool_call) passa `overwrite: true`. O tool_call então executa o protocolo atômico de D1, **e** cria um backup (D3 abaixo).

### D3 — Sobrescrita (`overwrite: true`) cria backup automático `.bak` no mesmo diretório

Quando `files.write` recebe `overwrite: true` e o `path` existe:

1. Calcula `backup_path = <path>.bak` (mesmo diretório).
2. Se `backup_path` já existe (escrita anterior sobrescreveu), calcula `backup_path = <path>.bak.<timestamp>` (ISO 8601 compactado, `20260808T104200Z`).
3. Copia o conteúdo atual de `path` para `backup_path` (cópia inteira, não rename — preserva o `path` original para o caso da escrita subsequente falhar).
4. Prossegue com o protocolo atômico de D1.
5. Se passo 1-3 falha, o tool_call retorna `Err(BackupFailed)` **sem** tocar no `path` original.

A regra "backup sempre que sobrescreve" é o que fecha o caso "perda silenciosa de histórico" (classe de bug 3 acima). O backup é local (mesmo diretório), versionado por timestamp, e **não é commitado em git** automaticamente — o usuário decide o que fazer com os `.bak` (a Etapa 8 do Modo Desenvolvedor integrado dá UI de "limpar backups antigos").

**Não-objetivo:** backup incremental / versionamento completo. O backup é **uma** cópia anterior; o usuário que quer histórico completo usa git (Fase 8).

**Teste de regressão:** `crates/e2e/tests/e2e_overwrite_backup.rs::overwrite_creates_backup_with_previous_content` — escreve `A`, sobrescreve com `B` (overwrite=true), afirma que `path == B` e `path.bak == A`.

### D4 — `files.edit` requer `find` único, recusa múltiplos matches

`files.edit` é a primitiva de "encontre e substitua":

```json
{
  "path": "src/main.py",
  "find": "def hello():",
  "replace": "def hello(name):",
  "replace_all": false
}
```

Regras:

- `find` deve aparecer **exatamente uma vez** no arquivo, a menos que `replace_all: true` seja passado. Se aparece 0 vezes, o tool_call retorna `Err(PatternNotFound)`. Se aparece 2+ vezes sem `replace_all: true`, retorna `Err(AmbiguousMatch)`.
- `find` é **texto literal**, não regex. Razão: regex silenciosamente casa mais do que o usuário espera (especialmente `.` `*` `+` `?` `(` `[` `\`), e a primitiva "find literal" cobre 95% do uso de `files.edit`. Regex fica para Fase 8 (com UI de "test pattern").
- O replace preserva indentação: o `replace` é inserido com a indentação do primeiro char do `find` (medido na linha). Razão: o uso comum é "muda a definição desta função", e o usuário quer que a substituição mantenha a indentação do código existente.
- A operação é atômica (D1): o arquivo é lido, o replace é calculado em memória, e a gravação usa o protocolo atômico. Se o replace falha (ex.: `find` mudou entre read e write por outra invocação), o arquivo original fica intacto.

**Não-objetivo:** `files.edit` com **conflito de mudança concorrente**. Sem locking, se duas invocações de `files.edit` rodam em paralelo no mesmo arquivo, a última a gravar vence (com a primeira já feita). A Etapa 5 da Fase 7 documenta essa lacuna; a Fase 8 (com UI de projeto) pode adicionar lock por arquivo.

**Teste de regressão:** `crates/e2e/tests/e2e_edit_semantics.rs::edit_with_no_match_returns_pattern_not_found`, `::edit_with_multiple_matches_returns_ambiguous_match_without_replace_all`, `::edit_with_replace_all_replaces_all_occurrences`, `::edit_preserves_indentation`.

### D5 — `files.write` com `create_parents: true` cria diretórios intermediários

`files.write` aceita o parâmetro opcional `create_parents: bool` (default `false`). Quando `true`, diretórios intermediários do `path` que não existem são criados com `std::fs::create_dir_all` (idempotente — não falha se já existe).

A regra "default `create_parents: false`" é coerente com D2: o tool_call mais comum (sobrescrever arquivo existente) não precisa de `create_parents`; o tool_call que cria estrutura nova (criar `src/utils/helper.py` quando `src/utils/` não existe) precisa do opt-in explícito.

**Limitação:** `create_parents` não escapa do `Jail` — diretórios intermediários são validados pela mesma rotina de `Jail::resolve_allowing_nonexistent` (Fase 6 Etapa 5.X, PR #25). Se o `path` é `/etc/passwd`, o `Jail` rejeita, e `create_parents` nem é tentado.

### D6 — Toda escrita tem entrada no `DbAuditSink` com `before` e `after` (hashes, não conteúdo)

O `DbAuditSink` (Fase 3, tabela `tool_audit`, migration 0005) registra cada `files.write` e `files.edit`:

```json
{
  "kind": "file_write",
  "tool": "files.write",
  "path": "src/main.py",
  "before_sha256": "abc123..." | null,   // null se arquivo não existia
  "after_sha256": "def456...",
  "bytes_written": 1234,
  "overwrite": true,
  "backup_path": "src/main.py.bak" | null,
  "approved_scope": "OneTurn"
}
```

A regra "SHA-256, não conteúdo" é o que fecha a porta de "audit log vaza o que o usuário escreveu". O log tem os **hashes** (que o usuário pode usar para provar que algo foi escrito, mas não o conteúdo em si). O conteúdo está no backup `.bak` (D3) e no `path` final.

Para `files.edit`, o audit registra o `find` e o `replace` literais (não os hashes) — para que a UI possa mostrar "o que mudou" no histórico. O `find`/`replace` são texto, mas o audit log é local, no SQLite do usuário (mesma trilha do `R1` do threat model), e o `T1` do threat model (filtro de logging) já veta que conteúdo de **credencial** entre no log.

**Teste de regressão:** `crates/e2e/tests/e2e_audit_logging.rs::file_write_audit_contains_hashes_not_content` — escreve um arquivo com credencial fake no conteúdo, lê o `tool_audit`, afirma que o conteúdo **não** aparece (só os hashes).

### D7 — `files.write` e `files.edit` rodam **fora** do sandbox (herdam só o Jail)

Diferente de `exec.python`/`exec.node` (que rodam sob sandbox com Job Object + Restricted Token + env zeroed, ADR-0031), `files.write` e `files.edit` rodam **dentro do processo do app** — sem spawn de filho, sem sandbox de processo. A barreira é o `Jail` (D2 do ADR-0031) + o protocolo atômico (D1) + o backup (D3) + o audit (D6).

A razão: spawnar um filho só para escrever um arquivo é overhead desproporcional (criar processo, atribuir Job Object, configurar env, esperar exit). O ganho de segurança do sandbox de processo (Job Object + Restricted Token) **não se aplica** — não há execução de código arbitrário, só `std::fs::write` em Rust, dentro do app que já roda sob todas as camadas de segurança do próprio OS (usuário limitado, AppLocker se ativo, etc.).

A Etapa 5 da Fase 7 implementa como método do `frederico-tool-registry` (no mesmo crate que `FilesReadTool`), não como filho de `DocumentWorkerLauncher` (que é o padrão dos kits de documento da Fase 5).

**Teste de regressão:** `crates/e2e/tests/e2e_files_write_orchestrator.rs::files_write_runs_in_process_no_child_spawned` — abre o `ChatOrchestrator`, conta processos filhos antes e depois de `files.write`, afirma que a contagem **não mudou**.

## Consequências

- O `ToolRegistry` ganha 3 ferramentas novas: `FilesWriteTool`, `FilesEditTool`, `FilesListTool` (a última só lista diretório, sem leitura de conteúdo — é o `ls` da Fase 7). Cada uma no `frederico-tool-registry/src/files_*` (~300 linhas cada estimada).
- O `Jail` (Fase 6 Etapa 5.X) ganha 1 método novo: `Jail::resolve_allowing_nonexistent` (já existe da Etapa 5.X, mas só para `docs.generate`; a Etapa 5 da Fase 7 o generaliza para `files.write` com `create_parents: true`).
- A UI da Fase 7 Etapa 7 ganha componente `FileWriteDiff` (mostra o diff de `before` vs `after` para confirmação na hora da aprovação, quando escopo é `OneExecution`).
- A Etapa 5 da Fase 7 implementa D1-D7. A Etapa 5.X (se necessária) corrige bugs latentes (mesmo padrão da Fase de Ligação).
- O `db/migrations/0038_file_audit.sql` (quando entrar) adiciona índice em `tool_audit.path` para que a UI de "histórico de mudanças no arquivo X" seja query barata. Opcional para a v1.

## Alternativas consideradas

1. **Sem atomicidade (escrita direta)**. Rejeitado por classe de bug 2 (escrita parcial em crash). Atômico é barato em Rust (`std::fs::write` + `rename`) e o teste de regressão é trivial.
2. **Sempre sobrescrever, sem backup**. Rejeitado por classe de bug 3 (perda silenciosa de histórico). Backup local no mesmo diretório é o mínimo que fecha.
3. **Backup versionado em diretório dedicado** (`~/.frederico/backups/<project>/<path>/<timestamp>`). Rejeitado por (a) aumenta superfície de "onde está o backup?", (b) diretório dedicado acumula lixo sem limpeza, (c) o `.bak` no mesmo dir é o padrão Unix de `cp`/`mv`/`install` (o usuário já conhece). Fase 8 (com UI de projeto) pode introduzir backup versionado se necessário.
4. **`files.edit` com regex**. Rejeitado por (a) regex silenciosamente casa mais do que o usuário espera, (b) o modelo que usa `files.edit` não precisa de regex para 95% dos casos (substituir definição de função, mudar string de config, ajustar import), (c) o 5% que precisa fica para Fase 8 (com `files.regex_edit` ou similar).
5. **`files.write` roda sob sandbox** (igual `exec.python`). Rejeitado por overhead (D7): spawnar processo para escrever arquivo é caro, e a barreira de Jail + atomicidade + backup + audit é o que cobre as ameaças reais. O sandbox de processo não adiciona garantia que o Jail já não dá.
6. **Lock por arquivo** (impedir `files.edit` paralelo no mesmo path). Rejeitado para a v1: o cenário "duas invocações paralelas no mesmo arquivo" é raro e o usuário consegue serializar manualmente. Fase 8 (com UI de projeto) introduz lock se virar problema real.

## Pendências

- **Cleanup de `.bak` antigos** — sem limpeza, o diretório acumula. A Etapa 5 da Fase 7 implementa **uma** versão de backup (o `.bak` mais recente) + **uma** opcional timestamped quando há colisão. Cleanup de `.bak` com mais de N dias é Etapa 7 (UI/Polish) — checkbox "limpar `.bak` com mais de 30 dias".
- **`files.write` em arquivos grandes (>10 MB)** — a leitura inteira + escrita inteira em memória não escala. A Etapa 5 da Fase 7 recusa arquivos > 10 MB com erro claro; streaming é roadmap de Fase 8+ (com `files.append` que é só append, e `files.write_streaming` que é chunked).
- **Conflito read-modify-write entre exec e write** — se `exec.python` está rodando em `src/main.py` e o usuário chama `files.edit` no mesmo path via outro tool_call, a ordem é **indefinida**. A Etapa 5 da Fase 7 documenta: `files.write`/`files.edit` em path que está sendo usado por `exec.*` em curso é **race condition aceita**, e o usuário vê o resultado imprevisível. A Fase 8 introduz lock se virar problema.
- **Permissões de arquivo do OS** — `files.write` herda as permissões do `umask` do processo do app. Se o app roda com `umask 077`, os arquivos são `600`. Se roda com `umask 022`, são `644`. A Etapa 5 da Fase 7 não normaliza; o usuário que precisa de permissão específica usa `chmod` (que entra via `exec.shell` na Etapa 6).
- **Atomicidade em Windows com `MoveFileEx`** — `MoveFileEx` é atômico **no mesmo volume**. Se o `<path>` está em volume diferente do `temp_path` (ex.: `C:\Users\foo` vs `D:\workspace`), o rename copia + deleta, não é atômico. A Etapa 5 da Fase 7 garante `temp_path` no mesmo diretório (que garante mesmo volume no caso comum) e **rejeita** a escrita se o `temp_path` calculado cai em volume diferente.
- **Detecção de "write race" com `git` na Fase 8** — quando o `git` entrar, `files.write` em path versionado pode criar inconsistência com `git status`. A Etapa 5 da Fase 7 não trata (git não existe). A Fase 8 do Modo Desenvolvedor integrado ganha coordenação.

## Histórico de revisão

- 2026-08-08 — versão inicial. Decisão da Etapa 1 da Fase 7. Validação pelo user (via `ask_user`): "Ferramentas de escrita são o primeiro salto de risco real. Até hoje só existe `files.read`. `files.write` e `files.edit` destroem dados; a política de aprovação, a barreira de caminho já ligada no PR #25 e o comportamento em sobrescrita precisam ser decididos antes, não durante." O protocolo "atômico + backup + audit + diff de confirmação" é o que fecha as 3 classes de bug sem as quais a ferramenta é insegura de usar.

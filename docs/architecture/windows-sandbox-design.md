<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 3
-->

# Design do Sandbox Windows (stub)

> Stub criado na Fase 0. Será aprofundado antes do início da Fase 3 (Motor de execução e ferramentas) — sandbox é estrutural, retrofit é caríssimo.

## Decisão tomada

- **Sem Docker, sem WSL, sem `PATH` global alterado** (`PROMPT MESTRE` §5.2, §22). Isolamento via primitivas do Windows.
- **Workspace em `%LOCALAPPDATA%\FredericoAIStudio\workspaces\`** (`PROMPT MESTRE` §22.2) — agente não acessa diretamente documentos pessoais, área de trabalho, credenciais, navegador, registro, pastas de sistema, outros projetos.
- **Acesso externo ao workspace só com**: seleção pelo usuário, concessão de permissão, definição de leitura/escrita, registro, possibilidade de revogação (`PROMPT MESTRE` §22.3).
- **Python e Node como pacotes gerenciados**, não como dependência de instalação do usuário (`PROMPT MESTRE` §22.4). Não alterar `PATH` global.
- **Env do processo filho zerado e reconstruído por allowlist**; ambiente do app nunca é herdado (`PROMPT MESTRE` §22.5).
- **Teste automatizado obrigatório**: nenhuma variável de ambiente de processo do sandbox contém valor de credencial cadastrada (`PROMPT MESTRE` §22.5).
- **Rede do sandbox só através de proxy local do app**, com allowlist e registro de URLs visível ao usuário na conversa (`PROMPT MESTRE` §22.5).
- **Comandos aprovados são exibidos ao usuário exatamente como serão executados, sem abreviação** (`PROMPT MESTRE` §22.5 final).

## Contrato previsto

```rust
struct SandboxConfig {
    isolation: IsolationLevel,        // AppContainer | RestrictedToken | JobObject
    resources: ResourceLimits,        // cpu, mem, processes, wall_clock
    network: NetworkPolicy,           // None | Allowlist(Vec<String>)
    env: EnvPolicy,                   // None (zerado) | Allowlist(Vec<String>)
    workspace_root: AppPath,          // %LOCALAPPDATA%\FredericoAIStudio\workspaces\<id>
    external_mounts: Vec<ExternalMount>,  // pastas externas concedidas pelo usuário
}

struct ResourceLimits {
    max_cpu_pct: u8,                  // 0-100
    max_memory_mb: u32,
    max_processes: u32,
    max_wall_clock: Duration,
}
```

## Não-objetivos

- Sandboxing de GPU.
- Anti-debug, anti-tamper.
- "Browser sandbox" completo (o `browser-worker` é separado, fora do sandbox principal).
- Suporte a sandbox em macOS/Linux na v1 (a arquitetura está pronta, mas a implementação Windows é a única da v1).

## Aprofundar antes da Fase 3

- **Mecanismo de isolamento por tipo de execução**: quando AppContainer vs. Restricted Token vs. Job Object. AppContainer é mais isolado mas quebra alguns runtimes Python; Restricted Token é mais compatível mas isola menos; Job Object é leve e bom para limites de recursos mas não isola filesystem. Decidir com base no tipo de execução (`exec.python` é mais restritivo que `files.read`).
- **Mecanismo de kill em árvore**: como garantir que subprocessos do subprocesso são encerrados quando o sandbox é destruído (Job Object no Windows resolve; documentar).
- **Proxy de rede local**: como implementar, onde roda, o que registra.
- **Estratégia de provisionamento do workspace**: quando criar, quando limpar, como sobreviver a crash do app.
- **Política de revogação** de acesso externo: o que acontece com arquivos em uso quando o usuário revoga.
- **Detecção de "tentativa de fuga"**: o que medir (acesso a `C:\Users\<outro>`, leitura de `SAM`, etc.) e o que fazer.
- **Testes de sandbox em CI**: como reproduzir Windows real no CI runner (a maioria dos runners é Linux); alternativa é um conjunto de testes de integração que não isola de fato mas valida as primitivas.

## Decisões

Nenhuma nova. Decisões serão tomadas quando o spec for aprofundado (especificamente: combinação de primitivas por tipo de execução, e o destino do proxy de rede).

## Referências

- `PROMPT MESTRE` §22 (execução local sem Docker), §22.5 (segredos e rede)
- [`security-threat-model.md`](./security-threat-model.md) — I1 (env leak), I3 (path traversal), D1 (worker travado), E1 (escalada de privilégio)
- [`process-architecture.md`](./process-architecture.md) — como o sandbox se relaciona com workers
- `docs/development-roadmap.md` (Fase 3)

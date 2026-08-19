//! `PermissionSet` — o conjunto de permissões de um agente/Run.
//!
//! O spec `tool-permission-model.md` §"Hierarquia" lista 7 camadas
//! (permissões globais → ... → aprovação do usuário). A Etapa 3
//! entrega o **tipo** `PermissionSet` (com todos os campos do spec
//! §"Contrato") e o invariante **"subagente ⊆ pai"** (verificável
//! em teste) — a integração da hierarquia em runtime é da Fase 6
//! (subagentes).
//!
//! **Default é deny** (spec §"Invariantes"):
//! "Default é deny: ferramenta perigosa nasce desligada; ligar é
//! decisão consciente." `PermissionSet::default()` tem todos os
//! campos em `false` / `None`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Permissão de leitura de arquivo. Spec `tool-permission-model.md`
/// §"Contrato" — `file_read: bool` virou enum pra suportar o caso
/// "dentro do workspace direto / fora exige aprovação".
///
/// A Etapa 3 implementa apenas o eixo `file_read` em runtime (Passo
/// 5 do `validate_tool_call`). Os outros eixos do `PermissionSet`
/// ficam tipados e com `Default` deny; a aplicação por eixo é da
/// Etapa 4 (integração) em diante, à medida que as ferramentas que
/// usam cada eixo chegarem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileReadPermission {
    /// `false` no spec antigo. Negado por default; usuário precisa
    /// ligar explicitamente.
    #[default]
    None,
    /// `true` no spec antigo, mas restrito ao workspace (o Jail já
    /// garante).
    WorkspaceOnly,
    /// `true` no spec antigo, sem restrição de jail. A Etapa 6
    /// (UI) consome essa flag para mostrar o modal de aprovação
    /// quando o usuário tenta ler pasta autorizada do PC.
    WorkspacePlusApproved,
}

impl fmt::Display for FileReadPermission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::WorkspaceOnly => f.write_str("workspace_only"),
            Self::WorkspacePlusApproved => f.write_str("workspace_plus_approved"),
        }
    }
}

/// Permissão de execução de runtime (Python, Node, etc.).
/// Spec §"Contrato": `RuntimePermission { None, ReadOnly, Sandboxed, Unrestricted }`.
/// Ordem de "permissividade": `None < ReadOnly < Sandboxed < Unrestricted`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePermission {
    #[default]
    None,
    ReadOnly,
    Sandboxed,
    Unrestricted,
}

impl fmt::Display for RuntimePermission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::ReadOnly => f.write_str("read_only"),
            Self::Sandboxed => f.write_str("sandboxed"),
            Self::Unrestricted => f.write_str("unrestricted"),
        }
    }
}

/// Permissão de terminal. Spec §"Contrato":
/// `TerminalPermission { None | RequireApproval | Denylist | Allowlist }`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalMode {
    #[default]
    None,
    RequireApproval,
    Denylist,
    Allowlist,
}

impl fmt::Display for TerminalMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::RequireApproval => f.write_str("require_approval"),
            Self::Denylist => f.write_str("denylist"),
            Self::Allowlist => f.write_str("allowlist"),
        }
    }
}

/// Permissão de Git local.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum GitPermission {
    /// Nada — `git` desligado.
    #[default]
    None,
    /// Leitura (`status`, `log`, `diff`).
    ReadOnly,
    /// Leitura + operações locais (`add`, `commit`, `branch`).
    Local,
    /// Tudo, exceto push (que exige `GitHubPermission::Push`).
    Full,
}

impl fmt::Display for GitPermission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::ReadOnly => f.write_str("read_only"),
            Self::Local => f.write_str("local"),
            Self::Full => f.write_str("full"),
        }
    }
}

/// Uma regra de GitHub como ela aparece no perfil ([ADR-0049] §D1).
///
/// **Dado simples, não o tipo que decide.** A `MatrizAutorizacao` do
/// `github-engine` é quem autoriza, e ela é construída a partir disto
/// na composição. Manter os dois separados evita que mudar o formato
/// do arquivo passe a mexer no portão — é o mesmo arranjo do
/// `network_allowlist`, que é `Vec<String>` aqui e `NetworkAllowlist`
/// no proxy.
///
/// **O enum `GitHubPermission` saiu** ([ADR-0049] §D3). Ele era uma
/// escala linear (`None` < `ReadOnly` < ... < `Push`), que é
/// moralmente o mesmo defeito que o [ADR-0041] §D2 rejeita: "pode até
/// Push" autorizava empurrar para qualquer repositório e qualquer
/// branch. Dois eixos para a mesma decisão produziriam a pergunta
/// "qual dos dois vale?", e a resposta certa nunca é "os dois".
///
/// [ADR-0041]: ../../docs/decisions/0041-github-auth-e-matriz-de-autorizacao.md
/// [ADR-0049]: ../../docs/decisions/0049-matriz-de-github-no-permission-set.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RegraGithubPerfil {
    /// `owner/repo`.
    pub repo: String,
    /// Padrões de branch. Vazio = nenhuma.
    #[serde(default)]
    pub branches: Vec<String>,
    /// `read`, `push`, `create_pr`. Vazio = nenhuma.
    #[serde(default)]
    pub operacoes: Vec<String>,
}

/// Permissão de memória. Spec §"Contrato":
/// `MemoryPermission { None, ReadOnly, ReadWrite }`.
/// Ordem de "permissividade": `None < ReadOnly < ReadWrite`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPermission {
    #[default]
    None,
    ReadOnly,
    ReadWrite,
}

impl fmt::Display for MemoryPermission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::ReadOnly => f.write_str("read_only"),
            Self::ReadWrite => f.write_str("read_write"),
        }
    }
}

/// Permissão de geração documental. Spec §"Contrato":
/// `DocumentPermission { None, WorkspaceOnly, Full }`.
/// Ordem de "permissividade": `None < WorkspaceOnly < Full`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DocumentPermission {
    #[default]
    None,
    /// Pode gerar documentos, mas não exfiltra do workspace.
    WorkspaceOnly,
    /// Pode gerar e copiar pra fora.
    Full,
}

impl fmt::Display for DocumentPermission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::WorkspaceOnly => f.write_str("workspace_only"),
            Self::Full => f.write_str("full"),
        }
    }
}

/// **Default é deny** — todos os campos `false` ou variante mais
/// restritiva. `PermissionSet::default()` é o que o `Run.allowed_tools`
/// de um `Run` recém-criado carrega antes do `PermissionSet` real ser
/// carregado da configuração do projeto/assistente.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionSet {
    /// Leitura de arquivo (com a nuance WorkspaceOnly vs.
    /// WorkspacePlusApproved).
    pub file_read: FileReadPermission,
    /// Criação de arquivo (write, sem leitura implícita).
    pub file_create: bool,
    /// Modificação de arquivo existente.
    pub file_modify: bool,
    /// Deleção de arquivo.
    pub file_delete: bool,
    /// Permissão de terminal.
    pub terminal: TerminalMode,
    /// Execução Python.
    pub python: RuntimePermission,
    /// Execução Node.
    pub node: RuntimePermission,
    /// Git local.
    pub git: GitPermission,
    /// GitHub remoto, como matriz ([ADR-0049] §D1). Vazio = nenhum
    /// repositório autorizado, que é o default e o comportamento
    /// certo: sem entrada, o app não sabe para onde empurrar.
    ///
    /// [ADR-0049]: ../../docs/decisions/0049-matriz-de-github-no-permission-set.md
    pub github_repos: Vec<RegraGithubPerfil>,
    /// Navegação web.
    pub web_browse: bool,
    /// Download via web.
    pub web_download: bool,
    /// Rede genérica (proxy local do app, allowlist de domínios).
    pub network: bool,
    /// Hosts liberados no proxy de rede do sandbox (Etapa 7 da Fase
    /// 7, ADR-0033). **Não confundir com `network`** (o gate grosso
    /// liga/desliga o subsistema inteiro; esta lista é o filtro
    /// fino que o `NetworkAllowlist` do proxy consome). Vazio =
    /// deny-by-default (mesma regra do `NetworkAllowlist::new()`).
    /// Carregado do TOML de perfil (`permission_loader.rs`); sem
    /// wildcard — `allow_all()` **não** abre a rede inteira via
    /// esta lista (não há convenção de "*" no `NetworkAllowlist`
    /// hoje; abrir tudo silenciosamente aqui seria inventar um
    /// comportamento que o proxy não sabe interpretar).
    pub network_allowlist: Vec<String>,
    /// Captura de tela.
    pub screen_capture: bool,
    /// Controle de mouse/teclado.
    pub input_control: bool,
    /// Memória.
    pub memory: MemoryPermission,
    /// Acesso a credenciais (DPAPI).
    pub credentials: bool,
    /// Geração documental.
    pub documents: DocumentPermission,
    /// Operações destrutivas (irreversíveis) — exige aprovação.
    pub destructive_ops: bool,
}

/// Padrão "tudo desligado" — spec §"Invariantes": "Default é deny".
/// A Etapa 4 (integração) carrega o `PermissionSet` real do
/// `assistant` / `project` / `user` antes de validar.
impl Default for PermissionSet {
    fn default() -> Self {
        Self {
            file_read: FileReadPermission::None,
            file_create: false,
            file_modify: false,
            file_delete: false,
            terminal: TerminalMode::None,
            python: RuntimePermission::None,
            node: RuntimePermission::None,
            git: GitPermission::None,
            github_repos: Vec::new(),
            web_browse: false,
            web_download: false,
            network: false,
            network_allowlist: Vec::new(),
            screen_capture: false,
            input_control: false,
            memory: MemoryPermission::None,
            credentials: false,
            documents: DocumentPermission::None,
            destructive_ops: false,
        }
    }
}

impl PermissionSet {
    /// Permissão **total** — todas as categorias no nível máximo
    /// (sem `destructive_ops: true` que exigiria aprovação
    /// explícita). Usado pela Etapa 4 (integração) quando o usuário
    /// marca "Permitir tudo nesta execução" no modal de aprovação.
    #[must_use]
    pub fn allow_all() -> Self {
        Self {
            file_read: FileReadPermission::WorkspacePlusApproved,
            file_create: true,
            file_modify: true,
            file_delete: true,
            terminal: TerminalMode::Allowlist,
            python: RuntimePermission::Unrestricted,
            node: RuntimePermission::Unrestricted,
            git: GitPermission::Full,
            // `allow_all()` **não** vira curinga de repositório —
            // mesma razão pela qual o `network_allowlist` fica vazio
            // aqui (Fase 7 Etapa 7): não existe "todos os
            // repositórios" que o sistema saiba interpretar sem
            // inventar comportamento (ADR-0041 §D2).
            github_repos: Vec::new(),
            web_browse: true,
            web_download: true,
            network: true,
            // Sem wildcard no `NetworkAllowlist` — `allow_all()`
            // continua deny-by-default nesta lista fina (ver doc
            // do campo). O usuário configura hosts explícitos no
            // TOML de perfil mesmo em modo "permitir tudo".
            network_allowlist: Vec::new(),
            screen_capture: false,
            input_control: false,
            memory: MemoryPermission::ReadWrite,
            credentials: true,
            documents: DocumentPermission::Full,
            destructive_ops: true,
        }
    }

    /// **Invariante da Fase 6**: subagente nunca tem permissão que
    /// o pai não tem. `self.is_subset_of(&parent)` é `true` sse
    /// `self ⊆ parent` em cada eixo.
    ///
    /// Especificação (regras de subconjunto por eixo):
    /// - `bool`: `self.x ≤ parent.x`. `!self.x || parent.x`.
    /// - `FileReadPermission`: `None < WorkspaceOnly < WorkspacePlusApproved`.
    ///   `self == None` ou `(self == WorkspaceOnly && parent ∈ {WorkspaceOnly, WorkspacePlusApproved})`
    ///   ou `(self == WorkspacePlusApproved && parent == WorkspacePlusApproved)`.
    /// - `RuntimePermission` (e similares): variante ≤ variante
    ///   (ordem do `PartialOrd`).
    /// - `TerminalPermission`: `self.mode ≤ parent.mode`
    ///   (None < RequireApproval < Denylist < Allowlist — Denylist
    ///   e Allowlist são "opostos" mas o invariante é só que o
    ///   subagente não pode ter um modo mais permissivo que o
    ///   pai; a checagem fina é da Etapa 6).
    #[must_use]
    pub fn is_subset_of(&self, parent: &Self) -> bool {
        // file_read
        let file_read_ok = matches!(
            (self.file_read, parent.file_read),
            (FileReadPermission::None, _)
                | (
                    FileReadPermission::WorkspaceOnly,
                    FileReadPermission::WorkspaceOnly,
                )
                | (
                    FileReadPermission::WorkspaceOnly,
                    FileReadPermission::WorkspacePlusApproved,
                )
                | (
                    FileReadPermission::WorkspacePlusApproved,
                    FileReadPermission::WorkspacePlusApproved,
                )
        );
        if !file_read_ok {
            return false;
        }

        // booleans
        for axis in [
            self.file_create && !parent.file_create,
            self.file_modify && !parent.file_modify,
            self.file_delete && !parent.file_delete,
            self.web_browse && !parent.web_browse,
            self.web_download && !parent.web_download,
            self.network && !parent.network,
            self.screen_capture && !parent.screen_capture,
            self.input_control && !parent.input_control,
            self.credentials && !parent.credentials,
            self.destructive_ops && !parent.destructive_ops,
        ] {
            if axis {
                return false;
            }
        }

        // network_allowlist: subagente não pode alcançar host que
        // o pai não alcança (mesma regra "self ⊆ parent" aplicada
        // a conjunto, não a bool/enum).
        if !self
            .network_allowlist
            .iter()
            .all(|h| parent.network_allowlist.contains(h))
        {
            return false;
        }

        // enums com ordem (PartialOrd)
        if self.python > parent.python {
            return false;
        }
        if self.node > parent.node {
            return false;
        }
        if self.git > parent.git {
            return false;
        }
        // `github_repos`: cada regra do filho tem que caber numa do
        // pai — repositório presente, branches e operações contidas.
        // Regra que o pai não cita é regra que o filho não pode ter.
        for minha in &self.github_repos {
            let Some(dela) = parent.github_repos.iter().find(|r| r.repo == minha.repo) else {
                return false;
            };
            if !minha.branches.iter().all(|b| dela.branches.contains(b))
                || !minha.operacoes.iter().all(|o| dela.operacoes.contains(o))
            {
                return false;
            }
        }
        if self.memory > parent.memory {
            return false;
        }
        if self.documents > parent.documents {
            return false;
        }

        // terminal.mode: ordem simples (None < RequireApproval <
        // Denylist/Allowlist). Denylist e Allowlist são
        // equivalentes em termos de subconjunto (ambos são "não
        // é None, não é RequireApproval").
        if self.terminal > parent.terminal {
            return false;
        }

        true
    }
}

// `PartialOrd` e `Ord` para `TerminalMode` foram promovidos pro
// `#[derive(...)]` no enum (Etapa 3 PR 2). O spec não define a
// ordem; o derive usa a ordem de declaração das variantes:
// `None < RequireApproval < Denylist < Allowlist` — exatamente a
// que o `PermissionSet::is_subset_of` (Etapa 3, Fase 3) e o
// `PermissionSet::merge` (Etapa 3 PR 2) precisam. `Denylist` e
// `Allowlist` são "opostos" mas o invariante só usa a posição
// relativa (`sub ≤ parent`); checagem fina de patterns fica
// pra Etapa 6 (modo dev).

/// Interseção tripla de `PermissionSet`s — `PermissionSet::merge`.
///
/// **Semântica (Etapa 3 da Fase 6, ADR-0030 §D3, decisão de 2026-08-06
/// no PR 2):** o `effective` é o resultado de mesclar `user ⊆ project ⊆
/// assistant`, onde "mesclar" significa **"mais restritivo vence"** (a
/// interseção de cada eixo). Em cada eixo, o **min** dos 3 é o que vale
/// (ex.: se user = `network: true` mas project = `network: false`,
/// effective = `network: false`).
///
/// ## Fail-closed (princípio do projeto, regra do PR 2)
///
/// "Mais restritivo vence" é enforçado **em todos os eixos, sem
/// exceção**:
/// - **Bool**: `effective = self.x && other.x && third.x` (todos true ⇒
///   true; qualquer false ⇒ false). Um dos 3 ser false **nega** o
///   eixo.
/// - **Enum com ordem** (`RuntimePermission`, `GitPermission`, etc):
///   `effective = min(self, other, third)`. O mais restritivo vence.
///
/// **Consequência crítica:** se um dos 3 inputs tem um **campo
/// faltando** ou **valor desconhecido** (enum variante que o parser
/// não conhece), o resultado da merge é o **fallback deny**
/// (variante mais restritiva do enum / `false` pro bool) **para esse
/// eixo**, nunca herdar o valor permissivo de outro layer.
///
/// **Por que fail-closed:** mesma família da `WorkerToolDispatcher`
/// (PR #25, allowed_paths fail-closed) e do `permission_loader`
/// cacheando parse (PR 2, decisão de 2026-08-06). Default-deny
/// significa "ferramenta perigosa nasce desligada; ligar é decisão
/// consciente" (spec `tool-permission-model.md` §"Invariantes").
/// Herdar `network: true` de um layer que **não cita** `network`
/// seria assumir que "ausente = permitido" — e essa suposição é o
/// que produz brechas silenciosas.
///
/// **Quando o caller quer herdar valor explícito ausente:** ele
/// precisa passar um `PermissionSet::default()` no slot ausente
/// (`network: false`, `file_read: None`, etc) — o deny explícito é
/// o "preenchi" da ausência. `merge` é a operação, não a
/// interpretação de ausência.
impl PermissionSet {
    /// `self.merge(other)` = `self ∩ other` (mais restritivo vence,
    /// fail-closed). `merge` é comutativa e associativa — pode ser
    /// encadeada: `user.merge(project).merge(assistant) ==
    /// user.merge(assistant).merge(project)`.
    ///
    /// **Inverso da `is_subset_of`:** `a.is_subset_of(b)` ⇒
    /// `a.merge(b) == a` (o `a` é o mais restritivo, merge não
    /// restringe mais). Mesma família de invariante que o
    /// `PermissionSet::is_subset_of` da Fase 3 Etapa 3, agora
    /// exercitada no caminho de produção (Etapa 3 PR 2 fecha
    /// essa peça do `tool-permission-model.md §"Hierarquia"`).
    #[must_use]
    pub fn merge(&self, other: &Self) -> Self {
        // Booleans: AND (todos true ⇒ true; um false ⇒ false).
        // Fail-closed: um dos 2 ser false **nega** o eixo.
        let file_create = self.file_create && other.file_create;
        let file_modify = self.file_modify && other.file_modify;
        let file_delete = self.file_delete && other.file_delete;
        let web_browse = self.web_browse && other.web_browse;
        let web_download = self.web_download && other.web_download;
        let network = self.network && other.network;
        // network_allowlist: interseção (mesma regra fail-closed
        // dos demais eixos — um host só sobrevive ao merge se
        // **todos** os layers o citam explicitamente; comparação
        // por string exata, não pelo match por sufixo do
        // `NetworkAllowlist::contains` do proxy — essa nuance fica
        // pro ponto de decisão do proxy, não pro merge de layers).
        let network_allowlist: Vec<String> = self
            .network_allowlist
            .iter()
            .filter(|h| other.network_allowlist.contains(h))
            .cloned()
            .collect();
        let screen_capture = self.screen_capture && other.screen_capture;
        let input_control = self.input_control && other.input_control;
        let credentials = self.credentials && other.credentials;
        let destructive_ops = self.destructive_ops && other.destructive_ops;

        // file_read: min da ordem `None < WorkspaceOnly <
        // WorkspacePlusApproved`. Fail-closed: se um dos 2 for
        // None, effective = None. Nunca herda o mais permissivo.
        let file_read = match (self.file_read, other.file_read) {
            (FileReadPermission::None, _) | (_, FileReadPermission::None) => {
                FileReadPermission::None
            }
            (FileReadPermission::WorkspaceOnly, FileReadPermission::WorkspaceOnly) => {
                FileReadPermission::WorkspaceOnly
            }
            (FileReadPermission::WorkspaceOnly, FileReadPermission::WorkspacePlusApproved)
            | (FileReadPermission::WorkspacePlusApproved, FileReadPermission::WorkspaceOnly) => {
                FileReadPermission::WorkspaceOnly
            }
            (
                FileReadPermission::WorkspacePlusApproved,
                FileReadPermission::WorkspacePlusApproved,
            ) => FileReadPermission::WorkspacePlusApproved,
        };

        // Enums com ordem: `min`. Fail-closed: variantes são
        // `None < ... < Unrestricted`, então `min` é a mais
        // restritiva. `min(None, Unrestricted) = None`.
        let terminal = std::cmp::min(self.terminal, other.terminal);
        let python = std::cmp::min(self.python, other.python);
        let node = std::cmp::min(self.node, other.node);
        let git = std::cmp::min(self.git, other.git);
        // `github_repos`: interseção nos três eixos (ADR-0049 §D2).
        // Repositório que só existe de um lado sai; branch e operação
        // idem. Regra que sobra sem branch ou sem operação é
        // descartada — ela não autorizaria nada, e mantê-la produziria
        // a mensagem de erro errada ("operação negada" em vez de
        // "fora da matriz").
        let github_repos: Vec<RegraGithubPerfil> = self
            .github_repos
            .iter()
            .filter_map(|minha| {
                let dela = other.github_repos.iter().find(|r| r.repo == minha.repo)?;
                let branches: Vec<String> = minha
                    .branches
                    .iter()
                    .filter(|b| dela.branches.contains(b))
                    .cloned()
                    .collect();
                let operacoes: Vec<String> = minha
                    .operacoes
                    .iter()
                    .filter(|o| dela.operacoes.contains(o))
                    .cloned()
                    .collect();
                if branches.is_empty() || operacoes.is_empty() {
                    return None;
                }
                Some(RegraGithubPerfil {
                    repo: minha.repo.clone(),
                    branches,
                    operacoes,
                })
            })
            .collect();
        let memory = std::cmp::min(self.memory, other.memory);
        let documents = std::cmp::min(self.documents, other.documents);

        Self {
            file_read,
            file_create,
            file_modify,
            file_delete,
            terminal,
            python,
            node,
            git,
            github_repos,
            web_browse,
            web_download,
            network,
            network_allowlist,
            screen_capture,
            input_control,
            memory,
            credentials,
            documents,
            destructive_ops,
        }
    }

    /// `self.merge3(a, b)` = `self.merge(a).merge(b)`. Atalho
    /// pra deixar o `permission_loader` mais legível quando
    /// mergear 3 camadas (user ∩ project ∩ assistant).
    #[must_use]
    pub fn merge3(&self, a: &Self, b: &Self) -> Self {
        self.merge(a).merge(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_deny() {
        let p = PermissionSet::default();
        assert_eq!(p.file_read, FileReadPermission::None);
        assert!(!p.file_create);
        assert!(!p.file_modify);
        assert!(!p.file_delete);
        assert_eq!(p.terminal, TerminalMode::None);
        assert_eq!(p.python, RuntimePermission::None);
        assert_eq!(p.node, RuntimePermission::None);
        assert_eq!(p.git, GitPermission::None);
        assert!(p.github_repos.is_empty(), "default nega todo repositório");
        assert!(!p.web_browse);
        assert!(!p.web_download);
        assert!(!p.network);
        assert!(!p.screen_capture);
        assert!(!p.input_control);
        assert_eq!(p.memory, MemoryPermission::None);
        assert!(!p.credentials);
        assert_eq!(p.documents, DocumentPermission::None);
        assert!(!p.destructive_ops);
    }

    #[test]
    fn allow_all_is_superset_of_default() {
        let default = PermissionSet::default();
        let all = PermissionSet::allow_all();
        assert!(all.is_subset_of(&all));
        assert!(default.is_subset_of(&all));
    }

    #[test]
    fn is_subset_of_self_is_true() {
        let p = PermissionSet::allow_all();
        assert!(p.is_subset_of(&p));
        let d = PermissionSet::default();
        assert!(d.is_subset_of(&d));
    }

    #[test]
    fn subagent_network_allowlist_superset_of_parent_is_rejected() {
        let sub = PermissionSet {
            network_allowlist: vec!["pypi.org".to_string(), "evil.example".to_string()],
            ..Default::default()
        };
        let parent = PermissionSet {
            network_allowlist: vec!["pypi.org".to_string()],
            ..Default::default()
        };
        assert!(!sub.is_subset_of(&parent));
    }

    #[test]
    fn subagent_network_allowlist_subset_of_parent_is_accepted() {
        let sub = PermissionSet {
            network_allowlist: vec!["pypi.org".to_string()],
            ..Default::default()
        };
        let parent = PermissionSet {
            network_allowlist: vec!["pypi.org".to_string(), "registry.npmjs.org".to_string()],
            ..Default::default()
        };
        assert!(sub.is_subset_of(&parent));
    }

    #[test]
    fn merge_network_allowlist_intersects() {
        let lhs = PermissionSet {
            network_allowlist: vec!["pypi.org".to_string(), "registry.npmjs.org".to_string()],
            ..Default::default()
        };
        let rhs = PermissionSet {
            network_allowlist: vec!["pypi.org".to_string(), "github.com".to_string()],
            ..Default::default()
        };
        let merged = lhs.merge(&rhs);
        assert_eq!(merged.network_allowlist, vec!["pypi.org".to_string()]);
    }

    #[test]
    fn merge_network_allowlist_layer_absent_yields_empty() {
        // Layer ausente (default = Vec::new()) → interseção vazia,
        // mesma regra fail-closed dos demais eixos.
        let lhs = PermissionSet {
            network_allowlist: vec!["pypi.org".to_string()],
            ..Default::default()
        };
        let rhs = PermissionSet::default();
        let merged = lhs.merge(&rhs);
        assert!(merged.network_allowlist.is_empty());
    }

    #[test]
    fn subagent_cannot_have_more_than_parent_in_file_read() {
        // Sub tem WorkspacePlusApproved, pai tem WorkspaceOnly → não é subset.
        let sub = PermissionSet {
            file_read: FileReadPermission::WorkspacePlusApproved,
            ..Default::default()
        };
        let parent = PermissionSet {
            file_read: FileReadPermission::WorkspaceOnly,
            ..Default::default()
        };
        assert!(!sub.is_subset_of(&parent));
    }

    #[test]
    fn subagent_with_none_always_subset() {
        // Sub com None é subset de qualquer pai.
        let sub = PermissionSet::default();
        let mut parent = PermissionSet {
            file_read: FileReadPermission::WorkspaceOnly,
            ..Default::default()
        };
        assert!(sub.is_subset_of(&parent));
        parent.file_read = FileReadPermission::WorkspacePlusApproved;
        assert!(sub.is_subset_of(&parent));
    }

    #[test]
    fn subagent_cannot_have_bool_true_when_parent_false() {
        let sub = PermissionSet {
            network: true,
            ..Default::default()
        };
        let parent = PermissionSet::default();
        assert!(!sub.is_subset_of(&parent));
    }

    #[test]
    fn subagent_can_have_bool_false_when_parent_true() {
        let sub = PermissionSet {
            network: false,
            ..Default::default()
        };
        let parent = PermissionSet {
            network: true,
            ..Default::default()
        };
        assert!(sub.is_subset_of(&parent));
    }

    #[test]
    fn subagent_cannot_have_higher_runtime_permission() {
        let sub = PermissionSet {
            python: RuntimePermission::Sandboxed,
            ..Default::default()
        };
        let parent = PermissionSet {
            python: RuntimePermission::ReadOnly,
            ..Default::default()
        };
        assert!(!sub.is_subset_of(&parent));
    }

    #[test]
    fn subagent_can_have_lower_runtime_permission() {
        let sub = PermissionSet {
            python: RuntimePermission::ReadOnly,
            ..Default::default()
        };
        let parent = PermissionSet {
            python: RuntimePermission::Sandboxed,
            ..Default::default()
        };
        assert!(sub.is_subset_of(&parent));
    }

    #[test]
    fn random_pair_invariant_holds() {
        // Teste de invariante: para pares (sub, parent) onde sub é
        // construído com bits aleatórios, se `is_subset_of` retorna
        // `true`, então `sub` é "seguro" pro pai. Se retorna
        // `false`, é violação. O teste abaixo itera pares válidos
        // (sub ⊆ parent) e confirma que o subset detection
        // funciona; depois itera pares inválidos e confirma que
        // retorna `false`.
        let all = PermissionSet::allow_all();
        let default = PermissionSet::default();

        // Pares válidos
        assert!(default.is_subset_of(&all));
        assert!(default.is_subset_of(&default));
        assert!(all.is_subset_of(&all));

        // Pares inválidos: sub tem coisas que o pai não tem
        let sub_only_network = PermissionSet {
            network: true,
            ..Default::default()
        };
        assert!(!sub_only_network.is_subset_of(&default));

        let sub_workspace_plus = PermissionSet {
            file_read: FileReadPermission::WorkspacePlusApproved,
            ..Default::default()
        };
        assert!(!sub_workspace_plus.is_subset_of(&default));
    }

    #[test]
    fn file_read_permission_display() {
        assert_eq!(FileReadPermission::None.to_string(), "none");
        assert_eq!(
            FileReadPermission::WorkspaceOnly.to_string(),
            "workspace_only"
        );
        assert_eq!(
            FileReadPermission::WorkspacePlusApproved.to_string(),
            "workspace_plus_approved"
        );
    }

    // -------- `PermissionSet::merge` (Etapa 3 da Fase 6, PR 2) --------

    /// **Princípio fail-closed:** `merge` é o **min** de cada eixo.
    /// Teste por eixo: bool (AND), enums (min), file_read (matriz
    /// `None < WorkspaceOnly < WorkspacePlusApproved`).

    #[test]
    fn merge_bool_axis_more_restrictive_wins() {
        // network: self=true, other=false → false (nega)
        let lhs = PermissionSet {
            network: true,
            ..PermissionSet::default()
        };
        let rhs = PermissionSet::default();
        assert!(
            !lhs.merge(&rhs).network,
            "false em rhs nega mesmo se lhs=true"
        );
        assert!(
            !rhs.merge(&lhs).network,
            "comutativa: false em lhs nega mesmo se rhs=true"
        );
    }

    #[test]
    fn merge_bool_axis_allows_only_if_all_true() {
        let lhs = PermissionSet {
            network: true,
            ..PermissionSet::default()
        };
        let rhs = PermissionSet {
            network: true,
            ..PermissionSet::default()
        };
        assert!(lhs.merge(&rhs).network);
    }

    #[test]
    fn merge_enum_axis_more_restrictive_wins() {
        // python: lhs=ReadOnly, rhs=Unrestricted → ReadOnly (mais restritivo)
        let lhs = PermissionSet {
            python: RuntimePermission::ReadOnly,
            ..PermissionSet::default()
        };
        let rhs = PermissionSet {
            python: RuntimePermission::Unrestricted,
            ..PermissionSet::default()
        };
        assert_eq!(lhs.merge(&rhs).python, RuntimePermission::ReadOnly);
        assert_eq!(rhs.merge(&lhs).python, RuntimePermission::ReadOnly);
    }

    #[test]
    fn merge_enum_axis_none_dominates() {
        // python: lhs=None, rhs=Unrestricted → None (nunca herda Unrestricted)
        let lhs = PermissionSet::default(); // python = None
        let rhs = PermissionSet {
            python: RuntimePermission::Unrestricted,
            ..PermissionSet::default()
        };
        assert_eq!(lhs.merge(&rhs).python, RuntimePermission::None);
    }

    #[test]
    fn merge_file_read_none_dominates() {
        // file_read: lhs=None, rhs=WorkspacePlusApproved → None
        let lhs = PermissionSet::default(); // file_read = None
        let rhs = PermissionSet {
            file_read: FileReadPermission::WorkspacePlusApproved,
            ..PermissionSet::default()
        };
        assert_eq!(lhs.merge(&rhs).file_read, FileReadPermission::None);
    }

    #[test]
    fn merge_file_read_workspace_only_workspace_plus_intersects_to_workspace_only() {
        // WorkspaceOnly ∩ WorkspacePlusApproved = WorkspaceOnly
        let lhs = PermissionSet {
            file_read: FileReadPermission::WorkspaceOnly,
            ..PermissionSet::default()
        };
        let rhs = PermissionSet {
            file_read: FileReadPermission::WorkspacePlusApproved,
            ..PermissionSet::default()
        };
        assert_eq!(
            lhs.merge(&rhs).file_read,
            FileReadPermission::WorkspaceOnly,
            "fail-closed: WorkspaceOnly ∩ WorkspacePlusApproved = WorkspaceOnly (mais restritivo)"
        );
    }

    #[test]
    fn merge_is_commutative() {
        let lhs = PermissionSet {
            network: true,
            python: RuntimePermission::Sandboxed,
            file_read: FileReadPermission::WorkspaceOnly,
            ..PermissionSet::default()
        };
        let rhs = PermissionSet {
            network: false,
            python: RuntimePermission::Unrestricted,
            file_read: FileReadPermission::WorkspacePlusApproved,
            ..PermissionSet::default()
        };
        assert_eq!(lhs.merge(&rhs), rhs.merge(&lhs));
    }

    #[test]
    fn merge_is_associative() {
        let a = PermissionSet {
            network: true,
            ..PermissionSet::default()
        };
        let b = PermissionSet {
            python: RuntimePermission::Sandboxed,
            ..PermissionSet::default()
        };
        let c = PermissionSet {
            file_read: FileReadPermission::WorkspaceOnly,
            ..PermissionSet::default()
        };
        assert_eq!(a.merge(&b).merge(&c), a.merge(&b.merge(&c)));
    }

    #[test]
    fn merge_with_default_is_self_when_self_more_restrictive() {
        // Se self já é o mais restritivo, merge com default =
        // self (não restringe mais). Inverso da is_subset_of.
        let restrictive = PermissionSet {
            file_read: FileReadPermission::None,
            network: false,
            python: RuntimePermission::None,
            ..PermissionSet::default()
        };
        let default = PermissionSet::default();
        assert_eq!(restrictive.merge(&default), restrictive);
    }

    #[test]
    fn merge3_intersects_three_layers() {
        // Triplo: user ∩ project ∩ assistant
        // user = tudo permissivo, project = nega network, assistant = nega web_browse
        // effective: network=false (nega), web_browse=false (nega), outros=true
        let user = PermissionSet {
            network: true,
            web_browse: true,
            file_read: FileReadPermission::WorkspacePlusApproved,
            ..PermissionSet::default()
        };
        let project = PermissionSet {
            network: false,
            web_browse: true,
            file_read: FileReadPermission::WorkspaceOnly,
            ..PermissionSet::default()
        };
        let assistant = PermissionSet {
            network: true,
            web_browse: false,
            file_read: FileReadPermission::WorkspaceOnly,
            ..PermissionSet::default()
        };
        let effective = user.merge3(&project, &assistant);
        assert!(!effective.network, "project nega network → effective nega");
        assert!(
            !effective.web_browse,
            "assistant nega web_browse → effective nega"
        );
        assert_eq!(
            effective.file_read,
            FileReadPermission::WorkspaceOnly,
            "WorkspaceOnly ∩ WorkspaceOnly = WorkspaceOnly"
        );
    }

    #[test]
    fn merge_preserves_is_subset_of_relation() {
        // Para cada par (a, b) onde a.is_subset_of(b), vale
        // a.merge(b) == a. Inverso: se a.merge(b) == a, então
        // a é o mais restritivo.
        let a = PermissionSet {
            file_read: FileReadPermission::None,
            network: false,
            ..PermissionSet::default()
        };
        let b = PermissionSet {
            file_read: FileReadPermission::WorkspacePlusApproved,
            network: true,
            ..PermissionSet::default()
        };
        assert!(a.is_subset_of(&b), "precondição");
        assert_eq!(
            a.merge(&b),
            a,
            "merge com mais permissivo = o mais restritivo"
        );
    }

    #[test]
    fn merge_fail_closed_unknown_field_does_not_leak_permissive_default() {
        // Cenário crítico do PR 2: se um dos profiles não cita
        // um campo, o effective **não herda** o valor permissivo
        // de outro layer. Aqui, profile A (parsed de TOML) tem
        // `network = false` (explícito), profile B tem
        // `network` ausente (= `default()` = false). Effective
        // continua false. **Nenhum caminho** transforma "ausente"
        // em "true".
        let a = PermissionSet {
            network: false,
            ..PermissionSet::default()
        };
        // b é exatamente default — sem o campo `network` setado
        // (todos os outros campos idem).
        let b = PermissionSet::default();
        let merged = a.merge(&b);
        assert!(!merged.network);
        // Inverso: se b dissesse network=true, merged também true.
        let b_with_network = PermissionSet {
            network: true,
            ..PermissionSet::default()
        };
        assert!(
            !a.merge(&b_with_network).network,
            "fail-closed: false em a nega mesmo se b=true"
        );
    }
}

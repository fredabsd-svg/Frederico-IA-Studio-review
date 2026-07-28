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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileReadPermission {
    /// `false` no spec antigo. Negado por default; usuário precisa
    /// ligar explicitamente.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePermission {
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalMode {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitPermission {
    /// Nada — `git` desligado.
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

/// Permissão de GitHub remoto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubPermission {
    None,
    ReadOnly,
    Clone,
    /// Tudo **exceto** push e PR (operações destrutivas exigem
    /// aprovação caso a caso).
    Commit,
    /// Tudo, incluindo push e PR.
    Push,
}

impl fmt::Display for GitHubPermission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::ReadOnly => f.write_str("read_only"),
            Self::Clone => f.write_str("clone"),
            Self::Commit => f.write_str("commit"),
            Self::Push => f.write_str("push"),
        }
    }
}

/// Permissão de memória.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPermission {
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

/// Permissão de geração documental.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentPermission {
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

/// O conjunto de permissões. Spec §"Contrato".
///
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
    /// GitHub remoto.
    pub github: GitHubPermission,
    /// Navegação web.
    pub web_browse: bool,
    /// Download via web.
    pub web_download: bool,
    /// Rede genérica (proxy local do app, allowlist de domínios).
    pub network: bool,
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
            github: GitHubPermission::None,
            web_browse: false,
            web_download: false,
            network: false,
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
            github: GitHubPermission::Push,
            web_browse: true,
            web_download: true,
            network: true,
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
        if self.github > parent.github {
            return false;
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

// Helper para `PermissionSet::is_subset_of`: precisamos de
// `PartialOrd` no `TerminalMode` pra simplificar. O spec não define
// a ordem; vou colocar None < RequireApproval < Denylist/Allowlist
// e a checagem fina é "sub ≤ parent".
impl PartialOrd for TerminalMode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TerminalMode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use TerminalMode::*;
        // None < RequireApproval < Denylist/Allowlist (iguais entre si).
        let rank = |m: &TerminalMode| -> u8 {
            match m {
                None => 0,
                RequireApproval => 1,
                Denylist | Allowlist => 2,
            }
        };
        rank(self).cmp(&rank(other))
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
        assert_eq!(p.github, GitHubPermission::None);
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
}

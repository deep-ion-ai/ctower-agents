//! Registro tipado dos métodos RPC do ambiente (`/ws` do ambiente, #228).
//!
//! Cada método tem um [`MethodSpec`]: nome, tipo TypeScript dos parâmetros e
//! do resultado, e os escopos em que vale. O despacho do `/ws` do ambiente
//! (`ct-server::ws`) recusa o que não está aqui; as listas de escopo
//! ([`CHANNEL_METHODS`], [`WEB_METHODS`]) saem daqui, não de listas à mão.
//! `ui/src/protocol/gen/methods.ts` (`DaemonMethods`, `METHOD_SCOPES`) é gerado
//! do mesmo registro pelo teste `export_ts` do `ct-server` e entra no CI de
//! drift.
//!
//! Os tipos TypeScript são expressões escritas à mão, com os nomes dos tipos
//! exportados pelo ts-rs (`EnvInfo`, `Session`...). O gerador só aceita nomes
//! que existem em `ui/src/protocol/gen`; `unknown` marca o que ainda não tem
//! tipo nomeado.

/// Escopo de um método: quem pode chamá-lo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// A UI (desktop, pelo `/ws` do ambiente).
    Ui,
    /// A UI web, pelo canal reverso do hub (`hub.env.call`).
    Web,
    /// O canal reverso do hub (`/sync/channel`), o mesmo que a web usa.
    Channel,
}

/// Um método do `/ws` do ambiente.
#[derive(Debug, Clone, Copy)]
pub struct MethodSpec {
    /// Nome do método, como o cliente o envia (`session.start`).
    pub name: &'static str,
    /// Tipo TypeScript dos parâmetros; `null` quando não há parâmetros.
    pub params: &'static str,
    /// Tipo TypeScript do resultado.
    pub result: &'static str,
    /// Escopos em que o método vale.
    pub scopes: &'static [Scope],
}

const UI: &[Scope] = &[Scope::Ui];
const UI_WEB_CHANNEL: &[Scope] = &[Scope::Ui, Scope::Web, Scope::Channel];

/// Os métodos do `/ws` do ambiente. Todo braço de despacho em `ws.rs` tem de
/// estar aqui (teste em `ws.rs`), e a UI web só chama o que tem o escopo `Web`.
pub const METHODS: &[MethodSpec] = &[
    MethodSpec { name: "env.info", params: "null", result: "EnvInfo", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "env.update", params: "{ force?: boolean | null } | null", result: "UpdateStatus", scopes: UI },
    MethodSpec { name: "env.rollback", params: "{ force?: boolean | null } | null", result: "UpdateStatus", scopes: UI },
    MethodSpec { name: "env.check_update", params: "null", result: "UpdateStatus", scopes: UI },
    MethodSpec { name: "env.config.get", params: "null", result: "ConfigItem[]", scopes: UI },
    MethodSpec { name: "env.config.set", params: "{ key: string; value: unknown }", result: "ConfigItem", scopes: UI },
    MethodSpec { name: "env.credentials.get", params: "null", result: "EnvCredential[]", scopes: UI },
    MethodSpec {
        name: "env.credentials.set",
        params: "{ owner: string; repo: string; tracker_cmd?: string | null; forge_cmd?: string | null }",
        result: "EnvCredential[]",
        scopes: UI,
    },
    MethodSpec { name: "env.statusline.install", params: "null", result: "StatuslineInstall", scopes: UI },
    MethodSpec { name: "account.get", params: "null", result: "EnvAccount", scopes: UI },
    MethodSpec { name: "projects.list", params: "{ unassigned?: boolean | null } | null", result: "Project[]", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "dashboard.get", params: "null", result: "Dashboard", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "recovery.list", params: "null", result: "RecoveryItem[]", scopes: UI_WEB_CHANNEL },
    MethodSpec {
        name: "recovery.archived",
        params: "{ since_ms?: number | null; limit?: number | null } | null",
        result: "ArchivedItem[]",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "transcript.tail",
        params: "{ session_id: string; entries?: number | null }",
        result: "TranscriptTail",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec { name: "session.usage", params: "{ session_id: string }", result: "SessionUsage | null", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "session.context", params: "{ session_id: string }", result: "ContextUsage", scopes: UI_WEB_CHANNEL },
    MethodSpec {
        name: "session.start",
        params: "{ project_id?: number | null; owner?: string | null; repo?: string | null; gh_number: number; kind?: \"epic\" | \"issue\" | null; title?: string | null; labels?: string[] | null; prompt?: string | null; autonomous?: boolean | null; override_approval?: boolean | null; cols?: number | null; rows?: number | null; model?: string | null; effort?: string | null; mode?: \"native\" | \"subagents\" | null; remote_control?: boolean | null }",
        result: "Session",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "session.start_project",
        params: "{ project_id?: number | null; owner?: string | null; repo?: string | null; prompt: string; analysis?: boolean | null; epic_plan?: number | null; cols?: number | null; rows?: number | null; model?: string | null; effort?: string | null; mode?: \"native\" | \"subagents\" | null; remote_control?: boolean | null }",
        result: "Session",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "session.start_release",
        params: "{ project_id?: number | null; owner?: string | null; repo?: string | null; prompt: string; cols?: number | null; rows?: number | null; model?: string | null; effort?: string | null; mode?: \"native\" | \"subagents\" | null; remote_control?: boolean | null }",
        result: "Session",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "session.start_gestor",
        params: "{ scope: GestorScope; prompt?: string | null; cols?: number | null; rows?: number | null; model?: string | null; effort?: string | null; mode?: \"native\" | \"subagents\" | null; remote_control?: boolean | null }",
        result: "Session",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "autonomy.align",
        params: "{ project_id?: number | null; owner?: string | null; repo?: string | null }",
        result: "AutonomyReport",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "sync.now",
        params: "{ project_id?: number | null } | null",
        result: "ProjectSync[]",
        scopes: UI,
    },
    MethodSpec {
        name: "tracker.issue_body",
        params: "{ project_id?: number | null; owner?: string | null; repo?: string | null; number: number }",
        result: "unknown",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "tracker.add_issue",
        params: "{ project_id?: number | null; owner?: string | null; repo?: string | null; key: string | number }",
        result: "TrackerIssueAdded",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec { name: "github.status", params: "null", result: "TokenStatus", scopes: UI },
    MethodSpec { name: "github.set_token", params: "{ token: string }", result: "TokenStatus", scopes: UI },
    MethodSpec { name: "github.clear_token", params: "null", result: "TokenStatus", scopes: UI },
    MethodSpec {
        name: "github.pr.comment",
        params: "{ project_id?: number | null; owner?: string | null; repo?: string | null; pr: number; body: string }",
        result: "PrComment",
        scopes: UI,
    },
    MethodSpec { name: "tree.get", params: "{ project_id: number }", result: "ProjectTree", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "docs.dir", params: "{ project_id: number; epic?: number | null }", result: "DocsDir", scopes: UI },
    MethodSpec {
        name: "file.read",
        params: "{ path: string; session_id?: string | null; project_id?: number | null }",
        result: "MdFile",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec { name: "files.shared", params: "{ session_id: string }", result: "SharedFile[]", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "docs.list", params: "{ project_id: number; epic?: number | null }", result: "DocsFiles", scopes: UI_WEB_CHANNEL },
    MethodSpec {
        name: "git.diff",
        params: "{ work_item_id: number; mode: \"branch\" | \"uncommitted\" | \"commit\"; commit?: string | null }",
        result: "DiffSummary",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "git.diff_file",
        params: "{ work_item_id: number; mode: \"branch\" | \"uncommitted\" | \"commit\"; commit?: string | null; path: string; old_path?: string | null }",
        result: "DiffPatch",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec { name: "git.commits", params: "{ work_item_id: number }", result: "BranchCommits", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "review.get", params: "{ work_item_id: number }", result: "Review", scopes: UI_WEB_CHANNEL },
    MethodSpec {
        name: "events.list",
        params: "{ work_item_id: number; before?: number | null; limit?: number | null }",
        result: "Event[]",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "review.mark_reviewed",
        params: "{ work_item_id: number; pr?: number | null; decision_ids?: number[] | null }",
        result: "Marked",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "session.resume",
        params: "{ session_id: string; prompt?: string | null; remote_control?: boolean | null }",
        result: "Session",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec { name: "session.continue", params: "{ session_id: string; text?: string | null }", result: "{ continued: boolean }", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "session.remote_control.enable", params: "{ session_id: string }", result: "RemoteControlEnable", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "session.get", params: "{ session_id: string }", result: "Session & { work_item_concluded: boolean }", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "session.archive", params: "{ session_id: string }", result: "{}", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "session.kill", params: "{ session_id: string }", result: "{}", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "session.peek", params: "{ session_id: string; bytes?: number | null }", result: "Peek", scopes: UI },
    MethodSpec { name: "session.permission.watch", params: "null", result: "PermissionRequest[]", scopes: UI },
    MethodSpec { name: "session.permission.list", params: "null", result: "PermissionRequest[]", scopes: UI },
    MethodSpec {
        name: "session.permission.respond",
        params: "{ request_id: string; decision: string; message?: string | null }",
        result: "unknown",
        scopes: UI,
    },
    MethodSpec {
        name: "shell.open",
        params: "{ work_item_id: number; cols?: number | null; rows?: number | null }",
        result: "Shell",
        scopes: UI,
    },
    MethodSpec { name: "shell.list", params: "null", result: "Shell[]", scopes: UI },
    MethodSpec { name: "shell.close", params: "{ shell_id: string }", result: "{}", scopes: UI },
    MethodSpec { name: "session.attach", params: "{ session_id: string }", result: "unknown", scopes: UI },
    MethodSpec { name: "session.detach", params: "{ session_id: string }", result: "{}", scopes: UI },
    MethodSpec { name: "session.input", params: "{ session_id: string; data: string }", result: "{}", scopes: UI },
    MethodSpec { name: "session.resize", params: "{ session_id: string; cols: number; rows: number }", result: "{}", scopes: UI },
    MethodSpec { name: "cleanup.status", params: "null", result: "CleanupStatus", scopes: UI },
    MethodSpec { name: "cleanup.plan", params: "null", result: "CleanupPlan", scopes: UI },
    MethodSpec {
        name: "cleanup.run",
        params: "{ work_item_id?: number | null; step?: string | null; dry_run?: boolean | null } | null",
        result: "CleanupRun",
        scopes: UI,
    },
    MethodSpec {
        name: "logs.tail",
        params: "{ level?: string | null; q?: string | null; since?: number | null; until?: number | null; limit?: number | null; before?: number | null }",
        result: "unknown",
        scopes: UI_WEB_CHANNEL,
    },
    MethodSpec {
        name: "logs.follow",
        params: "{ level?: string | null; q?: string | null; since?: number | null; until?: number | null }",
        result: "{}",
        scopes: UI,
    },
    MethodSpec { name: "logs.unfollow", params: "null", result: "{}", scopes: UI },
    MethodSpec { name: "hub.config.changed", params: "null", result: "{}", scopes: UI_WEB_CHANNEL },
    MethodSpec { name: "hub.sync.now", params: "null", result: "HubSyncResult", scopes: UI_WEB_CHANNEL },
];

/// O registro tem `name`? Método fora dele não é despachado.
pub fn find(name: &str) -> Option<&'static MethodSpec> {
    METHODS.iter().find(|m| m.name == name)
}

/// `name` existe no registro e vale no escopo `scope`?
pub fn allowed_in(name: &str, scope: Scope) -> bool {
    find(name).is_some_and(|m| m.scopes.contains(&scope))
}

const fn has_scope(scopes: &[Scope], want: Scope) -> bool {
    let mut i = 0;
    while i < scopes.len() {
        if scopes[i] as u8 == want as u8 {
            return true;
        }
        i += 1;
    }
    false
}

/// Quantos métodos de `methods` valem em `scope` (tamanho do array de
/// [`names_in`]).
pub const fn count_in(methods: &[MethodSpec], scope: Scope) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < methods.len() {
        if has_scope(methods[i].scopes, scope) {
            n += 1;
        }
        i += 1;
    }
    n
}

/// Os nomes de `methods` que valem em `scope`, na ordem do registro (com
/// `N` = [`count_in`]). Usado por este módulo e pelo registro do hub
/// (`cthubd::methods`), que tem a sua própria tabela.
pub const fn names_in<const N: usize>(methods: &[MethodSpec], scope: Scope) -> [&'static str; N] {
    let mut out = [""; N];
    let mut i = 0;
    let mut k = 0;
    while i < methods.len() {
        if has_scope(methods[i].scopes, scope) {
            out[k] = methods[i].name;
            k += 1;
        }
        i += 1;
    }
    out
}

/// Métodos que o canal reverso do hub aceita (`ct_http::sync`), derivados de
/// [`Scope::Channel`].
pub const CHANNEL_METHODS: &[&str] =
    &names_in::<{ count_in(METHODS, Scope::Channel) }>(METHODS, Scope::Channel);

/// Métodos que a UI web chama pelo hub, derivados de [`Scope::Web`].
pub const WEB_METHODS: &[&str] =
    &names_in::<{ count_in(METHODS, Scope::Web) }>(METHODS, Scope::Web);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn nomes_unicos() {
        let mut seen = BTreeSet::new();
        for m in METHODS {
            assert!(seen.insert(m.name), "método repetido: {}", m.name);
        }
    }

    #[test]
    fn todo_metodo_tem_escopo_de_ui() {
        for m in METHODS {
            assert!(m.scopes.contains(&Scope::Ui), "{}", m.name);
        }
    }

    /// A lista do canal e a da web saem do registro: o mesmo conjunto de
    /// escopos que a UI web usa (`ENV_METHODS` em `web.ts`, gerada em
    /// `METHOD_SCOPES.web`).
    #[test]
    fn listas_de_escopo_derivadas() {
        let channel: BTreeSet<&str> = METHODS
            .iter()
            .filter(|m| m.scopes.contains(&Scope::Channel))
            .map(|m| m.name)
            .collect();
        let set: BTreeSet<&str> = CHANNEL_METHODS.iter().copied().collect();
        assert_eq!(set, channel);
        assert_eq!(CHANNEL_METHODS.len(), channel.len());
        let web: BTreeSet<&str> = WEB_METHODS.iter().copied().collect();
        assert_eq!(web, set, "a web chama só o que o canal aceita");
    }

    #[test]
    fn fora_do_registro_nao_vale() {
        assert!(find("metodo.inexistente").is_none());
        assert!(!allowed_in("metodo.inexistente", Scope::Ui));
        assert!(!allowed_in("session.attach", Scope::Channel));
        assert!(allowed_in("session.start", Scope::Channel));
    }
}

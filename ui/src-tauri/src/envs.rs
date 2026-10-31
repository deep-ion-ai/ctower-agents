//! Ambientes (`envs.json`) e preferências da UI (`ui.json`) no diretório de
//! dados do app. O token do tipo `wsl` nunca é gravado: é lido da distro a
//! cada conexão. Nada que vai para o JavaScript carrega token.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvKind {
    Wsl,
    Remote,
}

/// Como fica em disco.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvEntry {
    pub id: String,
    pub name: String,
    pub url: String,
    pub kind: EnvKind,
    #[serde(default)]
    pub distro: Option<String>,
    /// Só no tipo `remote`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl std::fmt::Debug for EnvEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvEntry")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// Como o JavaScript vê: sem token.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnvView {
    pub id: String,
    pub name: String,
    pub url: String,
    pub kind: EnvKind,
    pub distro: Option<String>,
    pub has_token: bool,
}

impl From<&EnvEntry> for EnvView {
    fn from(e: &EnvEntry) -> Self {
        EnvView {
            id: e.id.clone(),
            name: e.name.clone(),
            url: e.url.clone(),
            kind: e.kind,
            distro: e.distro.clone(),
            has_token: e.token.as_deref().is_some_and(|t| !t.is_empty()),
        }
    }
}

/// Edição vinda da UI. Token vazio ou ausente mantém o atual.
#[derive(Debug, Clone, Deserialize)]
pub struct EnvInput {
    pub id: String,
    pub name: String,
    pub url: String,
    pub kind: EnvKind,
    #[serde(default)]
    pub distro: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Templates {
    pub issue: String,
    pub epic: String,
    /// Sessão do projeto, sem issue (ausente em ui.json antigos: usa o padrão).
    #[serde(default = "default_project_template")]
    pub project: String,
    /// Gestor de projetos (ausente em ui.json antigos: usa o padrão).
    #[serde(default = "default_gestor_template")]
    pub gestor: String,
    /// Projetos Jira + GitLab (#351; ausentes em ui.json antigos: os padrões).
    #[serde(default = "default_jira_issue_template")]
    pub jira_issue: String,
    #[serde(default = "default_jira_epic_template")]
    pub jira_epic: String,
    #[serde(default = "default_jira_project_template")]
    pub jira_project: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyPrefs {
    pub needs_human: bool,
    pub session_error: bool,
    pub env_down: bool,
    /// Ambientes (por `id`) silenciados nas Configurações › Ambiente (#315);
    /// ausente em ui.json antigos: nenhum.
    #[serde(default)]
    pub muted_envs: Vec<String>,
}

/// Modelo e modo pré-selecionados ao iniciar uma sessão (issue #94).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionDefaults {
    /// Alias do modelo; `None` = padrão do Claude Code.
    #[serde(default)]
    pub model: Option<String>,
    /// `native` ou `subagents` (validado pelo daemon).
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Pré-marca o Remote Control na modal de início (issue #197);
    /// desligado por padrão e em ui.json antigos.
    #[serde(default)]
    pub remote_control: bool,
}

impl Default for SessionDefaults {
    fn default() -> Self {
        SessionDefaults {
            model: None,
            mode: default_mode(),
            remote_control: false,
        }
    }
}

fn default_mode() -> String {
    "native".into()
}

/// Tema da UI (#290); `System` segue a aparência do sistema. O terminal é
/// sempre escuro.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    System,
    Light,
    #[default]
    Dark,
}

impl ThemePref {
    pub fn as_str(self) -> &'static str {
        match self {
            ThemePref::System => "system",
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
        }
    }
}

/// Cores de um tipo de aba (#73), em HEX `#rrggbb`. Ausente ou `null` =
/// cor do tema; quem valida e aplica é a UI.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TabColor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TabKindColors {
    #[serde(default)]
    pub terminal: TabColor,
    #[serde(default)]
    pub shell: TabColor,
    #[serde(default)]
    pub issue: TabColor,
    #[serde(default)]
    pub screen: TabColor,
}

/// Cores das abas por tema: cada tema com a sua configuração.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TabColors {
    #[serde(default)]
    pub dark: TabKindColors,
    #[serde(default)]
    pub light: TabKindColors,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    #[serde(default)]
    pub theme: ThemePref,
    /// Ausente em ui.json antigos: tudo na cor do tema.
    #[serde(default)]
    pub tabs: TabColors,
}

/// Prompt pronto do console (#425): `id` estável (o dos padrões permite o
/// "Restaurar padrão" de cada um), nome no menu e texto com as variáveis dos
/// templates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadyPrompt {
    pub id: String,
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiConfig {
    pub templates: Templates,
    /// Ausente em ui.json antigos: os padrões. Lista vazia gravada fica vazia.
    #[serde(default = "default_prompts")]
    pub prompts: Vec<ReadyPrompt>,
    pub notify: NotifyPrefs,
    /// Ausente em ui.json antigos: padrão do Claude Code, modo `native`.
    #[serde(default)]
    pub session: SessionDefaults,
    /// Ausente em ui.json antigos: tema escuro.
    #[serde(default)]
    pub appearance: Appearance,
}

// Espelham ui/src/settings/defaults.ts (spec §7.1).
const ISSUE_TEMPLATE: &str = "Trabalhe na {{kind}} #{{number}} do repositório {{owner}}/{{repo}} (leia com `gh issue view {{number}}`).
Branch de trabalho: {{branch}}. Você pode mesclar apenas em {{branch}} — nunca em {{protected}}.
Antes de começar, consulte decisões anteriores com `ledger_list_decisions`.
Registre toda decisão relevante com `ledger_record_decision`.
No corpo do PR, escreva `Closes #{{number}}`: o GitHub só fecha a issue com Closes, Fixes ou Resolves, e \"Fecha #{{number}}\" não fecha.";

const EPIC_TEMPLATE: &str = "Trabalhe no épico #{{number}} de {{owner}}/{{repo}}: uma sub-issue por vez, cada uma em PR para {{branch}}.
Nunca mescle em {{protected}}.
Consulte `ledger_list_decisions` com scope epic antes de começar.
Ao começar cada sub-issue, chame `ledger_start_sub_issue` com o número dela.
Registre toda decisão relevante com `ledger_record_decision`.
No PR do épico para a base, escreva `Closes #{{number}}` e um `Closes #<n>` por sub-issue: o GitHub só fecha com Closes, Fixes ou Resolves, e \"Fecha #n\" não fecha.";

const PROJECT_TEMPLATE: &str = "Sessão de planejamento no clone principal de {{owner}}/{{repo}}, sem issue associada.
Use-a para entender o projeto e criar issues e épicos com `gh issue create` (sub-issues ligadas ao épico).
Não faça commits nem mescle em {{protected}}: código vai em sessões de issue.
Consulte decisões anteriores com `ledger_list_decisions` e registre as relevantes com `ledger_record_decision`.";

const GESTOR_TEMPLATE: &str = "Você é o gestor de projetos do Control Tower, numa sessão sem repositório no ambiente {{env}}.
{{scope}}
Leia o hub só pelas ferramentas portfolio_*. Para ler uma issue, use o `issue_view` do projeto em portfolio_overview (`gh issue view <n> -R owner/repo` no GitHub; o jira-cli com o comando de credencial no Jira).
Mudanças de prioridade e de status: proponha com portfolio_propose, com justificativa; o usuário aprova na tela Hub.
Issues novas: proponha na conversa e crie com `gh issue create -R owner/repo` (no Jira, `jira issue create`) só depois do meu ok.
Consulte suas decisões anteriores com ledger_list_decisions e registre as relevantes com ledger_record_decision.";

// Projetos Jira + GitLab (spec do Jira e GitLab §9, #351); espelham os
// `DEFAULT_JIRA_*` de defaults.ts.
const JIRA_ISSUE_TEMPLATE: &str = "Trabalhe na {{kind}} {{key}} do Jira ({{jira_url}}/browse/{{key}}): {{title}}. Repositório GitLab {{forge_path}}.
Leia a issue com `{{jira}} issue view {{key}} --comments 20 --plain`.
Branch de trabalho: {{branch}}. Nunca mescle nem faça push em {{protected}}.
Ao terminar, abra o MR para {{base}} com `{{glab}} mr create -R {{forge_path}} -s {{branch}} -b {{base}} -t '<título>' -d '<descrição>' -y` e, logo depois, comente o link na issue com `{{jira}} issue comment add {{key}} 'MR aberto: <url do MR>' --no-input`.
Nunca mescle o MR nem mude o status da issue no Jira (nada de `jira issue move`): o merge e a transição são do usuário.
Outros comentários no Jira e issues novas, só depois do meu ok.
Siga as regras do projeto e do épico do system prompt (formato do commit e do MR, por exemplo); releia com `rules_get` e proponha mudanças com `rules_propose`.
Antes de começar, consulte decisões anteriores com `ledger_list_decisions`.
Registre toda decisão relevante com `ledger_record_decision`.";

const JIRA_EPIC_TEMPLATE: &str = "Trabalhe no épico {{key}} do Jira ({{jira_url}}/browse/{{key}}): {{title}}. Repositório GitLab {{forge_path}}.
Leia o épico com `{{jira}} issue view {{key}} --comments 20 --plain` e liste as issues dele com `{{jira}} epic list {{key}} --plain`.
Uma issue por vez, cada uma em MR para {{branch}} com `{{glab}} mr create -R {{forge_path}} -s <branch da issue> -b {{branch}} -t '<título>' -d '<descrição>' -y`; logo depois de abrir cada MR, comente o link na issue com `{{jira}} issue comment add <chave> 'MR aberto: <url do MR>' --no-input`.
Ao concluir o épico, abra o MR de {{branch}} para {{base}} do mesmo jeito e comente o link em {{key}}.
Nunca mescle MR nem faça push em {{protected}}, e não mude o status de issues no Jira (nada de `jira issue move`): o merge e a transição são do usuário.
Issue nova no épico, só depois do meu ok: `{{jira}} issue create -p {{jira_project}} -t <tipo> -s '<título>' -b '<descrição>' -P {{key}} --no-input` (o -P liga ao épico pelo Epic Link; numa sub-tarefa, `-P <chave da issue>`).
Siga as regras do projeto e do épico do system prompt; releia com `rules_get` e proponha mudanças com `rules_propose`.
Consulte `ledger_list_decisions` com scope epic antes de começar.
Registre toda decisão relevante com `ledger_record_decision`.";

const JIRA_PROJECT_TEMPLATE: &str = "Sessão de planejamento no clone principal de {{forge_path}} (GitLab), com as issues no projeto {{jira_project}} do Jira ({{jira_url}}), sem issue associada.
Use-a para entender o projeto e propor issues e épicos. Leia com `{{jira}} issue list -p {{jira_project}} --plain` e `{{jira}} issue view <chave> --comments 20 --plain`.
Crie só depois do meu ok: issue com `{{jira}} issue create -p {{jira_project}} -t <tipo> -s '<título>' -b '<descrição>' -P <chave do épico> --no-input` (o -P liga ao épico pelo Epic Link; numa sub-tarefa, `-P <chave da issue>`) e épico com `{{jira}} epic create -p {{jira_project}} -n '<nome>' -s '<título>' -b '<descrição>' --no-input`.
Comente com `{{jira}} issue comment add <chave> '<texto>' --no-input`, também só depois do meu ok. Nunca mude o status de issues (nada de `jira issue move`).
Não faça commits nem mescle em {{protected}}: código vai em sessões de issue.
Siga as regras do projeto do system prompt; releia com `rules_get` e proponha mudanças com `rules_propose`.
Consulte decisões anteriores com `ledger_list_decisions` e registre as relevantes com `ledger_record_decision`.";

fn default_project_template() -> String {
    PROJECT_TEMPLATE.into()
}

fn default_gestor_template() -> String {
    GESTOR_TEMPLATE.into()
}

fn default_jira_issue_template() -> String {
    JIRA_ISSUE_TEMPLATE.into()
}

fn default_jira_epic_template() -> String {
    JIRA_EPIC_TEMPLATE.into()
}

fn default_jira_project_template() -> String {
    JIRA_PROJECT_TEMPLATE.into()
}

// Espelham `DEFAULT_PROMPTS` de ui/src/settings/defaults.ts (#425).
const PROMPTS: [(&str, &str, &str); 5] = [
    ("continue", "Continuar", "continue"),
    (
        "tests",
        "Rodar os testes e corrigir",
        "Rode os testes do projeto (e o typecheck e o lint, se houver).
Corrija as falhas e rode de novo até ficar tudo verde; no fim, diga o que quebrou e o que você mudou.",
    ),
    (
        "ledger",
        "Registrar as decisões no ledger",
        "Revise o que foi decidido nesta sessão e registre com `ledger_record_decision` cada decisão relevante que ainda não está no ledger.
Use origin \"autonoma\" para o que você decidiu sozinho e \"orientada\" para o que eu defini.",
    ),
    (
        "pr",
        "Abrir o PR",
        "Se o trabalho estiver pronto, faça o commit do que falta, dê push na branch {{branch}} e abra o PR (ou MR) como pedem as instruções do início da sessão.
Não mescle: me mande o link.",
    ),
    (
        "summary",
        "Resumir o estado atual",
        "Resuma o estado atual do trabalho: o que já está feito, o que falta, o que está bloqueado e o próximo passo. Não mude nada agora.",
    ),
];

fn default_prompts() -> Vec<ReadyPrompt> {
    PROMPTS
        .iter()
        .map(|(id, name, text)| ReadyPrompt {
            id: (*id).into(),
            name: (*name).into(),
            text: (*text).into(),
        })
        .collect()
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            templates: Templates {
                issue: ISSUE_TEMPLATE.into(),
                epic: EPIC_TEMPLATE.into(),
                project: PROJECT_TEMPLATE.into(),
                gestor: GESTOR_TEMPLATE.into(),
                jira_issue: JIRA_ISSUE_TEMPLATE.into(),
                jira_epic: JIRA_EPIC_TEMPLATE.into(),
                jira_project: JIRA_PROJECT_TEMPLATE.into(),
            },
            prompts: default_prompts(),
            notify: NotifyPrefs {
                needs_human: true,
                session_error: true,
                env_down: true,
                muted_envs: Vec::new(),
            },
            session: SessionDefaults::default(),
            appearance: Appearance::default(),
        }
    }
}

/// Distro padrão criada pelo provisionamento (spec §13).
pub const DEFAULT_DISTRO: &str = "control-tower";

pub fn default_envs() -> Vec<EnvEntry> {
    vec![EnvEntry {
        id: "wsl".into(),
        name: "control-tower (WSL)".into(),
        url: "ws://127.0.0.1:7878/ws".into(),
        kind: EnvKind::Wsl,
        distro: Some(DEFAULT_DISTRO.into()),
        token: None,
    }]
}

/// Esquema, host e porta de uma URL `ws(s)://` (porta padrão explícita,
/// minúsculas). `None` se não for `ws://` nem `wss://`.
fn origin(url: &str) -> Option<(String, String)> {
    let url = url.trim().to_ascii_lowercase();
    let (scheme, rest, port) = match url.strip_prefix("ws://") {
        Some(r) => ("ws", r.to_string(), "80"),
        None => ("wss", url.strip_prefix("wss://")?.to_string(), "443"),
    };
    let auth = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let auth = auth.rsplit('@').next().unwrap_or_default();
    // Sem porta (e fora de um IPv6 entre colchetes): a padrão do esquema.
    let has_port = auth.rsplit_once(':').is_some_and(|(h, p)| {
        !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && !h.ends_with('[')
    });
    let auth = if has_port {
        auth.to_string()
    } else {
        format!("{auth}:{port}")
    };
    Some((scheme.into(), auth))
}

/// O token gravado só vale para o mesmo destino: com esquema, host ou porta
/// novos, a UI precisa mandar o token de novo (senão o token de um daemon
/// iria para outro).
pub fn same_origin(a: &str, b: &str) -> bool {
    matches!((origin(a), origin(b)), (Some(x), Some(y)) if x == y)
}

/// Valida e aplica as edições sobre os ambientes atuais (tokens preservados
/// quando a UI não manda um novo).
pub fn merge(current: &[EnvEntry], inputs: Vec<EnvInput>) -> Result<Vec<EnvEntry>, String> {
    let mut out: Vec<EnvEntry> = Vec::new();
    for i in inputs {
        let id = i.id.trim().to_string();
        if id.is_empty() || i.name.trim().is_empty() {
            return Err("ambiente sem id ou nome".into());
        }
        if out.iter().any(|e| e.id == id) {
            return Err(format!("id de ambiente repetido: {id}"));
        }
        if !(i.url.starts_with("ws://") || i.url.starts_with("wss://")) {
            return Err(format!("URL deve começar com ws:// ou wss://: {}", i.url));
        }
        let distro = i.distro.filter(|d| !d.trim().is_empty());
        let token = match i.kind {
            EnvKind::Wsl => {
                if distro.is_none() {
                    return Err(format!("ambiente WSL sem distro: {}", i.name));
                }
                None
            }
            EnvKind::Remote => match i.token.filter(|t| !t.is_empty()) {
                Some(t) => Some(t),
                None => current
                    .iter()
                    .find(|e| e.id == id && same_origin(&e.url, i.url.trim()))
                    .and_then(|e| e.token.clone()),
            },
        };
        out.push(EnvEntry {
            id,
            name: i.name.trim().into(),
            url: i.url.trim().into(),
            kind: i.kind,
            distro,
            token,
        });
    }
    Ok(out)
}

/// Arquivos do app.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: &Path) -> Self {
        Store { dir: dir.into() }
    }

    fn read<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Option<T> {
        let s = fs::read_to_string(self.dir.join(name)).ok()?;
        serde_json::from_str(&s).ok()
    }

    fn write<T: Serialize>(&self, name: &str, v: &T) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let tmp = self.dir.join(format!("{name}.tmp"));
        let json = serde_json::to_string_pretty(v).expect("serializa");
        fs::write(&tmp, json).map_err(|e| e.to_string())?;
        fs::rename(&tmp, self.dir.join(name)).map_err(|e| e.to_string())
    }

    /// Ausente ou ilegível: o ambiente WSL padrão.
    pub fn envs(&self) -> Vec<EnvEntry> {
        self.read("envs.json").unwrap_or_else(default_envs)
    }

    pub fn save_envs(&self, envs: &[EnvEntry]) -> Result<(), String> {
        // Defesa extra: nunca grava token de ambiente WSL.
        let clean: Vec<EnvEntry> = envs
            .iter()
            .cloned()
            .map(|mut e| {
                if e.kind == EnvKind::Wsl {
                    e.token = None;
                }
                e
            })
            .collect();
        self.write("envs.json", &clean)
    }

    /// Conexão com o hub; ausente = não configurada.
    pub fn hub(&self) -> Option<crate::hub::HubEntry> {
        self.read("hub.json")
    }

    /// `None` apaga a conexão com o hub.
    pub fn save_hub(&self, hub: Option<&crate::hub::HubEntry>) -> Result<(), String> {
        match hub {
            Some(h) => self.write("hub.json", h),
            None => match fs::remove_file(self.dir.join("hub.json")) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            },
        }
    }

    pub fn ui_config(&self) -> UiConfig {
        self.read("ui.json").unwrap_or_default()
    }

    pub fn save_ui_config(&self, cfg: &UiConfig) -> Result<(), String> {
        self.write("ui.json", cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, kind: EnvKind, token: Option<&str>) -> EnvInput {
        EnvInput {
            id: id.into(),
            name: format!("Ambiente {id}"),
            url: "ws://127.0.0.1:7878/ws".into(),
            kind,
            distro: (kind == EnvKind::Wsl).then(|| "control-tower".into()),
            token: token.map(String::from),
        }
    }

    #[test]
    fn sem_arquivo_usa_o_wsl_padrao() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        assert_eq!(s.envs(), default_envs());
        assert_eq!(s.ui_config(), UiConfig::default());
    }

    #[test]
    fn ida_e_volta_sem_token_do_wsl() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        let envs = merge(
            &[],
            vec![
                input("wsl", EnvKind::Wsl, Some("vazou")),
                input("mac", EnvKind::Remote, Some("segredo-mac")),
            ],
        )
        .unwrap();
        assert_eq!(envs[0].token, None);
        s.save_envs(&envs).unwrap();
        assert_eq!(s.envs(), envs);
        let raw = fs::read_to_string(d.path().join("envs.json")).unwrap();
        assert!(!raw.contains("vazou"));
        assert!(raw.contains("segredo-mac"), "token remoto é persistido");
        // A visão para o JS não carrega token.
        let views: Vec<EnvView> = envs.iter().map(EnvView::from).collect();
        let js = serde_json::to_string(&views).unwrap();
        assert!(!js.contains("segredo"));
        assert!(views[1].has_token && !views[0].has_token);
    }

    #[test]
    fn token_vazio_mantem_o_atual() {
        let atual = merge(&[], vec![input("mac", EnvKind::Remote, Some("t1"))]).unwrap();
        let novo = merge(&atual, vec![input("mac", EnvKind::Remote, Some(""))]).unwrap();
        assert_eq!(novo[0].token.as_deref(), Some("t1"));
        let novo = merge(&atual, vec![input("mac", EnvKind::Remote, Some("t2"))]).unwrap();
        assert_eq!(novo[0].token.as_deref(), Some("t2"));
    }

    #[test]
    fn token_nao_segue_para_outro_destino() {
        let atual = merge(&[], vec![input("mac", EnvKind::Remote, Some("t1"))]).unwrap();
        let mut mesma = input("mac", EnvKind::Remote, None);
        mesma.url = "ws://127.0.0.1:7878/outro".into();
        assert_eq!(
            merge(&atual, vec![mesma]).unwrap()[0].token.as_deref(),
            Some("t1")
        );
        for url in [
            "ws://127.0.0.1:7879/ws",
            "wss://127.0.0.1:7878/ws",
            "ws://10.0.0.2:7878/ws",
        ] {
            let mut outra = input("mac", EnvKind::Remote, None);
            outra.url = url.into();
            assert_eq!(merge(&atual, vec![outra]).unwrap()[0].token, None, "{url}");
        }
    }

    #[test]
    fn mesma_origem() {
        assert!(same_origin("ws://h/ws", "ws://H:80/x"));
        assert!(same_origin("wss://h/ws", "wss://h:443"));
        assert!(same_origin("ws://[::1]:7878/ws", "ws://[::1]:7878"));
        assert!(!same_origin("ws://[::1]/ws", "ws://[::1]:7878/ws"));
        assert!(!same_origin("ws://h:7878/ws", "ws://h:7879/ws"));
        assert!(!same_origin("ws://h/ws", "http://h/ws"));
    }

    #[test]
    fn validacao() {
        let mut url = input("a", EnvKind::Remote, None);
        url.url = "http://x".into();
        assert!(merge(&[], vec![url]).is_err());
        let mut sem_distro = input("a", EnvKind::Wsl, None);
        sem_distro.distro = Some(" ".into());
        assert!(merge(&[], vec![sem_distro]).is_err());
        let dup = vec![
            input("a", EnvKind::Remote, None),
            input("a", EnvKind::Remote, None),
        ];
        assert!(merge(&[], dup).is_err());
    }

    #[test]
    fn debug_nao_mostra_token() {
        let e = merge(&[], vec![input("mac", EnvKind::Remote, Some("segredo"))]).unwrap();
        assert!(!format!("{e:?}").contains("segredo"));
    }

    #[test]
    fn ui_json_antigo_ganha_template_de_projeto() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("ui.json"),
            r#"{"templates":{"issue":"i","epic":"e"},"notify":{"needs_human":true,"session_error":false,"env_down":true}}"#,
        )
        .unwrap();
        let cfg = Store::new(d.path()).ui_config();
        assert_eq!(cfg.templates.issue, "i");
        assert!(!cfg.notify.session_error);
        assert_eq!(cfg.templates.project, PROJECT_TEMPLATE);
        assert_eq!(cfg.templates.gestor, GESTOR_TEMPLATE);
        assert_eq!(cfg.templates.jira_issue, JIRA_ISSUE_TEMPLATE);
        assert_eq!(cfg.templates.jira_epic, JIRA_EPIC_TEMPLATE);
        assert_eq!(cfg.templates.jira_project, JIRA_PROJECT_TEMPLATE);
        assert_eq!(cfg.session, SessionDefaults::default());
        assert_eq!(cfg.appearance.theme, ThemePref::Dark);
        assert_eq!(cfg.prompts, default_prompts());
    }

    /// Prompts prontos (#425): a lista gravada volta igual, e a vazia fica vazia.
    #[test]
    fn prompts_no_ui_json() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        let mut cfg = UiConfig::default();
        cfg.prompts.truncate(1);
        cfg.prompts.push(ReadyPrompt {
            id: "meu".into(),
            name: "Meu".into(),
            text: "linha 1\nlinha 2 {{number}}".into(),
        });
        s.save_ui_config(&cfg).unwrap();
        assert_eq!(s.ui_config(), cfg);
        cfg.prompts.clear();
        s.save_ui_config(&cfg).unwrap();
        assert!(s.ui_config().prompts.is_empty());
    }

    #[test]
    fn prompts_espelham_os_da_ui() {
        let ts = include_str!("../../src/settings/defaults.ts");
        for (id, name, text) in PROMPTS {
            assert!(
                ts.contains(&format!("id: \"{id}\"")),
                "fora de defaults.ts: {id}"
            );
            assert!(
                ts.contains(&format!("name: \"{name}\"")),
                "fora de defaults.ts: {name}"
            );
            for line in text.lines() {
                assert!(ts.contains(line), "fora de defaults.ts: {line}");
            }
        }
    }

    /// ui.json anterior ao #197: o Remote Control nasce desligado.
    #[test]
    fn ui_json_sem_remote_control_fica_desligado() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("ui.json"),
            r#"{"templates":{"issue":"i","epic":"e"},"notify":{"needs_human":true,"session_error":true,"env_down":true},"session":{"model":"opus","mode":"subagents"}}"#,
        )
        .unwrap();
        let cfg = Store::new(d.path()).ui_config();
        assert_eq!(cfg.session.model.as_deref(), Some("opus"));
        assert!(!cfg.session.remote_control);
    }

    #[test]
    fn templates_espelham_os_da_ui() {
        let ts = include_str!("../../src/settings/defaults.ts");
        for t in [
            ISSUE_TEMPLATE,
            EPIC_TEMPLATE,
            PROJECT_TEMPLATE,
            GESTOR_TEMPLATE,
            JIRA_ISSUE_TEMPLATE,
            JIRA_EPIC_TEMPLATE,
            JIRA_PROJECT_TEMPLATE,
        ] {
            for line in t.lines() {
                assert!(ts.contains(line), "fora de defaults.ts: {line}");
            }
        }
    }

    #[test]
    fn templates_de_issue_e_epico_pedem_closes() {
        // "Fecha #n" não fecha a issue no GitHub (#375).
        assert!(ISSUE_TEMPLATE.contains("`Closes #{{number}}`"));
        assert!(EPIC_TEMPLATE.contains("`Closes #{{number}}`"));
        assert!(EPIC_TEMPLATE.contains("`Closes #<n>` por sub-issue"));
        // A sessão do épico marca cada sub-issue em execução (#458).
        assert!(EPIC_TEMPLATE.contains("chame `ledger_start_sub_issue`"));
    }

    /// Templates do Jira (#351): o jira-cli e o glab pelo prefixo com o
    /// comando de credencial, comentário do MR no Jira, sem transição e sem
    /// `Closes #n` (que é do GitHub).
    #[test]
    fn templates_do_jira_comentam_o_mr_sem_transicao() {
        for t in [
            JIRA_ISSUE_TEMPLATE,
            JIRA_EPIC_TEMPLATE,
            JIRA_PROJECT_TEMPLATE,
        ] {
            assert!(t.contains("{{jira}} issue"), "{t}");
            assert!(t.contains("nada de `jira issue move`"), "{t}");
            assert!(!t.contains("Closes"), "{t}");
            assert!(!t.contains("gh "), "{t}");
        }
        assert!(JIRA_ISSUE_TEMPLATE
            .contains("{{jira}} issue comment add {{key}} 'MR aberto: <url do MR>'"));
        assert!(JIRA_ISSUE_TEMPLATE
            .contains("{{glab}} mr create -R {{forge_path}} -s {{branch}} -b {{base}}"));
        assert!(JIRA_PROJECT_TEMPLATE.contains("-P <chave do épico> --no-input"));
    }

    #[test]
    fn cores_das_abas_no_ui_json() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        // ui.json antigo, sem `tabs`: tudo vazio.
        assert_eq!(s.ui_config().appearance.tabs, TabColors::default());
        let mut cfg = UiConfig::default();
        cfg.appearance.tabs.dark.terminal = TabColor {
            fg: Some("#7fd4bc".into()),
            bg: Some("#13241f".into()),
        };
        cfg.appearance.tabs.light.issue.fg = Some("#1a7a5e".into());
        s.save_ui_config(&cfg).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(d.path().join("ui.json")).unwrap()).unwrap();
        assert_eq!(
            raw["appearance"]["tabs"]["dark"]["terminal"]["fg"],
            "#7fd4bc"
        );
        assert_eq!(raw["appearance"]["tabs"]["light"]["issue"]["fg"], "#1a7a5e");
        assert!(raw["appearance"]["tabs"]["light"]["issue"]
            .get("bg")
            .is_none());
        assert_eq!(s.ui_config().appearance.tabs, cfg.appearance.tabs);
        // `null` vale como ausente.
        let t: TabColor = serde_json::from_str(r#"{"fg":null}"#).unwrap();
        assert_eq!(t, TabColor::default());
    }

    #[test]
    fn tema_no_ui_json() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        let mut cfg = UiConfig::default();
        cfg.appearance.theme = ThemePref::System;
        s.save_ui_config(&cfg).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(d.path().join("ui.json")).unwrap()).unwrap();
        assert_eq!(raw["appearance"]["theme"], "system");
        assert_eq!(s.ui_config().appearance.theme, ThemePref::System);
    }

    #[test]
    fn hub_ida_e_volta_e_remocao() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        assert_eq!(s.hub(), None);
        let h = crate::hub::HubEntry {
            url: "ws://h:7879/ws".into(),
            token: Some("t".into()),
        };
        s.save_hub(Some(&h)).unwrap();
        assert_eq!(s.hub(), Some(h));
        s.save_hub(None).unwrap();
        assert_eq!(s.hub(), None);
        s.save_hub(None).unwrap();
    }

    #[test]
    fn preferencias_ida_e_volta() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        let mut cfg = UiConfig::default();
        cfg.notify.env_down = false;
        cfg.templates.issue = "x {{number}}".into();
        cfg.session = SessionDefaults {
            model: Some("opus".into()),
            mode: "subagents".into(),
            remote_control: true,
        };
        s.save_ui_config(&cfg).unwrap();
        assert_eq!(s.ui_config(), cfg);
    }
}

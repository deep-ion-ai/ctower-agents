//! Secrets e variáveis do hub no processo do agente (issue #166, épico
//! #171). No spawn e no resume, o `ctowerd` lê o cache cifrado do ambiente
//! (`<data>/secrets-cache.age`, entregue no pull da configuração, #184),
//! resolve as entradas globais e as do projeto da sessão (`owner/repo`; por
//! nome, a do projeto sobrepõe a global) e as passa como variáveis de
//! ambiente do processo, ao lado das `CTOWER_*`.
//!
//! - os valores só vão para o ambiente do processo: não entram em
//!   `session.launch_cmd`, nos eventos do ledger, nos arquivos da sessão nem
//!   nos logs (que citam só nomes);
//! - nome reservado ([`ct_secrets::is_reserved_name`]: `CTOWER_*`, `PATH`,
//!   `HOME`, `LC_*`, `LD_*`...) que esteja no cache (o hub já recusa
//!   gravar) é ignorado com aviso; de qualquer modo, as `CTOWER_*` do
//!   daemon vêm depois e sempre vencem;
//! - sem cache (ambiente sem hub ou sem entrega ainda): sessão sem secrets,
//!   sem aviso;
//! - cache ilegível (chave trocada à mão, arquivo corrompido): a sessão
//!   sobe sem secrets, com aviso no log e alerta `secrets_unavailable` para
//!   a UI. Recusar o spawn travaria o trabalho por um problema que o
//!   próximo pull resolve (o hub reentrega), e a sessão pode ser retomada
//!   depois;
//! - vale o que estiver no cache no momento do spawn: mudar um secret não
//!   afeta sessões em andamento, só o próximo spawn ou resume depois do
//!   pull que atualizou o cache;
//! - hub fora do ar: vale o último cache. Se a última sincronização bem
//!   sucedida passou de `secrets_max_age_hours` (padrão 24h), o spawn segue
//!   com o cache e avisa no log e com o alerta `secrets_stale` (a idade
//!   também aparece em `env.info.hub_config.stale`).
//!
//! ## Mascaramento (#167)
//!
//! O agente pode imprimir um secret (`env`, `echo $TOKEN`) e o valor
//! acabaria no scrollback, na UI e no backup do transcript. Não protege
//! contra o próprio agente, que tem o valor no ambiente: só reduz a
//! exposição.
//!
//! - entram no conjunto da sessão só os **secrets** efetivamente injetados
//!   (depois de filtrar os reservados) com pelo menos
//!   [`ct_secrets::MIN_MASK_LEN`] bytes; variáveis não são mascaradas;
//! - na saída do PTY, cada ocorrência vira `***NOME***` antes do scrollback
//!   e do broadcast (filtro em fluxo do `ct-pty`, [`ct_pty::MaskSet`]);
//! - no backup do transcript, a cópia passa pelo mesmo conjunto, mais a
//!   forma escapada em JSON de cada valor (o transcript é JSONL); o arquivo
//!   original do Claude não é tocado. Ao retomar de um backup, o transcript
//!   restaurado traz `***NOME***` no lugar do valor;
//! - o conjunto fica só em memória no daemon, associado à sessão (nunca é
//!   persistido), e sai na retomada ou no arquivamento. Se o backup
//!   acontece sem ele (daemon reiniciado), o conjunto é recalculado do
//!   cache atual pelo projeto da sessão; se o cache mudou nesse meio
//!   tempo, valem os valores atuais;
//! - no resume, a nova sessão ganha o conjunto com os valores atuais do
//!   cache (o que foi injetado no processo retomado). Valores de uma
//!   injeção anterior que mudaram no hub não são mais mascarados.
//!
//! ## Sessão do gestor (#219)
//!
//! A sessão do gestor de projetos (projeto sentinela `_ctower/gestor`) não
//! pertence a um repositório: recebe só as entradas **globais**, nunca as
//! de algum projeto (`resolve(None)`). O mascaramento segue a mesma regra.

use crate::Sessions;
use ct_core::ProjectKind;
use ct_ledger::{Project, Session};
use ct_pty::{MaskSet, Zeroizing};
use ct_secrets::{is_reserved_name, EnvCache, EnvSecrets, Kind, MIN_MASK_LEN};
use serde_json::json;

/// Valores mascarados de uma sessão: na saída do PTY e no backup do
/// transcript (que também cobre a forma escapada em JSON).
#[derive(Clone, Default)]
pub(crate) struct SessionMask {
    pub pty: MaskSet,
    pub transcript: MaskSet,
}

impl SessionMask {
    /// `secrets`: (nome, valor) dos secrets injetados (sem variáveis).
    fn new(secrets: &[(&str, &str)]) -> Self {
        let long = || {
            secrets
                .iter()
                .filter(|(_, v)| v.len() >= MIN_MASK_LEN)
                .map(|(n, v)| (n.to_string(), *v))
        };
        let pty = MaskSet::new(long().map(|(n, v)| (n, Zeroizing::new(v.as_bytes().to_vec()))));
        let transcript = MaskSet::new(long().flat_map(|(n, v)| {
            let raw = Zeroizing::new(v.as_bytes().to_vec());
            let json = Zeroizing::new(serde_json::to_string(v).unwrap_or_default());
            let escaped = json.trim_matches('"');
            let mut out = vec![(n.clone(), raw)];
            if escaped != v {
                out.push((n, Zeroizing::new(escaped.as_bytes().to_vec())));
            }
            out
        }));
        SessionMask { pty, transcript }
    }
}

/// Variáveis de ambiente de uma sessão e o mascaramento dos seus secrets.
pub(crate) struct SessionSecrets {
    pub env: Vec<(String, String)>,
    pub mask: SessionMask,
}

impl Sessions {
    /// Chave e cache de secrets deste ambiente.
    pub fn env_cache(&self) -> EnvCache {
        let dirs = &self.config().dirs;
        EnvCache::new(dirs.env_key_file(), dirs.secrets_cache_file())
    }

    /// Idade (ms) da configuração do hub no ambiente: desde a última
    /// sincronização bem-sucedida (`config_pulled_at`; um pull sem mudança
    /// também prova que o cache está em dia). Sem pull registrado, a data
    /// de gravação do cache. `None` = nem uma coisa nem outra.
    pub fn config_age_ms(&self) -> Option<i64> {
        let pulled = self.ledger().hub_config_pulled_at().ok().flatten();
        let at = pulled.or_else(|| {
            let modified = std::fs::metadata(self.config().dirs.secrets_cache_file())
                .and_then(|m| m.modified())
                .ok()?;
            let ms = modified
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_millis();
            i64::try_from(ms).ok()
        })?;
        Some((ct_core::now_ms() - at).max(0))
    }

    /// Variáveis de ambiente que os secrets e variáveis do hub dão à sessão
    /// (`owner/repo` do projeto) e o mascaramento dos secrets. Nunca falha:
    /// problemas viram aviso e a sessão sobe sem (parte dos) secrets.
    pub(crate) fn session_secrets(&self, project: &Project, session_id: &str) -> SessionSecrets {
        let none = || SessionSecrets {
            env: Vec::new(),
            mask: SessionMask::default(),
        };
        let secrets = match self.env_cache().load() {
            Ok(Some(s)) => s,
            Ok(None) => return none(),
            Err(e) => {
                // `SecretsError` nunca carrega valores: só nomes e caminhos.
                tracing::warn!(
                    "secrets: cache do ambiente ilegível ({e}); sessão {session_id} sobe sem secrets"
                );
                self.notify(
                    "alert",
                    json!({ "kind": "secrets_unavailable", "session_id": session_id,
                            "error": e.to_string() }),
                );
                return none();
            }
        };
        self.warn_if_stale(session_id);
        let env = injected(&secrets, project, session_id);
        let mask = mask_of(&secrets, project);
        SessionSecrets { env, mask }
    }

    /// Guarda (só em memória) o mascaramento de uma sessão.
    pub(crate) fn set_mask(&self, session_id: &str, mask: SessionMask) {
        let mut masks = self.inner.masks.lock().unwrap();
        if mask.pty.is_empty() {
            masks.remove(session_id);
        } else {
            masks.insert(session_id.to_string(), mask);
        }
    }

    /// Esquece o mascaramento de uma sessão (retomada ou arquivada).
    pub(crate) fn forget_mask(&self, session_id: &str) {
        self.inner.masks.lock().unwrap().remove(session_id);
    }

    /// Mascaramento do backup do transcript da sessão: o guardado no spawn
    /// ou, sem ele (daemon reiniciado), recalculado do cache atual pelo
    /// projeto da sessão. Cache ausente ou ilegível: sem mascaramento (não
    /// há valores conhecidos a esconder).
    pub(crate) fn transcript_mask(&self, s: &Session) -> MaskSet {
        if let Some(m) = self.inner.masks.lock().unwrap().get(&s.id) {
            return m.transcript.clone();
        }
        let project = {
            let l = self.ledger();
            l.get_work_item(s.work_item_id)
                .and_then(|wi| l.get_project(wi.project_id))
        };
        let Ok(project) = project else {
            return MaskSet::default();
        };
        let secrets = match self.env_cache().load() {
            Ok(Some(secrets)) => secrets,
            Ok(None) => return MaskSet::default(),
            Err(e) => {
                tracing::warn!(
                    "secrets: cache do ambiente ilegível ({e}); backup da sessão {} sem mascaramento",
                    s.id
                );
                return MaskSet::default();
            }
        };
        let mask = mask_of(&secrets, &project);
        let transcript = mask.transcript.clone();
        self.set_mask(&s.id, mask);
        transcript
    }

    /// Configuração do hub mais velha que o limite: a sessão usa o cache
    /// assim mesmo, com aviso.
    fn warn_if_stale(&self, session_id: &str) {
        let Some(max) = self.config().secrets_max_age else {
            return;
        };
        let Some(age) = self.config_age_ms() else {
            return;
        };
        let max_ms = i64::try_from(max.as_millis()).unwrap_or(i64::MAX);
        if age <= max_ms {
            return;
        }
        let msg = format!(
            "configuração do hub sem sincronizar há {} (limite {}); a sessão usa os secrets do cache",
            hours(age),
            hours(max_ms)
        );
        tracing::warn!("secrets: {msg} (sessão {session_id})");
        self.notify(
            "alert",
            json!({ "kind": "secrets_stale", "session_id": session_id, "error": msg,
                    "age_ms": age, "max_age_ms": max_ms }),
        );
    }
}

/// (nome, valor) das entradas que vão para o ambiente da sessão: globais e
/// do projeto, sem os nomes reservados (com aviso).
fn injected(secrets: &EnvSecrets, project: &Project, session_id: &str) -> Vec<(String, String)> {
    let key = scope_of(project);
    let mut env = Vec::new();
    for (name, entry) in secrets.resolve(key.as_deref()) {
        if is_reserved_name(&name) {
            tracing::warn!(
                "secrets: {name} é um nome reservado do ambiente; ignorado na sessão {session_id}"
            );
            continue;
        }
        env.push((name, entry.value.expose().to_string()));
    }
    env
}

/// Projeto (`owner/repo`) cujas entradas a sessão recebe além das globais.
/// `None` no gestor, que não está preso a um projeto: só as globais.
pub(crate) fn scope_of(project: &Project) -> Option<String> {
    (project.kind != ProjectKind::Gestor)
        .then(|| format!("{}/{}", project.gh_owner, project.gh_repo))
}

/// Mascaramento dos secrets (não das variáveis) injetados na sessão.
fn mask_of(secrets: &EnvSecrets, project: &Project) -> SessionMask {
    let key = scope_of(project);
    let resolved = secrets.resolve(key.as_deref());
    let list: Vec<(&str, &str)> = resolved
        .iter()
        .filter(|(name, e)| e.kind == Kind::Secret && !is_reserved_name(name))
        .map(|(name, e)| (name.as_str(), e.value.expose()))
        .collect();
    SessionMask::new(&list)
}

/// Duração em horas, para as mensagens.
fn hours(ms: i64) -> String {
    format!("{}h", ms / 3_600_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conjunto_ignora_valores_curtos_e_cobre_o_json_no_transcript() {
        let m = SessionMask::new(&[
            ("CURTO", "1234567"),
            ("LIMITE", "12345678"),
            ("Q", "a\"bcdefgh"),
        ]);
        assert_eq!(m.pty.apply(b"1234567 12345678"), b"1234567 ***LIMITE***");
        assert_eq!(m.pty.apply(b"a\"bcdefgh"), b"***Q***");
        assert_eq!(m.pty.apply(b"a\\\"bcdefgh"), b"a\\\"bcdefgh");
        assert_eq!(m.transcript.apply(b"\"a\\\"bcdefgh\""), b"\"***Q***\"");
        assert!(SessionMask::new(&[("CURTO", "1234567")]).pty.is_empty());
    }
}

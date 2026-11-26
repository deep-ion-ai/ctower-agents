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
//! ## Valor atual durante a sessão (#165)
//!
//! O ambiente do processo não muda depois do spawn, e tokens de vida curta
//! (JWT) expiram no meio da sessão. Por isso:
//!
//! - no launch, a sessão ganha um token próprio ([`SESSION_TOKEN_ENV`]),
//!   gerado aqui e guardado só em memória até o processo sair;
//! - `ctower secret get NOME` manda esse token ao `POST /session/secret` do
//!   daemon, que o troca pela sessão ([`Sessions::session_secret`]) e
//!   devolve o valor atual do cache para o projeto dela (mesma resolução do
//!   spawn). O cabeçalho `X-Ctower-Session` não vale aqui: uma sessão que o
//!   trocasse leria os secrets de outro projeto;
//! - quando o pull regrava o cache, [`Sessions::refresh_masks`] soma ao
//!   conjunto mascarado de cada sessão viva os valores novos do projeto dela,
//!   mantendo os anteriores. O mesmo vale quando o sync resolve o token de
//!   uma conexão (Jira, GitLab): o spawn só mascara os tokens já em cache e
//!   não roda comando de credencial (#266);
//! - o system prompt da sessão com secrets ([`agent_instruction`]) orienta a
//!   usar `$(ctower secret get NOME)` e a nunca imprimir o valor.
//!
//! ## Sessão do gestor (#219)
//!
//! A sessão do gestor de projetos (projeto sentinela `_ctower/gestor`) não
//! pertence a um repositório: recebe só as entradas **globais**, nunca as
//! de algum projeto (`resolve(None)`). O mascaramento segue a mesma regra.

use crate::resolver::{gitlab_key, jira_key};
use crate::Sessions;
use ct_core::ProjectKind;
use ct_ledger::{Project, Session};
use ct_proc::Secret;
use ct_pty::{MaskSet, Zeroizing};
use ct_secrets::{is_reserved_name, EnvCache, EnvSecrets, Kind, MIN_MASK_LEN};
use serde_json::json;
use std::io::Read;

/// Variável com o token da sessão (#165). Reservada (prefixo `CTOWER_`).
pub const SESSION_TOKEN_ENV: &str = "CTOWER_SESSION_TOKEN";

/// Erro de [`Sessions::session_secret`]. Nenhuma variante carrega valor.
#[derive(Debug, thiserror::Error)]
pub enum SecretGetError {
    /// Token ausente, desconhecido ou de uma sessão que já saiu.
    #[error("token de sessão inválido ou de uma sessão encerrada")]
    Unauthorized,
    #[error("{0}")]
    Invalid(String),
    /// Nome que não vale para o projeto da sessão (inexistente, só de outro
    /// projeto ou reservado). Não diz qual dos casos.
    #[error("{0}")]
    NotFound(String),
    /// Sem entrega do hub ainda, ou cache ilegível.
    #[error("{0}")]
    Unavailable(String),
}

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

impl SessionMask {
    /// Este conjunto somado ao de `other` (valores antigos continuam).
    fn union(&self, other: &SessionMask) -> SessionMask {
        SessionMask {
            pty: self.pty.union(&other.pty),
            transcript: self.transcript.union(&other.transcript),
        }
    }
}

/// Variáveis de ambiente de uma sessão e o mascaramento dos seus secrets.
pub(crate) struct SessionSecrets {
    pub env: Vec<(String, String)>,
    pub mask: SessionMask,
}

impl SessionSecrets {
    /// Nomes injetados, para a instrução ao agente.
    pub fn names(&self) -> Vec<&str> {
        self.env.iter().map(|(n, _)| n.as_str()).collect()
    }
}

/// Instrução ao agente de uma sessão que recebeu secrets ou variáveis do hub
/// (#165). Só nomes, nunca valores. `None` = nada injetado.
pub(crate) fn agent_instruction(names: &[&str]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    Some(format!(
        "Secrets do hub (Control Tower): esta sessão recebeu {} como variáveis de ambiente, \
         com o valor do início da sessão. Um secret pode ser renovado no hub durante a sessão \
         (um token que expira, por exemplo): dentro dos comandos, use $(ctower secret get NOME) \
         em vez de $NOME para ler o valor atual. Nunca imprima, registre nem cite o valor de um \
         secret (nada de echo, env, printenv ou set -x com ele); use-o só dentro do comando que \
         precisa dele.",
        names.join(", ")
    ))
}

/// Igualdade sem atalho no primeiro byte diferente.
fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Token novo de sessão: 32 bytes aleatórios em hex.
pub(crate) fn new_session_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
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
    /// Na sessão de release (`release`, #204), o secret do token exclusivo
    /// também vai como `GH_TOKEN`; nas outras, ele não vai.
    pub(crate) fn session_secrets(
        &self,
        project: &Project,
        session_id: &str,
        release: bool,
    ) -> SessionSecrets {
        // Tokens das conexões (Jira, GitLab) entram no mascaramento mesmo
        // sem secrets do hub (#236).
        let conn = self.connection_mask(project);
        let none = || SessionSecrets {
            env: Vec::new(),
            mask: conn.clone(),
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
        let env = injected(&secrets, project, session_id, release);
        let mask = mask_of(&secrets, project).union(&conn);
        SessionSecrets { env, mask }
    }

    /// Mascaramento dos tokens das conexões do projeto (Jira e GitLab, #236).
    /// Só com os tokens que já estão no cache do resolvedor (#266): abrir
    /// uma sessão nunca roda comando de credencial, que pode esperar o
    /// pinentry ou o prazo inteiro. Quem resolve é o sync; o token novo
    /// chega às sessões vivas por [`Sessions::refresh_masks`].
    ///
    /// Vale para qualquer fonte (comando, secret ou arquivo, #274): o cache
    /// guarda o token com a fonte que o resolveu, a mesma que o sync usa
    /// ([`Sessions::jira_source`], [`Sessions::gitlab_source`]).
    pub(crate) fn connection_mask(&self, project: &Project) -> SessionMask {
        let resolver = self.credentials();
        let mut found: Vec<std::sync::Arc<Secret>> = Vec::new();
        if let Some(s) = self.jira_source(project) {
            found.extend(resolver.cached(&jira_key(project.id), &s.source));
        }
        if let Some(s) = self.gitlab_source(project) {
            found.extend(resolver.cached(&gitlab_key(project.id), &s.source));
        }
        let pairs: Vec<(&str, &str)> = found
            .iter()
            .map(|t| ("CONEXAO_TOKEN", t.expose()))
            .collect();
        SessionMask::new(&pairs)
    }

    /// Busca de secret do resolvedor para as conexões do projeto (fonte
    /// `Secret`, #274): o valor atual no cache de secrets do ambiente, pelas
    /// regras do spawn e do `ctower secret get` ([`cached_secret`]).
    pub(crate) fn secret_lookup<'a>(
        &'a self,
        project: &'a Project,
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| cached_secret(&self.env_cache(), project, name)
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

    /// Guarda (só em memória) o token da sessão (#165).
    pub(crate) fn set_session_token(&self, token: &str, session_id: &str) {
        self.inner
            .session_tokens
            .lock()
            .unwrap()
            .insert(token.to_string(), session_id.to_string());
    }

    /// Esquece o token da sessão (processo saiu ou spawn falhou).
    pub(crate) fn forget_session_token(&self, session_id: &str) {
        self.inner
            .session_tokens
            .lock()
            .unwrap()
            .retain(|_, s| s != session_id);
    }

    /// Sessão dona do token. Comparação em tempo constante com cada token
    /// guardado: são poucas sessões vivas.
    pub(crate) fn session_of_token(&self, token: &str) -> Option<String> {
        if token.is_empty() {
            return None;
        }
        let tokens = self.inner.session_tokens.lock().unwrap();
        tokens
            .iter()
            .find(|(t, _)| same_bytes(t.as_bytes(), token.as_bytes()))
            .map(|(_, s)| s.clone())
    }

    /// Valor atual de `name` do cache para o projeto da sessão dona de
    /// `token` (#165): globais e do projeto, o do projeto sobrepõe o global,
    /// como no spawn. Vale para `secret` e `variable`. Nome reservado,
    /// inexistente ou só de outro projeto: [`SecretGetError::NotFound`].
    /// Cache ilegível: a mesma mensagem e o mesmo alerta do spawn.
    pub fn session_secret(&self, token: &str, name: &str) -> Result<String, SecretGetError> {
        let session_id = self
            .session_of_token(token)
            .ok_or(SecretGetError::Unauthorized)?;
        ct_secrets::validate_name(name).map_err(|e| SecretGetError::Invalid(e.to_string()))?;
        let not_found =
            || SecretGetError::NotFound(format!("{name} não existe para o projeto da sessão"));
        if is_reserved_name(name) {
            return Err(SecretGetError::NotFound(format!(
                "{name} é um nome reservado do ambiente; não vem do hub"
            )));
        }
        let (project, wi) = {
            let l = self.ledger();
            l.get_session(&session_id)
                .and_then(|s| l.get_work_item(s.work_item_id))
                .and_then(|wi| Ok((l.get_project(wi.project_id)?, wi)))
        }
        .map_err(|e| SecretGetError::Unavailable(e.to_string()))?;
        // O token exclusivo da release (#204) só vale na sessão de release:
        // nas outras, o nome não existe.
        if project.release.reserved_secret().as_deref() == Some(name)
            && !crate::release::is_release_item(&wi)
        {
            return Err(not_found());
        }
        let secrets = match self.env_cache().load() {
            Ok(Some(s)) => s,
            Ok(None) => {
                return Err(SecretGetError::Unavailable(
                    "sem cache de secrets: o ambiente ainda não recebeu entrega do hub".into(),
                ))
            }
            Err(e) => {
                tracing::warn!(
                    "secrets: cache do ambiente ilegível ({e}); {name} não lido na sessão {session_id}"
                );
                self.notify(
                    "alert",
                    json!({ "kind": "secrets_unavailable", "session_id": session_id,
                            "error": e.to_string() }),
                );
                return Err(SecretGetError::Unavailable(format!(
                    "cache do ambiente ilegível: {e}"
                )));
            }
        };
        let key = scope_of(&project);
        let resolved = secrets.resolve(key.as_deref());
        let entry = resolved.get(name).ok_or_else(not_found)?;
        tracing::info!("secrets: {name} lido pela sessão {session_id}");
        Ok(entry.value.expose().to_string())
    }

    /// O cache mudou (pull com entrega nova) ou o sync resolveu o token de
    /// uma conexão (#266): soma ao conjunto mascarado de cada sessão viva os
    /// secrets atuais do projeto dela e os tokens das conexões em cache,
    /// mantendo os anteriores (a saída antiga ainda pode trazer o valor
    /// velho), e o aplica ao PTY no ar. Cache de secrets ausente ou
    /// ilegível: só os tokens das conexões.
    pub fn refresh_masks(&self) {
        let live: Vec<(String, ct_pty::PtySession)> = self
            .inner
            .live
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if live.is_empty() {
            return;
        }
        let secrets = match self.env_cache().load() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "secrets: cache do ambiente ilegível ({e}); mascaramento das sessões só com os tokens das conexões"
                );
                None
            }
        };
        for (sid, pty) in live {
            let project = {
                let l = self.ledger();
                l.get_session(&sid)
                    .and_then(|s| l.get_work_item(s.work_item_id))
                    .and_then(|wi| l.get_project(wi.project_id))
            };
            let Ok(project) = project else {
                continue;
            };
            let conn = self.connection_mask(&project);
            let fresh = match &secrets {
                Some(secrets) => mask_of(secrets, &project).union(&conn),
                None => conn,
            };
            let merged = {
                let mut masks = self.inner.masks.lock().unwrap();
                let merged = masks.get(&sid).cloned().unwrap_or_default().union(&fresh);
                if !merged.pty.is_empty() {
                    masks.insert(sid.clone(), merged.clone());
                }
                merged
            };
            pty.set_mask(merged.pty);
        }
    }

    /// Esquece o mascaramento de uma sessão (retomada ou arquivada).
    pub(crate) fn forget_mask(&self, session_id: &str) {
        self.inner.masks.lock().unwrap().remove(session_id);
    }

    /// Mascaramento do backup do transcript da sessão: o guardado no spawn
    /// ou, sem ele (daemon reiniciado), recalculado do cache atual pelo
    /// projeto da sessão. Cache de secrets ausente ou ilegível: só os tokens
    /// das conexões em cache (não há outros valores conhecidos a esconder).
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
        let conn = self.connection_mask(&project);
        let mask = match self.env_cache().load() {
            Ok(Some(secrets)) => mask_of(&secrets, &project).union(&conn),
            Ok(None) => conn,
            Err(e) => {
                tracing::warn!(
                    "secrets: cache do ambiente ilegível ({e}); backup da sessão {} só com os tokens das conexões",
                    s.id
                );
                conn
            }
        };
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
/// do projeto, sem os nomes reservados (com aviso). O secret do token
/// exclusivo da release (#204) só vai para a sessão de release, que o
/// recebe também como `GH_TOKEN` (no lugar de um `GH_TOKEN` do hub).
fn injected(
    secrets: &EnvSecrets,
    project: &Project,
    session_id: &str,
    release: bool,
) -> Vec<(String, String)> {
    let key = scope_of(project);
    let reserved = project.release.reserved_secret();
    let mut env = Vec::new();
    let mut release_token = None;
    for (name, entry) in secrets.resolve(key.as_deref()) {
        if is_reserved_name(&name) {
            tracing::warn!(
                "secrets: {name} é um nome reservado do ambiente; ignorado na sessão {session_id}"
            );
            continue;
        }
        if reserved.as_deref() == Some(name.as_str()) {
            if !release {
                continue;
            }
            release_token = Some(entry.value.expose().to_string());
        }
        env.push((name, entry.value.expose().to_string()));
    }
    if let Some(t) = release_token {
        env.retain(|(n, _)| n != RELEASE_GH_TOKEN_ENV);
        env.push((RELEASE_GH_TOKEN_ENV.to_string(), t));
    }
    env
}

/// Variável que o `gh` e o `ctower release` leem para o token (#204).
pub(crate) const RELEASE_GH_TOKEN_ENV: &str = "GH_TOKEN";

/// Projeto (`owner/repo`) cujas entradas a sessão recebe além das globais.
/// `None` no gestor, que não está preso a um projeto: só as globais.
pub(crate) fn scope_of(project: &Project) -> Option<String> {
    (project.kind != ProjectKind::Gestor)
        .then(|| format!("{}/{}", project.gh_owner, project.gh_repo))
}

/// Valor de `name` no cache de secrets do ambiente para o projeto (#279):
/// globais e do projeto, o do projeto sobrepõe o global (no gestor, só os
/// globais), como no spawn e no `ctower secret get`. Nome reservado, cache
/// ausente ou ilegível: `None` (o resolvedor diz que o secret não existe no
/// ambiente; o cache ilegível já avisa no log).
pub(crate) fn cached_secret(cache: &EnvCache, project: &Project, name: &str) -> Option<String> {
    if is_reserved_name(name) {
        return None;
    }
    let secrets = match cache.load() {
        Ok(s) => s?,
        Err(e) => {
            tracing::warn!("secrets: cache do ambiente ilegível ({e}); {name} não lido");
            return None;
        }
    };
    let resolved = secrets.resolve(scope_of(project).as_deref());
    resolved.get(name).map(|e| e.value.expose().to_string())
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
    use crate::test_support::{FakeClaude, TestEnv, T};

    /// O token de uma conexão que a sessão imprime sai mascarado, no PTY e
    /// no backup do transcript (#236).
    #[test]
    fn token_de_conexao_sai_mascarado_no_pty_e_no_transcript() {
        let m = SessionMask::new(&[("CONEXAO_TOKEN", "tok-acme-123456")]);
        let saida = String::from_utf8(m.pty.apply(b"ok tok-acme-123456 fim")).unwrap();
        assert!(!saida.contains("tok-acme-123456"), "{saida}");
        assert!(saida.contains("***CONEXAO_TOKEN***"), "{saida}");
        let json = m.transcript.apply(br#"{"t":"tok-acme-123456"}"#);
        assert!(!String::from_utf8(json).unwrap().contains("tok-acme-123456"));
    }
    use crate::{AgentOptions, GestorScope};
    use ct_core::provider::CredentialSource;
    use ct_core::SessionState;
    use std::time::{Duration, Instant};

    /// Token da sessão, do registro do `claude` falso.
    fn token_of(env: &TestEnv, session_id: &str) -> String {
        let out = env.wait_fake_output(session_id);
        out.lines()
            .find_map(|l| l.strip_prefix(&format!("{SESSION_TOKEN_ENV}=")))
            .expect("sessão sem token")
            .to_string()
    }

    fn cache_acme(env: &TestEnv) {
        env.write_secrets_cache_kinds(&[
            (None, "API_JWT", Kind::Secret, "jwt-global-aaaaaaaa"),
            (None, "SO_GLOBAL", Kind::Secret, "valor-so-global-1"),
            (
                Some("deep-ion-ai/demo"),
                "API_JWT",
                Kind::Secret,
                "jwt-do-projeto-bbbbbbbb",
            ),
            (
                Some("deep-ion-ai/demo"),
                "API_URL",
                Kind::Variable,
                "https://api.example.com",
            ),
            (
                Some("acme/acme-web"),
                "SO_OUTRO",
                Kind::Secret,
                "valor-de-outro-projeto",
            ),
            (None, "PATH", Kind::Variable, "/caminho/falso"),
        ]);
    }

    #[test]
    fn valor_atual_pelo_token_da_sessao() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        cache_acme(&env);
        let s = env.sessions.start(env.request(31, "Secret get")).unwrap();
        let tok = token_of(&env, &s.id);
        assert_eq!(tok.len(), 64);
        let get = |n: &str| env.sessions.session_secret(&tok, n);
        // Projeto sobrepõe global; global sozinho; variável também.
        assert_eq!(get("API_JWT").unwrap(), "jwt-do-projeto-bbbbbbbb");
        assert_eq!(get("SO_GLOBAL").unwrap(), "valor-so-global-1");
        assert_eq!(get("API_URL").unwrap(), "https://api.example.com");
        // Inexistente, de outro projeto e reservado: mesma família de erro,
        // sem valor na mensagem.
        for n in ["NAO_EXISTE", "SO_OUTRO", "PATH", "CTOWER_TOKEN"] {
            match get(n) {
                Err(SecretGetError::NotFound(m)) => {
                    assert!(m.contains(n) && !m.contains("valor"), "{m}")
                }
                r => panic!("{n}: {r:?}"),
            }
        }
        assert!(matches!(
            get("nome-invalido"),
            Err(SecretGetError::Invalid(_))
        ));
        // O cache muda: o valor novo vale na hora, sem reiniciar a sessão.
        env.write_secrets_cache(&[(Some("deep-ion-ai/demo"), "API_JWT", "jwt-renovado-cccccccc")]);
        assert_eq!(get("API_JWT").unwrap(), "jwt-renovado-cccccccc");
        assert!(matches!(get("SO_GLOBAL"), Err(SecretGetError::NotFound(_))));
        // Token ausente ou desconhecido.
        for t in ["", "x", &"0".repeat(64)] {
            assert!(matches!(
                env.sessions.session_secret(t, "API_JWT"),
                Err(SecretGetError::Unauthorized)
            ));
        }
        // O processo sai: o token deixa de valer.
        env.sessions.kill(&s.id).unwrap();
        env.wait_state(&s.id, SessionState::Exited);
        assert!(matches!(get("API_JWT"), Err(SecretGetError::Unauthorized)));
    }

    #[test]
    fn token_de_outra_sessao_le_so_o_escopo_dela() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        cache_acme(&env);
        let projeto = env.sessions.start(env.request(32, "Projeto")).unwrap();
        let gestor = env
            .sessions
            .start_gestor(&GestorScope::Portfolio, "", None, &AgentOptions::default())
            .unwrap();
        let (tp, tg) = (token_of(&env, &projeto.id), token_of(&env, &gestor.id));
        assert_ne!(tp, tg);
        // O gestor só tem os globais: o token dele não lê os do projeto.
        assert_eq!(
            env.sessions.session_secret(&tg, "API_JWT").unwrap(),
            "jwt-global-aaaaaaaa"
        );
        assert!(matches!(
            env.sessions.session_secret(&tg, "API_URL"),
            Err(SecretGetError::NotFound(_))
        ));
        assert_eq!(
            env.sessions.session_secret(&tp, "API_JWT").unwrap(),
            "jwt-do-projeto-bbbbbbbb"
        );
        env.sessions.kill(&projeto.id).unwrap();
        env.sessions.kill(&gestor.id).unwrap();
    }

    #[test]
    fn cache_ausente_ou_ilegivel_da_erro_sem_valor() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = env.sessions.start(env.request(33, "Sem cache")).unwrap();
        let tok = token_of(&env, &s.id);
        match env.sessions.session_secret(&tok, "API_JWT") {
            Err(SecretGetError::Unavailable(m)) => assert!(m.contains("sem cache"), "{m}"),
            r => panic!("{r:?}"),
        }
        cache_acme(&env);
        // Chave trocada por fora: o cache não abre mais.
        std::fs::remove_file(env.cfg.dirs.env_key_file()).unwrap();
        env.sessions.env_cache().ensure_key().unwrap();
        match env.sessions.session_secret(&tok, "API_JWT") {
            Err(SecretGetError::Unavailable(m)) => {
                assert!(m.contains("cache do ambiente ilegível"), "{m}");
                assert!(!m.contains("jwt-"), "{m}");
            }
            r => panic!("{r:?}"),
        }
        assert!(env.notices().iter().any(|(n, d)| n == "alert"
            && d["kind"] == "secrets_unavailable"
            && d["session_id"] == s.id.as_str()));
        env.sessions.kill(&s.id).unwrap();
    }

    #[test]
    fn instrucao_ao_agente_so_com_nomes() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        cache_acme(&env);
        let s = env.sessions.start(env.request(34, "Instrução")).unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("$(ctower secret get NOME)"), "{out}");
        assert!(out.contains("API_JWT, API_URL, SO_GLOBAL"), "{out}");
        assert!(!out.contains("jwt-do-projeto"), "{out}");
        env.sessions.kill(&s.id).unwrap();
        // Sem nada injetado, sem instrução.
        assert!(agent_instruction(&[]).is_none());
    }

    /// Lê o scrollback até achar `needle`.
    fn wait_output(env: &TestEnv, sid: &str, needle: &str) -> String {
        let pty = env.sessions.pty(sid).expect("sessão viva");
        let deadline = Instant::now() + T;
        loop {
            let s = String::from_utf8_lossy(&pty.snapshot()).to_string();
            if s.contains(needle) {
                return s;
            }
            assert!(Instant::now() < deadline, "sem {needle:?} em {s:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn cache_novo_soma_valores_ao_mascaramento_da_sessao_viva() {
        let env = TestEnv::new(FakeClaude::script(
            "while read l; do echo \"saida=$l\"; done",
        ));
        cache_acme(&env);
        let s = env.sessions.start(env.request(35, "Máscara")).unwrap();
        env.wait_fake_output(&s.id);
        let pty = env.sessions.pty(&s.id).unwrap();
        pty.write(b"jwt-do-projeto-bbbbbbbb\n").unwrap();
        wait_output(&env, &s.id, "saida=***API_JWT***");
        // O hub renova o token: o valor novo ainda não é conhecido.
        env.write_secrets_cache(&[
            (Some("deep-ion-ai/demo"), "API_JWT", "jwt-renovado-cccccccc"),
            (Some("acme/acme-web"), "SO_OUTRO", "valor-de-outro-projeto"),
        ]);
        env.sessions.refresh_masks();
        pty.write(b"jwt-renovado-cccccccc jwt-do-projeto-bbbbbbbb valor-de-outro-projeto\n")
            .unwrap();
        let out = wait_output(&env, &s.id, "saida=***API_JWT*** ***API_JWT***");
        assert!(!out.contains("jwt-renovado"), "{out}");
        assert!(!out.contains("jwt-do-projeto"), "{out}");
        // Secret de outro projeto não entra no conjunto da sessão.
        assert!(out.contains("valor-de-outro-projeto"), "{out}");
        // O backup do transcript usa o mesmo conjunto, com o novo e o antigo.
        let sess = env.sessions.ledger().get_session(&s.id).unwrap();
        let t = env.sessions.transcript_mask(&sess);
        assert_eq!(
            t.apply(b"jwt-renovado-cccccccc jwt-do-projeto-bbbbbbbb"),
            b"***API_JWT*** ***API_JWT***"
        );
        env.sessions.kill(&s.id).unwrap();
    }

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

    /// Comando de credencial que deixa marca a cada execução e imprime o
    /// token.
    fn cmd_com_marca(marca: &std::path::Path, token: &str) -> String {
        format!("echo x >> {}; printf '{token}\\n'", marca.display())
    }

    /// O projeto do ambiente vira Jira + GitLab, com os comandos dados.
    fn assign_jira_gitlab(env: &TestEnv, jira_cmd: &str, gitlab_cmd: &str) {
        env.assign_config(1, |p| {
            p.tracker = ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                url: "https://jira.example.com".into(),
                project_key: "ACME".into(),
                credential_cmd: jira_cmd.into(),
                ..Default::default()
            });
            p.forge = ct_core::ForgeConfig::Gitlab(ct_core::GitlabConfig {
                connection: None,
                host: "gitlab.example.com".into(),
                path: "acme/acme-web".into(),
                clone_url: "https://gitlab.example.com/acme/acme-web.git".into(),
                credential_cmd: gitlab_cmd.into(),
            });
        });
    }

    /// O que o sync faz: resolve o token pelo resolvedor do ambiente.
    fn sync_resolve(env: &TestEnv, key: &str, cmd: &str) {
        let source = CredentialSource::Command { cmd: cmd.into() };
        env.sessions
            .credentials()
            .token(crate::resolver::Ask {
                key,
                what: "Jira",
                source: &source,
                lookup: &crate::resolver::no_lookup,
                deadline: Duration::from_secs(5),
            })
            .unwrap();
    }

    /// Abrir sessão não roda o comando de credencial das conexões (#266):
    /// nem no spawn, nem ao atualizar o mascaramento ou fazer o backup.
    #[test]
    fn abrir_sessao_nao_roda_comando_de_credencial() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let marca = env.tmp.path().join("marca-credencial");
        let cmd = cmd_com_marca(&marca, "tok-acme-jira-123456");
        assign_jira_gitlab(&env, &cmd, &cmd);
        let s = env
            .sessions
            .start(env.request(35, "Sem credencial"))
            .unwrap();
        env.wait_fake_output(&s.id);
        env.sessions.refresh_masks();
        let sess = env.sessions.ledger().get_session(&s.id).unwrap();
        env.sessions.transcript_mask(&sess);
        assert!(
            !marca.exists(),
            "abrir a sessão rodou o comando de credencial"
        );
        env.sessions.kill(&s.id).unwrap();
    }

    /// Token que o sync já resolveu sai mascarado no PTY e no transcript da
    /// sessão aberta depois (#266), sem rodar o comando de novo.
    #[test]
    fn token_ja_resolvido_pelo_sync_sai_mascarado() {
        let env = TestEnv::new(FakeClaude::script(
            "while read l; do echo \"saida=$l\"; done",
        ));
        let marca = env.tmp.path().join("marca-credencial");
        let jira = cmd_com_marca(&marca, "tok-acme-jira-123456");
        let gitlab = cmd_com_marca(&marca, "tok-acme-gitlab-654321");
        assign_jira_gitlab(&env, &jira, &gitlab);
        sync_resolve(&env, &crate::resolver::jira_key(env.project_id), &jira);
        sync_resolve(&env, &crate::resolver::gitlab_key(env.project_id), &gitlab);
        let s = env.sessions.start(env.request(35, "Máscara")).unwrap();
        env.wait_fake_output(&s.id);
        let pty = env.sessions.pty(&s.id).unwrap();
        pty.write(b"tok-acme-jira-123456 tok-acme-gitlab-654321\n")
            .unwrap();
        let out = wait_output(&env, &s.id, "saida=***CONEXAO_TOKEN*** ***CONEXAO_TOKEN***");
        assert!(!out.contains("tok-acme-"), "{out}");
        let sess = env.sessions.ledger().get_session(&s.id).unwrap();
        let t = env.sessions.transcript_mask(&sess);
        assert_eq!(
            t.apply(br#"{"t":"tok-acme-jira-123456 tok-acme-gitlab-654321"}"#),
            br#"{"t":"***CONEXAO_TOKEN*** ***CONEXAO_TOKEN***"}"#
        );
        let linhas = std::fs::read_to_string(&marca).unwrap();
        assert_eq!(linhas.lines().count(), 2, "o comando rodou fora do sync");
        env.sessions.kill(&s.id).unwrap();
    }

    /// Token que o sync resolve com a sessão já aberta passa a ser mascarado
    /// nela (#266): o resolvedor avisa e o mascaramento é atualizado.
    #[test]
    fn token_resolvido_depois_do_spawn_entra_na_sessao_viva() {
        let env = TestEnv::new(FakeClaude::script(
            "while read l; do echo \"saida=$l\"; done",
        ));
        let jira = "printf 'tok-acme-jira-777777\\n'";
        assign_jira_gitlab(&env, jira, "");
        let s = env.sessions.start(env.request(35, "Máscara")).unwrap();
        env.wait_fake_output(&s.id);
        assert!(!env.sessions.inner.masks.lock().unwrap().contains_key(&s.id));
        sync_resolve(&env, &crate::resolver::jira_key(env.project_id), jira);
        let deadline = Instant::now() + T;
        while !env.sessions.inner.masks.lock().unwrap().contains_key(&s.id) {
            assert!(Instant::now() < deadline, "máscara não atualizada");
            std::thread::sleep(Duration::from_millis(20));
        }
        let pty = env.sessions.pty(&s.id).unwrap();
        pty.write(b"tok-acme-jira-777777\n").unwrap();
        let out = wait_output(&env, &s.id, "saida=***CONEXAO_TOKEN***");
        assert!(!out.contains("tok-acme-jira"), "{out}");
        let sess = env.sessions.ledger().get_session(&s.id).unwrap();
        let t = env.sessions.transcript_mask(&sess);
        assert_eq!(t.apply(b"tok-acme-jira-777777"), b"***CONEXAO_TOKEN***");
        env.sessions.kill(&s.id).unwrap();
    }

    /// Tokens de conexão com `Secret` e `File` (#274) que o sync resolve com
    /// a sessão aberta entram no mascaramento dela, no PTY e no transcript.
    #[test]
    fn token_de_secret_e_de_arquivo_entra_na_sessao_viva() {
        use ct_core::provider::{Connection, ProviderKind};
        let env = TestEnv::new(FakeClaude::script(
            "while read l; do echo \"saida=$l\"; done",
        ));
        let arquivo = env.tmp.path().join("gitlab token");
        std::fs::write(&arquivo, " tok-acme-arquivo-424242 \n").unwrap();
        env.assign_config(1, |p| {
            p.tracker = ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                connection: Some(7),
                url: "https://jira.example.com".into(),
                project_key: "ACME".into(),
                ..Default::default()
            });
            p.forge = ct_core::ForgeConfig::Gitlab(ct_core::GitlabConfig {
                connection: Some(8),
                host: "gitlab.example.com".into(),
                path: "acme/acme-web".into(),
                clone_url: "https://gitlab.example.com/acme/acme-web.git".into(),
                credential_cmd: String::new(),
            });
        });
        let conn = |id, kind, credential| Connection {
            id,
            name: format!("{kind}-acme"),
            kind,
            endpoint: "https://jira.example.com".into(),
            credential,
            options: json!({}),
            created_at: 0,
            updated_at: 0,
        };
        env.cfg.credentials.set_connections(
            vec![
                conn(
                    7,
                    ProviderKind::Jira,
                    CredentialSource::Secret {
                        name: "JIRA_TOKEN_ACME".into(),
                    },
                ),
                conn(
                    8,
                    ProviderKind::Gitlab,
                    CredentialSource::File {
                        path: arquivo.display().to_string(),
                    },
                ),
            ],
            Default::default(),
        );
        // Como variável, o valor não entra no mascaramento dos secrets do
        // hub: o que o esconde é o cache do resolvedor.
        env.write_secrets_cache_kinds(&[(
            Some("deep-ion-ai/demo"),
            "JIRA_TOKEN_ACME",
            Kind::Variable,
            "tok-acme-secret-313131",
        )]);
        let s = env.sessions.start(env.request(35, "Máscara")).unwrap();
        env.wait_fake_output(&s.id);
        assert!(!env.sessions.inner.masks.lock().unwrap().contains_key(&s.id));

        // O que o sync faz: a fonte efetiva e a busca no cache de secrets.
        let p = env.sessions.ledger().get_project(env.project_id).unwrap();
        let lookup = env.sessions.secret_lookup(&p);
        for (key, what, source) in [
            (
                crate::resolver::jira_key(p.id),
                "Jira",
                env.sessions.jira_source(&p).unwrap().source,
            ),
            (
                crate::resolver::gitlab_key(p.id),
                "GitLab",
                env.sessions.gitlab_source(&p).unwrap().source,
            ),
        ] {
            env.sessions
                .credentials()
                .token(crate::resolver::Ask {
                    key: &key,
                    what,
                    source: &source,
                    lookup: &lookup,
                    deadline: Duration::from_secs(5),
                })
                .unwrap();
        }
        let deadline = Instant::now() + T;
        loop {
            let n = env
                .sessions
                .inner
                .masks
                .lock()
                .unwrap()
                .get(&s.id)
                .map(|m| m.pty.apply(b"tok-acme-arquivo-424242"));
            if n.as_deref() == Some(b"***CONEXAO_TOKEN***".as_slice()) {
                break;
            }
            assert!(Instant::now() < deadline, "máscara não atualizada");
            std::thread::sleep(Duration::from_millis(20));
        }
        let pty = env.sessions.pty(&s.id).unwrap();
        pty.write(b"tok-acme-secret-313131 tok-acme-arquivo-424242\n")
            .unwrap();
        let out = wait_output(&env, &s.id, "saida=***CONEXAO_TOKEN*** ***CONEXAO_TOKEN***");
        assert!(!out.contains("tok-acme-"), "{out}");
        let sess = env.sessions.ledger().get_session(&s.id).unwrap();
        let t = env.sessions.transcript_mask(&sess);
        assert_eq!(
            t.apply(br#"{"t":"tok-acme-secret-313131 tok-acme-arquivo-424242"}"#),
            br#"{"t":"***CONEXAO_TOKEN*** ***CONEXAO_TOKEN***"}"#
        );
        env.sessions.kill(&s.id).unwrap();
    }
}

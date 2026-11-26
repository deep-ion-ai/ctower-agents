//! Resolvedor único de credencial (#236): um lugar só para obter o token de
//! uma conexão, qualquer que seja a fonte ([`CredentialSource`]).
//!
//! - `Command` roda o comando de credencial (primeira linha da saída);
//! - `Cli` pede o token ao `gh auth token`;
//! - `File` lê o arquivo (primeira linha não vazia, `~` expandido);
//! - `Secret` pede o valor ao store de secrets do ambiente (quem chama
//!   passa a busca por nome: no Jira e no GitLab, o cache de secrets do
//!   ambiente no projeto, `Sessions::secret_lookup`, #274);
//! - `None` não tem token e vira erro.
//!
//! A fonte de cada projeto vem de `SharedCredentials::jira_source` e
//! `gitlab_source`: a conexão do hub com a sobrescrita do ambiente.
//!
//! O cache é em memória, por chave (`jira:<projeto>`, `gitlab:<projeto>`),
//! e guarda a fonte usada: se a fonte mudar (cadastro ou sobrescrita por
//! ambiente), o token é resolvido de novo. [`CredentialResolver::with_refresh`]
//! descarta o token quando a operação falha por autenticação e tenta uma vez
//! com um token novo. Nenhum valor entra em log ou erro.
//!
//! [`CredentialResolver::cached`] só lê o cache, sem resolver: é o que o
//! mascaramento das sessões usa (#266), para que abrir uma sessão nunca
//! rode comando de credencial. Cada token resolvido de novo avisa quem
//! criou o resolvedor ([`CredentialResolver::with_on_new`]), que atualiza o
//! mascaramento das sessões vivas.

use ct_core::provider::CredentialSource;
use ct_proc::{run_credential, CredentialError, Secret};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Comando do `Cli` (token da CLI do GitHub; só GitHub por ora).
const CLI_TOKEN_CMD: &str = "gh auth token";

/// Busca de um secret do store do ambiente pelo nome.
pub(crate) type SecretLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Sem store de secrets na busca (testes).
#[cfg(test)]
pub(crate) fn no_lookup(_: &str) -> Option<String> {
    None
}

/// Chave do cache do Jira de um projeto.
pub(crate) fn jira_key(project_id: i64) -> String {
    format!("jira:{project_id}")
}

/// Chave do cache do GitLab de um projeto.
pub(crate) fn gitlab_key(project_id: i64) -> String {
    format!("gitlab:{project_id}")
}

/// O que pedir ao resolvedor: a chave do cache, o provedor (para as
/// mensagens), a fonte, a busca de secrets e o prazo do comando.
#[derive(Clone, Copy)]
pub(crate) struct Ask<'a> {
    pub key: &'a str,
    pub what: &'static str,
    pub source: &'a CredentialSource,
    pub lookup: SecretLookup<'a>,
    pub deadline: Duration,
}

struct Entry {
    source: CredentialSource,
    token: Arc<Secret>,
}

/// Aviso de token novo no cache (sem o valor).
type OnNew = Box<dyn Fn() + Send + Sync>;

/// Cache de tokens por chave. Fica no `Sessions`, compartilhado.
pub(crate) struct CredentialResolver {
    cache: Mutex<HashMap<String, Entry>>,
    on_new: Option<OnNew>,
}

impl std::fmt::Debug for CredentialResolver {
    /// Sem o conteúdo do cache: só quantas chaves há.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.cache.lock().map(|c| c.len()).unwrap_or(0);
        f.debug_struct("CredentialResolver")
            .field("chaves", &n)
            .finish()
    }
}

impl CredentialResolver {
    pub(crate) fn new() -> Self {
        CredentialResolver {
            cache: Mutex::new(HashMap::new()),
            on_new: None,
        }
    }

    /// Resolvedor que chama `on_new` a cada token resolvido e guardado no
    /// cache, já fora do lock (#266). Quem chama `token` pode estar com
    /// outros locks: `on_new` não deve tomá-los na mesma thread.
    pub(crate) fn with_on_new(on_new: impl Fn() + Send + Sync + 'static) -> Self {
        CredentialResolver {
            cache: Mutex::new(HashMap::new()),
            on_new: Some(Box::new(on_new)),
        }
    }

    /// Token da chave só se já estiver no cache com a mesma fonte. Nunca
    /// resolve (não roda comando, não lê arquivo).
    pub(crate) fn cached(&self, key: &str, source: &CredentialSource) -> Option<Arc<Secret>> {
        let cache = self.cache.lock().unwrap();
        let e = cache.get(key)?;
        (e.source == *source).then(|| e.token.clone())
    }

    /// Token da chave: do cache se a fonte não mudou, senão resolvido agora.
    pub(crate) fn token(&self, ask: Ask<'_>) -> Result<Arc<Secret>, CredentialError> {
        self.token_and_origin(ask).map(|(t, _)| t)
    }

    /// Como [`token`](Self::token), dizendo se o token veio do cache
    /// (`true`: a fonte não foi consultada agora). A prontidão mostra isso
    /// (#268).
    pub(crate) fn token_and_origin(
        &self,
        ask: Ask<'_>,
    ) -> Result<(Arc<Secret>, bool), CredentialError> {
        if let Some(e) = self.cache.lock().unwrap().get(ask.key) {
            if e.source == *ask.source {
                return Ok((e.token.clone(), true));
            }
        }
        let token = Arc::new(resolve(&ask)?);
        self.cache.lock().unwrap().insert(
            ask.key.to_string(),
            Entry {
                source: ask.source.clone(),
                token: token.clone(),
            },
        );
        if let Some(f) = &self.on_new {
            f();
        }
        Ok((token, false))
    }

    /// Descarta o token da chave: a próxima chamada resolve de novo.
    pub(crate) fn invalidate(&self, key: &str) {
        self.cache.lock().unwrap().remove(key);
    }

    /// Roda `op` com o token. Se a falha for de autenticação (`is_auth`),
    /// descarta o token, resolve de novo e tenta uma única vez. Falha do
    /// próprio comando de credencial vira `E` pelo `From`.
    pub(crate) fn with_refresh<T, E: From<CredentialError>>(
        &self,
        ask: Ask<'_>,
        is_auth: impl Fn(&E) -> bool,
        mut op: impl FnMut(&Secret) -> Result<T, E>,
    ) -> Result<T, E> {
        let token = self.token(ask)?;
        match op(&token) {
            Err(e) if is_auth(&e) => {
                drop(token);
                self.invalidate(ask.key);
                let fresh = self.token(ask)?;
                op(&fresh)
            }
            other => other,
        }
    }
}

fn resolve(ask: &Ask<'_>) -> Result<Secret, CredentialError> {
    let what = ask.what;
    match ask.source {
        CredentialSource::Command { cmd } => run_credential(what, cmd, ask.deadline),
        CredentialSource::Cli => run_credential(what, CLI_TOKEN_CMD, ask.deadline),
        CredentialSource::File { path } => read_file(what, path),
        CredentialSource::Secret { name } => (ask.lookup)(name.trim())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(Secret::new)
            .ok_or_else(|| CredentialError::Io {
                what,
                message: format!("secret {:?} não existe no ambiente", name.trim()),
            }),
        CredentialSource::None => Err(CredentialError::Missing { what }),
    }
}

/// Primeira linha não vazia do arquivo, sem espaços nas pontas, com `~` e
/// `~/...` expandidos pelo `HOME` (#274): o mesmo token que o prefixo da
/// sessão lê (`ct_core::provider::file_cmd`). O erro cita o caminho, nunca o
/// conteúdo.
fn read_file(what: &'static str, path: &str) -> Result<Secret, CredentialError> {
    read_file_in(what, path, std::env::var_os("HOME").map(PathBuf::from))
}

/// [`read_file`] com o `HOME` dado.
fn read_file_in(
    what: &'static str,
    path: &str,
    home: Option<PathBuf>,
) -> Result<Secret, CredentialError> {
    let path = path.trim();
    let full = match (path, path.strip_prefix("~/"), home) {
        ("~", _, Some(home)) => home,
        (_, Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    };
    let text = std::fs::read_to_string(full).map_err(|e| CredentialError::Io {
        what,
        message: format!("não foi possível ler {path:?}: {}", e.kind()),
    })?;
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or(CredentialError::Empty { what })?;
    Ok(Secret::new(line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const D: Duration = Duration::from_secs(5);

    fn cmd(s: &str) -> CredentialSource {
        CredentialSource::Command { cmd: s.into() }
    }

    fn ask<'a>(key: &'a str, what: &'static str, source: &'a CredentialSource) -> Ask<'a> {
        Ask {
            key,
            what,
            source,
            lookup: &no_lookup,
            deadline: D,
        }
    }

    /// Erro de teste: a recusa de autenticação, a falha e a do comando.
    #[derive(Debug, PartialEq)]
    enum Erro {
        Auth,
        Outro,
        Credencial(CredentialError),
    }

    impl From<CredentialError> for Erro {
        fn from(c: CredentialError) -> Self {
            Erro::Credencial(c)
        }
    }

    #[test]
    fn comando_roda_uma_vez_e_o_cache_serve_as_proximas() {
        let dir = tempfile::tempdir().unwrap();
        let marca = dir.path().join("marca");
        let c = cmd(&format!(
            "echo x >> {}; printf 'tok-acme-1\\n'",
            marca.display()
        ));
        let r = CredentialResolver::new();
        for _ in 0..3 {
            let t = r.token(ask("jira:1", "Jira", &c)).unwrap();
            assert_eq!(t.expose(), "tok-acme-1");
        }
        let linhas = std::fs::read_to_string(&marca).unwrap();
        assert_eq!(linhas.lines().count(), 1, "o comando rodou mais de uma vez");
    }

    #[test]
    fn fonte_nova_resolve_de_novo() {
        let r = CredentialResolver::new();
        let (a, b) = (cmd("printf 'um\\n'"), cmd("printf 'dois\\n'"));
        let x = r.token(ask("gitlab:1", "GitLab", &a)).unwrap();
        let y = r.token(ask("gitlab:1", "GitLab", &b)).unwrap();
        assert_eq!((x.expose(), y.expose()), ("um", "dois"));
    }

    #[test]
    fn auth_refaz_uma_vez_e_nao_mais() {
        let r = CredentialResolver::new();
        let c = cmd("printf 'tok-acme-2\\n'");
        let tentativas = Cell::new(0);
        let out: Result<(), Erro> = r.with_refresh(
            ask("jira:2", "Jira", &c),
            |e| *e == Erro::Auth,
            |_t| {
                tentativas.set(tentativas.get() + 1);
                Err(Erro::Auth)
            },
        );
        assert_eq!(out, Err(Erro::Auth));
        assert_eq!(tentativas.get(), 2, "uma tentativa a mais, e só");
    }

    #[test]
    fn erro_que_nao_e_auth_nao_refaz() {
        let r = CredentialResolver::new();
        let c = cmd("printf 'tok\\n'");
        let tentativas = Cell::new(0);
        let out: Result<(), Erro> = r.with_refresh(
            ask("jira:3", "Jira", &c),
            |e| *e == Erro::Auth,
            |_t| {
                tentativas.set(tentativas.get() + 1);
                Err(Erro::Outro)
            },
        );
        assert_eq!(out, Err(Erro::Outro));
        assert_eq!(tentativas.get(), 1);
    }

    #[test]
    fn arquivo_secret_e_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tok");
        std::fs::write(&p, "\n  tok-arquivo  \nresto\n").unwrap();
        let r = CredentialResolver::new();
        let src = CredentialSource::File {
            path: p.display().to_string(),
        };
        let t = r.token(ask("gh:1", "GitHub", &src)).unwrap();
        assert_eq!(t.expose(), "tok-arquivo");

        let sec = CredentialSource::Secret {
            name: "API_TOKEN_ACME".into(),
        };
        let lookup = |n: &str| (n == "API_TOKEN_ACME").then(|| "tok-secret".to_string());
        let mut a = ask("jira:9", "Jira", &sec);
        a.lookup = &lookup;
        assert_eq!(r.token(a).unwrap().expose(), "tok-secret");
        assert!(r.token(ask("jira:8", "Jira", &sec)).is_err());

        let none = CredentialSource::None;
        let e = r.token(ask("x:1", "GitLab", &none)).unwrap_err();
        assert_eq!(e, CredentialError::Missing { what: "GitLab" });
    }

    /// `~` e `~/...` expandem pelo `HOME` (#274); outro `~` no começo não.
    /// O secret vem sem espaços nas pontas, como o `$(...)` do prefixo.
    #[test]
    fn arquivo_com_til_e_secret_aparado() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("acme cfg")).unwrap();
        std::fs::write(
            home.path().join("acme cfg/it's token"),
            "\r\n \t tok-acme-til \r\nresto\n",
        )
        .unwrap();
        let h = || Some(home.path().to_path_buf());
        let t = read_file_in("Jira", " ~/acme cfg/it's token ", h()).unwrap();
        assert_eq!(t.expose(), "tok-acme-til");
        assert!(read_file_in("Jira", "~", h()).is_err(), "~ é diretório");
        let e = read_file_in("Jira", "~fulano/tok", h()).unwrap_err();
        assert!(e.to_string().contains("~fulano/tok"), "{e}");
        std::fs::write(home.path().join("branco"), " \n\t\n").unwrap();
        assert_eq!(
            read_file_in("Jira", "~/branco", h()).unwrap_err(),
            CredentialError::Empty { what: "Jira" }
        );

        let r = CredentialResolver::new();
        let sec = CredentialSource::Secret {
            name: "JIRA_TOKEN_ACME".into(),
        };
        let lookup = |_: &str| Some("  tok-acme-secret\n".to_string());
        let mut a = ask("jira:10", "Jira", &sec);
        a.lookup = &lookup;
        assert_eq!(r.token(a).unwrap().expose(), "tok-acme-secret");
        let branco = |_: &str| Some(" \n".to_string());
        a.lookup = &branco;
        r.invalidate("jira:10");
        assert!(r.token(a).is_err());
    }

    #[test]
    fn cached_nao_resolve_e_so_vale_com_a_mesma_fonte() {
        let dir = tempfile::tempdir().unwrap();
        let marca = dir.path().join("marca");
        let c = cmd(&format!(
            "echo x >> {}; printf 'tok-acme-5\\n'",
            marca.display()
        ));
        let r = CredentialResolver::new();
        assert!(r.cached("jira:5", &c).is_none());
        assert!(!marca.exists(), "cached rodou o comando");
        r.token(ask("jira:5", "Jira", &c)).unwrap();
        assert_eq!(r.cached("jira:5", &c).unwrap().expose(), "tok-acme-5");
        assert!(r.cached("jira:5", &cmd("printf 'outro\\n'")).is_none());
        assert!(r.cached("gitlab:5", &c).is_none());
    }

    #[test]
    fn token_novo_avisa_e_o_do_cache_nao() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let avisos = Arc::new(AtomicUsize::new(0));
        let n = avisos.clone();
        let r = CredentialResolver::with_on_new(move || {
            n.fetch_add(1, Ordering::SeqCst);
        });
        let c = cmd("printf 'tok-acme-6\\n'");
        r.token(ask("jira:6", "Jira", &c)).unwrap();
        r.token(ask("jira:6", "Jira", &c)).unwrap();
        assert_eq!(avisos.load(Ordering::SeqCst), 1);
        r.invalidate("jira:6");
        r.token(ask("jira:6", "Jira", &c)).unwrap();
        assert_eq!(avisos.load(Ordering::SeqCst), 2);
        // Falha ao resolver não avisa.
        let _ = r.token(ask("x:6", "GitLab", &CredentialSource::None));
        assert_eq!(avisos.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalidar_tira_do_cache() {
        let r = CredentialResolver::new();
        let c = cmd("printf 'tok-x\\n'");
        r.token(ask("jira:4", "Jira", &c)).unwrap();
        assert_eq!(r.cache.lock().unwrap().len(), 1);
        r.invalidate("jira:4");
        assert!(r.cache.lock().unwrap().is_empty());
    }
}

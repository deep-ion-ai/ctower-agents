//! Credenciais das conexões do ambiente (#236, #274, #275, #277): as fontes
//! de token de cada projeto, o resolvedor com o cache e o cache de secrets.
//! Fica fora do [`crate::Sessions`] para o sync usar sem o ciclo de vida das
//! sessões (#242); as sessões usam o mesmo resolvedor, e por isso mascaram
//! o token que o sync resolveu.

use crate::paths::Dirs;
use crate::resolver::{self, CredentialResolver};
use crate::secrets::cached_secret;
use ct_ledger::Project;
use ct_secrets::EnvCache;
use std::sync::Arc;

/// Fontes, resolvedor e cache de secrets das conexões. Clonar compartilha o
/// cache do resolvedor.
#[derive(Clone)]
pub struct Connections {
    shared: ct_core::SharedCredentials,
    dirs: Dirs,
    resolver: Arc<CredentialResolver>,
}

impl Connections {
    pub fn new(
        shared: ct_core::SharedCredentials,
        dirs: Dirs,
        resolver: Arc<CredentialResolver>,
    ) -> Self {
        Connections {
            shared,
            dirs,
            resolver,
        }
    }

    /// Conexões do hub do último pull e as sobrescritas deste ambiente
    /// ([`crate::SessionsConfig::credentials`]).
    pub fn shared(&self) -> &ct_core::SharedCredentials {
        &self.shared
    }

    /// Resolvedor de credencial do ambiente (#236).
    pub fn resolver(&self) -> &CredentialResolver {
        &self.resolver
    }

    /// Chave e cache de secrets deste ambiente.
    pub fn env_cache(&self) -> EnvCache {
        EnvCache::new(self.dirs.env_key_file(), self.dirs.secrets_cache_file())
    }

    /// Fonte do token do tipo `kind` do projeto neste ambiente (#274,
    /// #275): o comando de `[credentials]` que sobrou da conversão (#143),
    /// senão a da conexão do hub (a referenciada ou a padrão do tipo) com a
    /// sobrescrita `[connections]`, senão a de reserva do descritor
    /// ([`ct_core::SharedCredentials::source`]). `None` = nenhum lado do
    /// projeto é do tipo. Só para resolver aqui: o que vai ao hub sai do
    /// ledger, sem a sobrescrita.
    pub fn source(
        &self,
        project: &Project,
        kind: ct_core::provider::ProviderKind,
    ) -> Option<ct_core::EnvSource> {
        self.shared.source(
            &project.gh_owner,
            &project.gh_repo,
            &project.tracker,
            &project.forge,
            kind,
        )
    }

    /// As fontes de todos os tipos que o projeto usa, sem repetir o tipo
    /// (o GitHub nos dois lados conta uma vez).
    pub fn sources(
        &self,
        project: &Project,
    ) -> Vec<(ct_core::provider::ProviderKind, ct_core::EnvSource)> {
        let mut out: Vec<(ct_core::provider::ProviderKind, ct_core::EnvSource)> = Vec::new();
        for side in ct_core::provider::project_sides(&project.tracker, &project.forge) {
            if out.iter().any(|(k, _)| *k == side.kind) {
                continue;
            }
            out.extend(self.source(project, side.kind).map(|s| (side.kind, s)));
        }
        out
    }

    /// Fonte do token do Jira do projeto ([`Self::source`]). `None` = o
    /// rastreador não é Jira.
    pub fn jira_source(&self, project: &Project) -> Option<ct_core::EnvSource> {
        self.source(project, ct_core::provider::ProviderKind::Jira)
    }

    /// Fonte do token do GitLab do projeto ([`Self::source`]). `None` = a
    /// forja não é GitLab.
    pub fn gitlab_source(&self, project: &Project) -> Option<ct_core::EnvSource> {
        self.source(project, ct_core::provider::ProviderKind::Gitlab)
    }

    /// Fonte do token do GitHub do projeto ([`Self::source`]): sem a
    /// conexão no pull, o `gh auth token`. `None` = o projeto não usa o
    /// GitHub.
    pub fn github_source(&self, project: &Project) -> Option<ct_core::EnvSource> {
        self.source(project, ct_core::provider::ProviderKind::Github)
    }

    /// Token da conexão do tipo `kind` do projeto pelo resolvedor do
    /// ambiente (cache, sobrescrita `[connections]`, secret do cache): o
    /// mesmo do sync, e por isso no mascaramento das sessões. Usado no
    /// clone do GitLab por HTTPS (#281). Pode rodar o comando de credencial.
    pub fn connection_token(
        &self,
        project: &Project,
        kind: ct_core::provider::ProviderKind,
    ) -> std::result::Result<String, String> {
        let source = self
            .source(project, kind)
            .map_or(ct_core::provider::CredentialSource::None, |s| s.source);
        let lookup = self.secret_lookup(project);
        self.resolver()
            .token(resolver::Ask {
                key: &resolver::cache_key(kind, project.id),
                what: ct_core::provider::descriptor(kind).label(),
                source: &source,
                lookup: &lookup,
                deadline: ct_proc::CREDENTIAL_DEADLINE,
                cli: None,
            })
            .map(|t| t.expose().to_string())
            .map_err(|e| e.to_string())
    }

    /// As conexões do projeto neste ambiente (#277): as do último pull, com
    /// a fonte efetiva daqui ([`ct_core::SharedCredentials::resolve`]). É
    /// daqui que o sync, as sessões, os templates e os links leem a URL do
    /// Jira, o host do GitLab, o `config_file` do jira-cli e os prefixos.
    pub fn project_connections(&self, project: &Project) -> ct_core::ResolvedConnections {
        self.shared
            .resolve(
                &project.gh_owner,
                &project.gh_repo,
                &project.tracker,
                &project.forge,
            )
            .0
    }

    /// Busca de secret do resolvedor para as conexões do projeto (fonte
    /// `Secret`, #274): o valor atual no cache de secrets do ambiente, pelas
    /// regras do spawn e do `ctower secret get` ([`cached_secret`]).
    pub fn secret_lookup<'a>(
        &'a self,
        project: &'a Project,
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| cached_secret(&self.env_cache(), project, name)
    }
}

//! Última release publicada e entregas na base sem release (#355), lidas do
//! GitHub para os cards por projeto da Sala de controle. Como o resto do
//! sync, a busca não toca no `hub.db`: devolve um [`DeliveriesSnapshot`] que
//! o hub aplica numa transação, junto dos ETags.
//!
//! - **Release:** a mais recente publicada (rascunhos e pré-releases ficam
//!   de fora), pela data de publicação.
//! - **Entrega:** PR mesclado na branch base depois dessa data. A contagem é
//!   de PRs, não de commits. Sem nenhuma release, são todos os PRs mesclados
//!   na base.
//! - O PR de back-merge da branch de release para a base (o passo final da
//!   sessão de release, #204) não é entrega: sem isso, a contagem não voltaria
//!   a zero depois de publicada a release.

use crate::sync::encode;
use crate::{time, Fetched, GhClient, GhError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Itens por página das duas consultas (o máximo da API).
const PER_PAGE: usize = 100;

/// Páginas de PRs lidas, no máximo, numa rodada: 1000 PRs. Passou disso sem
/// chegar na release, a contagem sai marcada como incompleta (`truncated`).
const MAX_PULL_PAGES: usize = 10;

/// Última release publicada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ReleaseRef {
    pub tag: String,
    pub name: String,
    /// Publicação, em ms Unix.
    pub published_at: i64,
    pub url: String,
}

/// PR mesclado na base depois da última release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Delivery {
    pub number: i64,
    pub title: String,
    /// Merge, em ms Unix.
    pub merged_at: i64,
}

/// O que a busca precisa do hub, lido de uma vez (lock curto).
#[derive(Debug, Clone)]
pub struct DeliveriesInput {
    pub project_id: i64,
    pub owner: String,
    pub repo: String,
    /// Branch base do projeto (`develop` no fluxo padrão).
    pub base: String,
    /// Branch de release (`main`): o PR que sai dela para a base é o
    /// back-merge, não uma entrega.
    pub release_branch: String,
    /// ETags gravados das duas consultas.
    pub releases_etag: Option<String>,
    pub pulls_etag: Option<String>,
    /// Última release gravada, para filtrar os PRs quando só a listagem de
    /// PRs mudou.
    pub stored: Option<ReleaseRef>,
    pub now_ms: i64,
}

/// Resultado de uma rodada em que algo mudou, aplicado de uma vez.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveriesSnapshot {
    pub project_id: i64,
    pub release: Option<ReleaseRef>,
    /// Do merge mais recente ao mais antigo.
    pub deliveries: Vec<Delivery>,
    /// A leitura parou no limite de páginas antes de alcançar a release (ou
    /// de esgotar os PRs, sem release): a contagem é um mínimo.
    pub truncated: bool,
    /// `(caminho, etag)` das consultas que mudaram.
    pub etags: Vec<(String, String)>,
    pub synced_at: i64,
}

pub fn releases_path(owner: &str, repo: &str) -> String {
    format!("/repos/{owner}/{repo}/releases?per_page={PER_PAGE}")
}

/// PRs fechados com a base `base`, do mais recente atualizado ao mais antigo.
/// Um PR mesclado depois de uma data tem `updated_at` depois dela, então a
/// leitura para na primeira página que só tem PRs mais velhos que a release.
pub fn merged_pulls_path(owner: &str, repo: &str, base: &str) -> String {
    format!(
        "/repos/{owner}/{repo}/pulls?state=closed&base={}&sort=updated&direction=desc&per_page={PER_PAGE}",
        encode(base)
    )
}

/// Última release publicada da listagem: sem rascunho, sem pré-release e com
/// data de publicação; a de `published_at` mais recente.
pub fn latest_release(list: &Value) -> Option<ReleaseRef> {
    list.as_array()?
        .iter()
        .filter(|v| {
            !v.get("draft").and_then(Value::as_bool).unwrap_or(false)
                && !v
                    .get("prerelease")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .filter_map(|v| {
            let s = |k: &str| v.get(k).and_then(Value::as_str);
            Some(ReleaseRef {
                tag: s("tag_name").filter(|t| !t.is_empty())?.to_string(),
                name: s("name").unwrap_or_default().to_string(),
                published_at: time::parse_rfc3339(s("published_at")?)?,
                url: s("html_url").unwrap_or_default().to_string(),
            })
        })
        .max_by_key(|r| r.published_at)
}

/// A entrega que um PR da listagem representa, se for uma: mesclado depois
/// de `since`, e não o back-merge da branch de release para a base.
fn delivery(v: &Value, input: &DeliveriesInput, since: Option<i64>) -> Option<Delivery> {
    let merged_at = time::parse_rfc3339(v.get("merged_at")?.as_str()?)?;
    if since.is_some_and(|s| merged_at <= s) {
        return None;
    }
    let same_repo = v
        .pointer("/head/repo/full_name")
        .and_then(Value::as_str)
        .is_some_and(|f| f.eq_ignore_ascii_case(&format!("{}/{}", input.owner, input.repo)));
    let head = v.pointer("/head/ref").and_then(Value::as_str);
    // Só o `main` do próprio repositório: o de um fork é um PR comum.
    if same_repo && head == Some(input.release_branch.as_str()) {
        return None;
    }
    Some(Delivery {
        number: v.get("number")?.as_i64()?,
        title: v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        merged_at,
    })
}

/// Busca a última release e as entregas. `None` = nada mudou no GitHub
/// (as duas consultas em `304`): o que está gravado vale.
///
/// Custo por rodada com tudo igual: duas consultas condicionais, ambas em
/// `304`. Mudando a listagem de releases, os PRs são relidos inteiros (a
/// data da release mudou); mudando só a de PRs, só ela é relida, com a
/// release gravada.
pub fn fetch(
    api: &GhClient,
    input: &DeliveriesInput,
) -> Result<Option<DeliveriesSnapshot>, GhError> {
    let (o, r) = (&input.owner, &input.repo);
    let mut etags = Vec::new();

    let releases_path = releases_path(o, r);
    let (release, release_changed) =
        match api.get_one(&releases_path, input.releases_etag.as_deref())? {
            Fetched::NotModified => (input.stored.clone(), false),
            Fetched::Modified { body, etag } => {
                if let Some(e) = etag {
                    etags.push((releases_path, e));
                }
                (latest_release(&body), true)
            }
        };

    let pulls_path = merged_pulls_path(o, r, &input.base);
    // Release nova: a data mudou, e o `304` dos PRs não diz mais nada.
    let first_etag = input.pulls_etag.as_deref().filter(|_| !release_changed);
    let since = release.as_ref().map(|r| r.published_at);
    let mut deliveries = Vec::new();
    let mut truncated = false;
    for page in 1..=MAX_PULL_PAGES {
        let path = if page == 1 {
            pulls_path.clone()
        } else {
            format!("{pulls_path}&page={page}")
        };
        let body = match api.get_one(&path, if page == 1 { first_etag } else { None })? {
            // Só a primeira página tem ETag (e só sem release nova): um PR
            // que muda sobe para ela, então `304` vale para a listagem toda.
            Fetched::NotModified => return Ok(None),
            Fetched::Modified { body, etag } => {
                if let (1, Some(e)) = (page, etag) {
                    etags.push((pulls_path.clone(), e));
                }
                body
            }
        };
        let items = body.as_array().map(Vec::as_slice).unwrap_or_default();
        deliveries.extend(items.iter().filter_map(|v| delivery(v, input, since)));
        // Fim: página incompleta ou só PRs atualizados antes da release.
        let oldest_update = items
            .last()
            .and_then(|v| v.get("updated_at")?.as_str())
            .and_then(time::parse_rfc3339);
        let reached_release = matches!((since, oldest_update), (Some(s), Some(u)) if u <= s);
        if items.len() < PER_PAGE || reached_release {
            break;
        }
        truncated = page == MAX_PULL_PAGES;
    }
    Ok(Some(snapshot(input, release, deliveries, truncated, etags)))
}

fn snapshot(
    input: &DeliveriesInput,
    release: Option<ReleaseRef>,
    mut deliveries: Vec<Delivery>,
    truncated: bool,
    etags: Vec<(String, String)>,
) -> DeliveriesSnapshot {
    deliveries.sort_by_key(|d| std::cmp::Reverse((d.merged_at, d.number)));
    deliveries.dedup_by_key(|d| d.number);
    DeliveriesSnapshot {
        project_id: input.project_id,
        release,
        deliveries,
        truncated,
        etags,
        synced_at: input.now_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{fake_gh, FakeGitHub};
    use crate::GhConfig;
    use serde_json::json;

    const NOW: i64 = 1_792_000_000_000;

    fn rel(tag: &str, published: &str, draft: bool, pre: bool) -> Value {
        json!({
            "tag_name": tag, "name": format!("Versão {tag}"), "draft": draft,
            "prerelease": pre, "published_at": published,
            "html_url": format!("https://github.com/acme/acme-web/releases/tag/{tag}"),
        })
    }

    fn pr(n: i64, merged: Option<&str>, updated: &str, head: &str) -> Value {
        json!({
            "number": n, "title": format!("Entrega {n}"),
            "merged_at": merged, "updated_at": updated,
            "head": { "ref": head, "repo": { "full_name": "acme/acme-web" } },
            "base": { "ref": "develop" },
        })
    }

    struct Env {
        _d: tempfile::TempDir,
        gh: FakeGitHub,
        api: GhClient,
    }

    impl Env {
        fn new() -> Self {
            let d = tempfile::tempdir().unwrap();
            let gh = FakeGitHub::start("tok");
            let bin = fake_gh(d.path(), "tok");
            let api = GhClient::new(
                GhConfig::new(&gh.url(), bin.to_str().unwrap())
                    .with_token_timeout(crate::fake::FAKE_GH_TOKEN_TIMEOUT),
            );
            Env { _d: d, gh, api }
        }

        fn releases(&self, list: Vec<Value>) {
            self.gh
                .set(&releases_path("acme", "acme-web"), Value::Array(list));
        }

        fn pulls(&self, list: Vec<Value>) {
            self.gh.set(
                &merged_pulls_path("acme", "acme-web", "develop"),
                Value::Array(list),
            );
        }
    }

    fn input() -> DeliveriesInput {
        DeliveriesInput {
            project_id: 1,
            owner: "acme".into(),
            repo: "acme-web".into(),
            base: "develop".into(),
            release_branch: "main".into(),
            releases_etag: None,
            pulls_etag: None,
            stored: None,
            now_ms: NOW,
        }
    }

    /// Aplica o que mudou como o hub faz: os ETags e a release do snapshot
    /// viram a entrada da próxima rodada.
    fn next_input(prev: &DeliveriesInput, s: &DeliveriesSnapshot) -> DeliveriesInput {
        let etag = |p: String| {
            s.etags
                .iter()
                .find(|(k, _)| *k == p)
                .map(|(_, e)| e.clone())
        };
        DeliveriesInput {
            releases_etag: etag(releases_path("acme", "acme-web"))
                .or_else(|| prev.releases_etag.clone()),
            pulls_etag: etag(merged_pulls_path("acme", "acme-web", "develop"))
                .or_else(|| prev.pulls_etag.clone()),
            stored: s.release.clone(),
            ..prev.clone()
        }
    }

    #[test]
    fn escolhe_a_release_publicada_mais_recente() {
        let list = json!([
            rel("v3.0.0-rc.1", "2026-10-20T10:00:00Z", false, true),
            rel("v3.0.0", "", true, false),
            rel("v2.1.0", "2026-10-10T10:00:00Z", false, false),
            rel("v2.0.0", "2026-09-01T10:00:00Z", false, false),
        ]);
        let r = latest_release(&list).unwrap();
        assert_eq!(r.tag, "v2.1.0");
        assert_eq!(r.name, "Versão v2.1.0");
        assert_eq!(
            r.published_at,
            time::parse_rfc3339("2026-10-10T10:00:00Z").unwrap()
        );
        assert!(r.url.ends_with("/releases/tag/v2.1.0"));
        // Só rascunho e pré-release: sem release.
        assert_eq!(
            latest_release(&json!([rel("v1", "2026-10-10T10:00:00Z", true, false)])),
            None
        );
        assert_eq!(latest_release(&json!([])), None);
    }

    #[test]
    fn conta_os_prs_mesclados_depois_da_release() {
        let e = Env::new();
        e.releases(vec![rel("v2.1.0", "2026-10-10T10:00:00Z", false, false)]);
        e.pulls(vec![
            pr(
                30,
                Some("2026-10-14T09:00:00Z"),
                "2026-10-14T09:00:00Z",
                "feat/30-c",
            ),
            // Fechado sem merge, e mesclado antes da release.
            pr(29, None, "2026-10-13T09:00:00Z", "feat/29-b"),
            pr(
                28,
                Some("2026-10-12T09:00:00Z"),
                "2026-10-12T09:00:00Z",
                "feat/28-a",
            ),
            pr(
                27,
                Some("2026-10-09T09:00:00Z"),
                "2026-10-09T09:00:00Z",
                "feat/27-z",
            ),
        ]);
        let s = fetch(&e.api, &input()).unwrap().unwrap();
        let numbers: Vec<_> = s.deliveries.iter().map(|d| d.number).collect();
        assert_eq!(numbers, [30, 28]);
        assert_eq!(s.deliveries[0].title, "Entrega 30");
        assert_eq!(s.release.as_ref().unwrap().tag, "v2.1.0");
        assert!(!s.truncated);
        assert_eq!(s.etags.len(), 2);
    }

    #[test]
    fn sem_release_conta_todos_os_prs_mesclados() {
        let e = Env::new();
        e.releases(vec![]);
        e.pulls(vec![
            pr(
                5,
                Some("2026-10-14T09:00:00Z"),
                "2026-10-14T09:00:00Z",
                "feat/5-x",
            ),
            pr(4, None, "2026-10-13T09:00:00Z", "feat/4-x"),
            pr(
                3,
                Some("2026-01-02T09:00:00Z"),
                "2026-01-02T09:00:00Z",
                "feat/3-x",
            ),
        ]);
        let s = fetch(&e.api, &input()).unwrap().unwrap();
        assert_eq!(s.release, None);
        assert_eq!(s.deliveries.len(), 2);
    }

    /// O back-merge `main → develop` da sessão de release, mesclado depois da
    /// publicação, não é entrega; o PR de um fork com branch `main`, sim.
    #[test]
    fn back_merge_da_release_nao_e_entrega() {
        let e = Env::new();
        e.releases(vec![rel("v2.2.0", "2026-10-10T10:00:00Z", false, false)]);
        let mut fork = pr(
            41,
            Some("2026-10-12T09:00:00Z"),
            "2026-10-12T09:00:00Z",
            "main",
        );
        fork["head"]["repo"]["full_name"] = json!("fulano/acme-web");
        e.pulls(vec![
            pr(
                42,
                Some("2026-10-11T09:00:00Z"),
                "2026-10-11T09:00:00Z",
                "main",
            ),
            fork,
        ]);
        let s = fetch(&e.api, &input()).unwrap().unwrap();
        let numbers: Vec<_> = s.deliveries.iter().map(|d| d.number).collect();
        assert_eq!(numbers, [41]);
    }

    #[test]
    fn sem_mudanca_nao_ha_snapshot_e_nao_gasta_leitura_inteira() {
        let e = Env::new();
        e.releases(vec![rel("v2.1.0", "2026-10-10T10:00:00Z", false, false)]);
        e.pulls(vec![pr(
            30,
            Some("2026-10-14T09:00:00Z"),
            "2026-10-14T09:00:00Z",
            "feat/30-c",
        )]);
        let first = fetch(&e.api, &input()).unwrap().unwrap();
        let again = next_input(&input(), &first);
        assert_eq!(fetch(&e.api, &again).unwrap(), None);
        // Só as duas consultas condicionais, ambas com o ETag.
        let reqs = e.gh.requests();
        assert_eq!(reqs.len(), 4, "{reqs:?}");
        assert!(reqs[2].1.is_some() && reqs[3].1.is_some(), "{reqs:?}");
    }

    /// Mudou só a lista de PRs: a release gravada filtra, e a de releases
    /// não é relida inteira.
    #[test]
    fn so_os_prs_mudaram() {
        let e = Env::new();
        e.releases(vec![rel("v2.1.0", "2026-10-10T10:00:00Z", false, false)]);
        e.pulls(vec![pr(
            30,
            Some("2026-10-14T09:00:00Z"),
            "2026-10-14T09:00:00Z",
            "feat/30-c",
        )]);
        let first = fetch(&e.api, &input()).unwrap().unwrap();
        e.pulls(vec![
            pr(
                31,
                Some("2026-10-15T09:00:00Z"),
                "2026-10-15T09:00:00Z",
                "feat/31-d",
            ),
            pr(
                30,
                Some("2026-10-14T09:00:00Z"),
                "2026-10-14T09:00:00Z",
                "feat/30-c",
            ),
        ]);
        let s = fetch(&e.api, &next_input(&input(), &first))
            .unwrap()
            .unwrap();
        assert_eq!(s.deliveries.len(), 2);
        assert_eq!(s.release, first.release);
        assert_eq!(s.etags.len(), 1, "{:?}", s.etags);
    }

    /// A release em andamento é publicada: a listagem de releases muda, os
    /// PRs vêm inteiros de novo (o `304` deles não vale mais) e a contagem
    /// volta a zero, mesmo com o back-merge mesclado depois da publicação.
    #[test]
    fn release_publicada_zera_a_contagem() {
        let e = Env::new();
        e.releases(vec![rel("v2.1.0", "2026-10-10T10:00:00Z", false, false)]);
        let prs = vec![
            pr(
                30,
                Some("2026-10-14T09:00:00Z"),
                "2026-10-14T09:00:00Z",
                "feat/30-c",
            ),
            pr(
                29,
                Some("2026-10-12T09:00:00Z"),
                "2026-10-12T09:00:00Z",
                "feat/29-b",
            ),
        ];
        e.pulls(prs.clone());
        let before = fetch(&e.api, &input()).unwrap().unwrap();
        assert_eq!(before.deliveries.len(), 2);

        e.releases(vec![
            rel("v2.2.0", "2026-10-16T10:00:00Z", false, false),
            rel("v2.1.0", "2026-10-10T10:00:00Z", false, false),
        ]);
        // Mesmos PRs (a lista não mudou), mais o back-merge do fluxo.
        let mut with_back = vec![pr(
            31,
            Some("2026-10-16T11:00:00Z"),
            "2026-10-16T11:00:00Z",
            "main",
        )];
        with_back.extend(prs);
        e.pulls(with_back);
        let after = fetch(&e.api, &next_input(&input(), &before))
            .unwrap()
            .unwrap();
        assert_eq!(after.release.as_ref().unwrap().tag, "v2.2.0");
        assert!(after.deliveries.is_empty(), "{:?}", after.deliveries);
    }

    #[test]
    fn pagina_ate_a_release_e_marca_o_limite() {
        let e = Env::new();
        let path = merged_pulls_path("acme", "acme-web", "develop");
        let page = |from: i64| -> Value {
            Value::Array(
                (from..from + PER_PAGE as i64)
                    .map(|n| {
                        pr(
                            n,
                            Some("2026-10-14T09:00:00Z"),
                            "2026-10-14T09:00:00Z",
                            "feat/x",
                        )
                    })
                    .collect(),
            )
        };
        // Sem release e com PRs de sobra: lê o limite de páginas e avisa.
        e.releases(vec![]);
        e.gh.set(&path, page(1));
        for n in 2..=MAX_PULL_PAGES {
            e.gh.set(
                &format!("{path}&page={n}"),
                page(1 + (n as i64 - 1) * PER_PAGE as i64),
            );
        }
        let s = fetch(&e.api, &input()).unwrap().unwrap();
        assert!(s.truncated);
        assert_eq!(s.deliveries.len(), MAX_PULL_PAGES * PER_PAGE);

        // Com release, a segunda página só tem PRs mais velhos: para nela.
        e.releases(vec![rel("v2.1.0", "2026-10-10T10:00:00Z", false, false)]);
        e.gh.set(
            &format!("{path}&page=2"),
            Value::Array(vec![pr(
                900,
                Some("2026-10-01T09:00:00Z"),
                "2026-10-01T09:00:00Z",
                "feat/old",
            )]),
        );
        e.gh.set(&format!("{path}&page=3"), json!([]));
        let s = fetch(&e.api, &input()).unwrap().unwrap();
        assert!(!s.truncated);
        assert_eq!(s.deliveries.len(), PER_PAGE);
    }

    #[test]
    fn erro_do_github_cancela_a_rodada() {
        let e = Env::new();
        e.gh.fail_next(&releases_path("acme", "acme-web"), 500, &[]);
        assert!(fetch(&e.api, &input()).is_err());
    }
}

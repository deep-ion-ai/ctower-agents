//! Feedback de review dos PRs abertos dos work items (#130, épico #136):
//! as threads de review não resolvidas e os reviews vigentes que pedem
//! mudanças, pelo GraphQL (`reviewThreads` e `latestOpinionatedReviews` do
//! PR; a API REST não diz se a thread foi resolvida).
//!
//! Para não gastar cota à toa, só os PRs de [`ReviewTarget`] são lidos: os
//! abertos cujo `updated_at` no espelho mudou desde a última leitura (ou
//! nunca lidos), que o ledger escolhe (`pr_review_targets`). O
//! `updated_at` vem da listagem de PRs que o sync já faz com ETag, então o
//! critério não custa consulta nenhuma: comentário, review, push e resposta
//! numa thread mexem no `updated_at` do PR.
//!
//! Sem lock do ledger: quem chama lê os alvos, busca aqui e aplica com
//! `apply_pr_reviews`.

use crate::{time, GhClient, GhError};
use ct_ledger::{is_external_association, ChangesRequest, PrReviews, ReviewTarget, ReviewThread};
use serde_json::{json, Value};

/// Páginas de 100 threads lidas por PR, no máximo.
pub const MAX_THREAD_PAGES: usize = 10;

/// Corpo de comentário guardado, em caracteres; o resto é cortado com
/// [`TRUNCATED_MARK`].
pub const MAX_BODY_CHARS: usize = 16_000;
pub const TRUNCATED_MARK: &str = "\n[…cortado]";

/// Login de autor apagado, como o GitHub mostra.
const GHOST: &str = "ghost";

const QUERY: &str = r#"
query($owner: String!, $repo: String!, $number: Int!, $cursor: String, $withReviews: Boolean!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id isResolved isOutdated path line originalLine
          comments(first: 1) {
            totalCount
            nodes { author { login } authorAssociation body url createdAt }
          }
        }
      }
      latestOpinionatedReviews(first: 100) @include(if: $withReviews) {
        nodes { id state author { login } authorAssociation body url submittedAt }
      }
    }
  }
}"#;

/// Lê as reviews de cada alvo, em ordem. Um erro interrompe a leitura: volta
/// o que foi lido até ali (para gravar) e o erro (para o log); os PRs que
/// faltaram ficam para o próximo ciclo.
pub fn fetch_reviews(
    api: &GhClient,
    owner: &str,
    repo: &str,
    targets: &[ReviewTarget],
) -> (Vec<PrReviews>, Option<GhError>) {
    let mut out = Vec::new();
    for t in targets {
        match fetch_pr(api, owner, repo, *t) {
            Ok(r) => out.push(r),
            Err(e) => return (out, Some(e)),
        }
    }
    (out, None)
}

/// As reviews de um PR: as threads não resolvidas (todas as páginas, até
/// [`MAX_THREAD_PAGES`]) e os reviews vigentes `CHANGES_REQUESTED`.
pub fn fetch_pr(
    api: &GhClient,
    owner: &str,
    repo: &str,
    target: ReviewTarget,
) -> Result<PrReviews, GhError> {
    let mut out = PrReviews {
        number: target.number,
        pr_updated_at: target.updated_at,
        ..PrReviews::default()
    };
    let mut cursor = Value::Null;
    for page in 0..MAX_THREAD_PAGES {
        let data = api.graphql(
            QUERY,
            &json!({
                "owner": owner, "repo": repo, "number": target.number,
                "cursor": cursor, "withReviews": page == 0,
            }),
        )?;
        let pr = data
            .pointer("/repository/pullRequest")
            .filter(|p| !p.is_null())
            .ok_or_else(|| GhError::Http {
                status: 404,
                message: format!("PR #{} não encontrado", target.number),
            })?;
        if page == 0 {
            out.changes_requested = pr
                .pointer("/latestOpinionatedReviews/nodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(changes_request)
                .collect();
        }
        let threads = pr
            .get("reviewThreads")
            .ok_or_else(|| GhError::Parse("GraphQL sem reviewThreads".into()))?;
        out.threads.extend(
            threads
                .get("nodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(review_thread),
        );
        let next = threads
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match threads.pointer("/pageInfo/endCursor") {
            Some(c) if next && !c.is_null() => cursor = c.clone(),
            _ => break,
        }
    }
    out.threads
        .sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    Ok(out)
}

/// Nó de `reviewThreads` → thread do espelho; `None` para a resolvida e a
/// incompleta (sem comentário visível).
pub fn review_thread(v: &Value) -> Option<ReviewThread> {
    if v.get("isResolved").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    let comments = v.get("comments")?;
    let first = comments.get("nodes")?.as_array()?.first()?;
    let (author, author_association) = author_of(first);
    let total = comments
        .get("totalCount")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    Some(ReviewThread {
        id: v.get("id")?.as_str()?.to_string(),
        path: v.get("path")?.as_str()?.to_string(),
        line: v
            .get("line")
            .and_then(Value::as_i64)
            .or_else(|| v.get("originalLine").and_then(Value::as_i64)),
        outdated: v.get("isOutdated").and_then(Value::as_bool) == Some(true),
        external: is_external_association(&author_association),
        author,
        author_association,
        body: body_of(first),
        url: first.get("url")?.as_str()?.to_string(),
        replies: (total - 1).max(0),
        created_at: date_of(first, "createdAt"),
    })
}

/// Nó de `latestOpinionatedReviews` → pedido de mudanças; `None` para os
/// outros estados (aprovado) e o incompleto.
pub fn changes_request(v: &Value) -> Option<ChangesRequest> {
    if v.get("state").and_then(Value::as_str) != Some("CHANGES_REQUESTED") {
        return None;
    }
    let (author, author_association) = author_of(v);
    Some(ChangesRequest {
        id: v.get("id")?.as_str()?.to_string(),
        external: is_external_association(&author_association),
        author,
        author_association,
        body: body_of(v),
        url: v.get("url")?.as_str()?.to_string(),
        submitted_at: date_of(v, "submittedAt"),
    })
}

/// `(login, authorAssociation)`; autor apagado vira `ghost`, associação
/// ausente vira `NONE` (externo).
fn author_of(v: &Value) -> (String, String) {
    let login = v
        .pointer("/author/login")
        .and_then(Value::as_str)
        .filter(|l| !l.is_empty())
        .unwrap_or(GHOST);
    let association = v
        .get("authorAssociation")
        .and_then(Value::as_str)
        .filter(|a| !a.is_empty())
        .unwrap_or("NONE");
    (login.to_string(), association.to_string())
}

fn body_of(v: &Value) -> String {
    let body = v.get("body").and_then(Value::as_str).unwrap_or_default();
    if body.chars().count() <= MAX_BODY_CHARS {
        return body.to_string();
    }
    let mut cut: String = body.chars().take(MAX_BODY_CHARS).collect();
    cut.push_str(TRUNCATED_MARK);
    cut
}

fn date_of(v: &Value, field: &str) -> i64 {
    v.get(field)
        .and_then(Value::as_str)
        .and_then(time::parse_rfc3339)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{fake_gh, FakeGitHub, FAKE_GH_TOKEN_TIMEOUT};
    use crate::GhConfig;

    fn client(gh: &FakeGitHub, dir: &std::path::Path) -> GhClient {
        let bin = fake_gh(dir, "tok");
        GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(FAKE_GH_TOKEN_TIMEOUT),
        )
    }

    /// Thread montada à mão, no formato do GraphQL.
    pub(crate) fn thread(
        id: &str,
        resolved: bool,
        path: &str,
        line: Option<i64>,
        login: &str,
        association: &str,
        replies: i64,
    ) -> Value {
        json!({
            "id": id, "isResolved": resolved, "isOutdated": false,
            "path": path, "line": line, "originalLine": line,
            "comments": {
                "totalCount": replies + 1,
                "nodes": [{
                    "author": { "login": login }, "authorAssociation": association,
                    "body": format!("comentário de {login} em {path}"),
                    "url": format!("https://github.com/acme/acme-web/pull/7#discussion_{id}"),
                    "createdAt": "2026-10-15T10:00:00Z",
                }],
            },
        })
    }

    pub(crate) fn pr_response(threads: Vec<Value>, reviews: Vec<Value>) -> Value {
        json!({ "data": { "repository": { "pullRequest": {
            "reviewThreads": {
                "pageInfo": { "hasNextPage": false, "endCursor": null },
                "nodes": threads,
            },
            "latestOpinionatedReviews": { "nodes": reviews },
        }}}})
    }

    #[test]
    fn thread_resolvida_fica_de_fora_e_a_externa_vem_marcada() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        gh.set_graphql(
            json!({ "owner": "acme", "repo": "acme-web", "number": 7 }),
            pr_response(
                vec![
                    thread("T1", true, "src/a.rs", Some(3), "fulano", "MEMBER", 0),
                    thread(
                        "T2",
                        false,
                        "src/b.rs",
                        Some(42),
                        "fulano",
                        "COLLABORATOR",
                        2,
                    ),
                    thread("T3", false, "README.md", None, "beltrano", "CONTRIBUTOR", 0),
                ],
                vec![
                    json!({ "id": "R1", "state": "CHANGES_REQUESTED",
                            "author": { "login": "ciclano" }, "authorAssociation": "OWNER",
                            "body": "precisa de teste",
                            "url": "https://github.com/acme/acme-web/pull/7#pullrequestreview-1",
                            "submittedAt": "2026-10-15T11:00:00Z" }),
                    json!({ "id": "R2", "state": "APPROVED",
                            "author": { "login": "fulano" }, "authorAssociation": "MEMBER",
                            "body": "", "url": "https://github.com/acme/acme-web/pull/7#r2",
                            "submittedAt": "2026-10-15T11:00:00Z" }),
                ],
            ),
        );
        let api = client(&gh, d.path());
        let target = ReviewTarget {
            number: 7,
            updated_at: 123,
        };
        let r = fetch_pr(&api, "acme", "acme-web", target).unwrap();
        assert_eq!(r.number, 7);
        assert_eq!(r.pr_updated_at, 123);
        let ids: Vec<&str> = r.threads.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["T2", "T3"]);
        let t2 = &r.threads[0];
        assert_eq!((t2.path.as_str(), t2.line), ("src/b.rs", Some(42)));
        assert_eq!(t2.replies, 2);
        assert!(!t2.external);
        let t3 = &r.threads[1];
        assert_eq!((t3.path.as_str(), t3.line), ("README.md", None));
        assert_eq!(t3.author, "beltrano");
        assert_eq!(t3.author_association, "CONTRIBUTOR");
        assert!(t3.external);
        assert_eq!(r.changes_requested.len(), 1);
        assert_eq!(r.changes_requested[0].author, "ciclano");
        assert!(!r.changes_requested[0].external);
        // A consulta foi ao endpoint GraphQL, com o PR certo.
        let calls = gh.graphql_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["number"], 7);
        assert_eq!(calls[0]["withReviews"], true);
    }

    #[test]
    fn segue_as_paginas_de_threads() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let mut first = pr_response(
            vec![thread("T1", false, "a.rs", Some(1), "fulano", "MEMBER", 0)],
            vec![],
        );
        first["data"]["repository"]["pullRequest"]["reviewThreads"]["pageInfo"] =
            json!({ "hasNextPage": true, "endCursor": "c1" });
        gh.set_graphql(json!({ "number": 7, "cursor": null }), first);
        gh.set_graphql(
            json!({ "number": 7, "cursor": "c1" }),
            pr_response(
                vec![thread("T2", false, "b.rs", Some(2), "fulano", "MEMBER", 0)],
                vec![],
            ),
        );
        let api = client(&gh, d.path());
        let r = fetch_pr(
            &api,
            "acme",
            "acme-web",
            ReviewTarget {
                number: 7,
                updated_at: 1,
            },
        )
        .unwrap();
        assert_eq!(r.threads.len(), 2);
        let calls = gh.graphql_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1]["withReviews"], false);
    }

    #[test]
    fn erro_interrompe_e_devolve_o_que_ja_leu() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        gh.set_graphql(json!({ "number": 7 }), pr_response(vec![], vec![]));
        gh.set_graphql(
            json!({ "number": 8 }),
            json!({ "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }] }),
        );
        let api = client(&gh, d.path());
        let t = |n| ReviewTarget {
            number: n,
            updated_at: 1,
        };
        let (got, err) = fetch_reviews(&api, "acme", "acme-web", &[t(7), t(8), t(9)]);
        assert_eq!(got.len(), 1);
        assert_eq!(err, Some(GhError::RateLimited { reset_at: None }));
        // PR que não existe: o `NOT_FOUND` do GraphQL vira `404`.
        let (_, err) = fetch_reviews(&api, "acme", "acme-web", &[t(9)]);
        assert!(matches!(err, Some(GhError::Http { status: 404, .. })));
    }

    #[test]
    fn autor_apagado_e_corpo_longo() {
        let mut v = thread("T1", false, "a.rs", Some(1), "x", "NONE", 0);
        v["comments"]["nodes"][0]["author"] = Value::Null;
        v["comments"]["nodes"][0]["body"] = json!("a".repeat(MAX_BODY_CHARS + 10));
        let t = review_thread(&v).unwrap();
        assert_eq!(t.author, GHOST);
        assert!(t.external);
        assert!(t.body.ends_with(TRUNCATED_MARK));
        assert_eq!(
            t.body.chars().count(),
            MAX_BODY_CHARS + TRUNCATED_MARK.chars().count()
        );
    }

    #[test]
    fn endpoint_graphql_do_enterprise() {
        let d = tempfile::tempdir().unwrap();
        let api = |url: &str| GhClient::new(GhConfig::new(url, d.path().to_str().unwrap()));
        assert_eq!(
            api("https://api.github.com").graphql_url(),
            "https://api.github.com/graphql"
        );
        assert_eq!(
            api("https://ghe.example.com/api/v3").graphql_url(),
            "https://ghe.example.com/api/graphql"
        );
    }
}

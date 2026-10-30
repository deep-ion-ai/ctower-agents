//! Branches no GitHub para a limpeza de issues concluídas (#286): o head de
//! um PR, a ponta de uma branch e a remoção dela. Fora do sync periódico:
//! só a etapa "branch remota" chama, e só para a branch que vai sair.

use crate::{GhClient, GhError};
use serde_json::Value;

/// Percent-encoding de um caminho de branch, preservando as `/`.
fn encode_branch(branch: &str) -> String {
    branch
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn pull_path(owner: &str, repo: &str, number: i64) -> String {
    format!("/repos/{owner}/{repo}/pulls/{number}")
}

/// Leitura de uma referência (`GET .../git/ref/heads/<branch>`, singular:
/// casa só o nome exato).
pub fn branch_ref_path(owner: &str, repo: &str, branch: &str) -> String {
    format!(
        "/repos/{owner}/{repo}/git/ref/heads/{}",
        encode_branch(branch)
    )
}

/// Remoção de uma referência (`DELETE .../git/refs/heads/<branch>`).
pub fn branch_refs_path(owner: &str, repo: &str, branch: &str) -> String {
    format!(
        "/repos/{owner}/{repo}/git/refs/heads/{}",
        encode_branch(branch)
    )
}

/// SHA do head do PR (`head.sha`); `None` se o PR não existir ou o token
/// não tiver acesso. Para os PRs que entraram no espelho antes de ele
/// guardar o head.
pub fn pull_head_sha(
    api: &GhClient,
    owner: &str,
    repo: &str,
    number: i64,
) -> Result<Option<String>, GhError> {
    let Some(v) = api.get_json(&pull_path(owner, repo, number))? else {
        return Ok(None);
    };
    v.pointer("/head/sha")
        .and_then(Value::as_str)
        .map(|s| Some(s.to_string()))
        .ok_or_else(|| GhError::Parse(format!("PR #{number} sem head.sha")))
}

/// Ponta da branch no GitHub; `None` se ela não existir.
pub fn branch_head_sha(
    api: &GhClient,
    owner: &str,
    repo: &str,
    branch: &str,
) -> Result<Option<String>, GhError> {
    let Some(v) = api.get_json(&branch_ref_path(owner, repo, branch))? else {
        return Ok(None);
    };
    // Sem o nome exato, o GitHub pode devolver a lista das que começam com
    // ele: não é a branch.
    if v.get("ref").and_then(Value::as_str) != Some(&format!("refs/heads/{branch}")) {
        return Ok(None);
    }
    v.pointer("/object/sha")
        .and_then(Value::as_str)
        .map(|s| Some(s.to_string()))
        .ok_or_else(|| GhError::Parse(format!("referência {branch} sem object.sha")))
}

/// Apaga a branch no GitHub. `Ok(false)` = já não existia.
pub fn delete_branch(
    api: &GhClient,
    owner: &str,
    repo: &str,
    branch: &str,
) -> Result<bool, GhError> {
    api.delete(&branch_refs_path(owner, repo, branch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{fake_gh, FakeGitHub};
    use crate::GhConfig;
    use serde_json::json;

    fn client(gh: &FakeGitHub, dir: &std::path::Path) -> GhClient {
        let bin = fake_gh(dir, "tok");
        GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(crate::fake::FAKE_GH_TOKEN_TIMEOUT),
        )
    }

    fn git_ref(branch: &str, sha: &str) -> Value {
        json!({ "ref": format!("refs/heads/{branch}"), "object": { "sha": sha, "type": "commit" } })
    }

    #[test]
    fn caminhos() {
        assert_eq!(
            branch_ref_path("o", "r", "feat/2-x"),
            "/repos/o/r/git/ref/heads/feat/2-x"
        );
        assert_eq!(
            branch_refs_path("o", "r", "fix/3-a b"),
            "/repos/o/r/git/refs/heads/fix/3-a%20b"
        );
        assert_eq!(pull_path("o", "r", 30), "/repos/o/r/pulls/30");
    }

    #[test]
    fn head_do_pr_e_ponta_da_branch() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let c = client(&gh, d.path());
        gh.set(
            "/repos/o/r/pulls/30",
            json!({ "number": 30, "head": { "ref": "feat/2-x", "sha": "abc" } }),
        );
        assert_eq!(
            pull_head_sha(&c, "o", "r", 30).unwrap().as_deref(),
            Some("abc")
        );
        assert_eq!(pull_head_sha(&c, "o", "r", 31).unwrap(), None);
        gh.set(
            &branch_ref_path("o", "r", "feat/2-x"),
            git_ref("feat/2-x", "abc"),
        );
        assert_eq!(
            branch_head_sha(&c, "o", "r", "feat/2-x")
                .unwrap()
                .as_deref(),
            Some("abc")
        );
        assert_eq!(branch_head_sha(&c, "o", "r", "feat/9-y").unwrap(), None);
        // Resposta de outra referência não é a branch.
        gh.set(
            &branch_ref_path("o", "r", "feat"),
            git_ref("feat/2-x", "abc"),
        );
        assert_eq!(branch_head_sha(&c, "o", "r", "feat").unwrap(), None);
    }

    #[test]
    fn apaga_a_branch_e_trata_a_que_ja_saiu() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let c = client(&gh, d.path());
        gh.set(
            &branch_ref_path("o", "r", "feat/2-x"),
            git_ref("feat/2-x", "abc"),
        );
        assert!(delete_branch(&c, "o", "r", "feat/2-x").unwrap());
        assert_eq!(
            gh.calls().last().unwrap(),
            &(
                "DELETE".to_string(),
                "/repos/o/r/git/refs/heads/feat/2-x".to_string()
            )
        );
        assert_eq!(branch_head_sha(&c, "o", "r", "feat/2-x").unwrap(), None);
        // Já apagada (422 "Reference does not exist"): não é erro.
        assert!(!delete_branch(&c, "o", "r", "feat/2-x").unwrap());
        // Sem permissão: erro, com a mensagem da API.
        gh.fail_next(&branch_refs_path("o", "r", "feat/3-y"), 403, &[]);
        assert!(matches!(
            delete_branch(&c, "o", "r", "feat/3-y").unwrap_err(),
            GhError::Http { status: 403, .. }
        ));
        // Token renovado depois de um 401.
        gh.set(
            &branch_ref_path("o", "r", "feat/4-z"),
            git_ref("feat/4-z", "def"),
        );
        gh.fail_next(&branch_refs_path("o", "r", "feat/4-z"), 401, &[]);
        assert!(delete_branch(&c, "o", "r", "feat/4-z").unwrap());
    }
}

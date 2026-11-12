//! Testes das etapas da release com o GitHub falso (`acme/acme-web`).

use super::*;
use crate::fake::{fake_gh, FakeGitHub, FAKE_GH_TOKEN_TIMEOUT};
use crate::GhConfig;
use ct_core::release::ReleaseConfig;
use serde_json::{json, Value};
use std::time::Duration;

const R: &str = "/repos/acme/acme-web";

struct Gh {
    fake: FakeGitHub,
    client: GhClient,
    _dir: tempfile::TempDir,
}

fn setup() -> Gh {
    let fake = FakeGitHub::start("tok-release");
    let dir = tempfile::tempdir().unwrap();
    // O `gh` falso imprime outro token: o token fixo tem de vencer.
    let bin = fake_gh(dir.path(), "tok-do-ambiente");
    let client = GhClient::new(
        GhConfig::new(&fake.url(), bin.to_str().unwrap())
            .with_token_timeout(FAKE_GH_TOKEN_TIMEOUT)
            .with_token("tok-release".into()),
    );
    let gh = Gh {
        fake,
        client,
        _dir: dir,
    };
    gh.branch("develop", "d1", "tree-a");
    gh.branch("main", "m1", "tree-a");
    gh.compare(0, 0);
    gh.prs("develop", "main", vec![]);
    gh.prs("release-please--branches--main", "main", vec![]);
    gh.prs("main", "develop", vec![]);
    gh.fake.set(&format!("{R}/releases?per_page=20"), json!([]));
    gh.fake.set(&format!("{R}/rules/branches/main"), json!([]));
    gh.fake
        .set(&format!("{R}/rules/branches/develop"), json!([]));
    gh
}

fn pace() -> Pace {
    Pace {
        interval: Duration::from_millis(5),
        minute: Duration::from_millis(20),
    }
}

impl Gh {
    fn release(&self) -> Release<'_> {
        Release::new(&self.client, "acme", "acme-web", &cfg()).with_pace(pace())
    }

    fn branch(&self, name: &str, sha: &str, tree: &str) {
        self.fake.set(
            &format!("{R}/branches/{name}"),
            json!({ "name": name, "commit": { "sha": sha, "commit": { "tree": { "sha": tree } } } }),
        );
    }

    /// `target_ahead` = commits de main fora da develop.
    fn compare(&self, target_ahead: i64, source_ahead: i64) {
        self.fake.set(
            &format!("{R}/compare/develop...main"),
            json!({ "ahead_by": target_ahead, "behind_by": source_ahead }),
        );
    }

    fn prs(&self, head: &str, base: &str, list: Vec<Value>) {
        self.fake.set(
            &open_pulls_path("acme", "acme-web", head, base),
            json!(list),
        );
    }

    fn closed(&self, head: &str, base: &str, list: Vec<Value>) {
        self.fake.set(
            &format!(
                "{R}/pulls?state=closed&head=acme%3A{}&base={base}&sort=updated&direction=desc&per_page=20",
                head
            ),
            json!(list),
        );
    }

    fn pull(&self, n: i64, v: Value) {
        self.fake.set(&format!("{R}/pulls/{n}"), v);
    }

    fn runs(&self, sha: &str, runs: Vec<Value>) {
        self.fake.set(
            &format!("{R}/actions/runs?head_sha={sha}&per_page=100"),
            json!({ "total_count": runs.len(), "workflow_runs": runs }),
        );
    }

    fn puts(&self) -> Vec<(String, Value)> {
        let calls = self.fake.calls();
        self.fake
            .posted()
            .into_iter()
            .filter(|(p, _)| calls.iter().any(|(m, cp)| m == "PUT" && cp == p))
            .collect()
    }

    fn posted_to(&self, suffix: &str) -> Vec<Value> {
        self.fake
            .posted()
            .into_iter()
            .filter(|(p, _)| p.ends_with(suffix))
            .map(|(_, v)| v)
            .collect()
    }
}

fn cfg() -> ReleaseConfig {
    ReleaseConfig {
        enabled: true,
        ..Default::default()
    }
}

fn pr_json(n: i64, head: &str, base: &str, sha: &str, title: &str) -> Value {
    json!({
        "number": n,
        "title": title,
        "html_url": format!("https://github.com/acme/acme-web/pull/{n}"),
        "state": "open",
        "merged": false,
        "mergeable": true,
        "mergeable_state": "clean",
        "head": { "ref": head, "sha": sha },
        "base": { "ref": base },
    })
}

fn run(
    id: i64,
    name: &str,
    sha: &str,
    status: &str,
    conclusion: Option<&str>,
    attempt: i64,
) -> Value {
    json!({
        "id": id, "name": name, "head_sha": sha, "status": status,
        "conclusion": conclusion, "run_attempt": attempt,
        "html_url": format!("https://github.com/acme/acme-web/actions/runs/{id}"),
    })
}

#[test]
fn sem_diferenca_de_conteudo_nada_a_lancar() {
    let gh = setup();
    // Back-merge sozinho: main tem commits fora da develop? Não: develop
    // tem o merge, mesma árvore.
    gh.compare(0, 1);
    let st = gh.release().status().unwrap();
    assert!(!st.content_diff);
    assert_eq!(st.next, ReleaseStep::Nothing);
    let o = gh.release().promote(None, "corpo");
    assert_eq!(o.result, StepResult::Refused);
    assert!(o.message.contains("nada a lançar"), "{}", o.message);
    assert!(gh.fake.posted().is_empty());
}

#[test]
fn promote_abre_o_pr_com_o_corpo_do_agente() {
    let gh = setup();
    gh.branch("develop", "d2", "tree-b");
    gh.compare(0, 3);
    gh.fake.set(
        &format!("{R}/pulls"),
        pr_json(10, "develop", "main", "d2", "release: develop -> main"),
    );
    let st = gh.release().status().unwrap();
    assert!(st.content_diff);
    assert_eq!(st.next, ReleaseStep::Promote);
    let o = gh.release().promote(None, "## Novidades\n- x");
    assert_eq!(o.result, StepResult::Ok, "{}", o.message);
    assert_eq!(o.pr, Some(10));
    let sent = gh.posted_to("/pulls");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["head"], "develop");
    assert_eq!(sent[0]["base"], "main");
    assert_eq!(sent[0]["body"], "## Novidades\n- x");
    assert_eq!(sent[0]["title"], "release: develop -> main");

    // Idempotente: com o PR aberto, reaproveita.
    gh.prs(
        "develop",
        "main",
        vec![pr_json(10, "develop", "main", "d2", "t")],
    );
    let o = gh.release().promote(None, "");
    assert_eq!(o.result, StepResult::Noop);
    assert_eq!(gh.posted_to("/pulls").len(), 1);
}

#[test]
fn promote_recusa_com_back_merge_pendente() {
    let gh = setup();
    gh.branch("develop", "d2", "tree-b");
    gh.prs(
        "main",
        "develop",
        vec![pr_json(
            9,
            "main",
            "develop",
            "m1",
            "chore: back-merge v1.2.2",
        )],
    );
    let o = gh.release().promote(None, "x");
    assert_eq!(o.result, StepResult::Refused);
    assert!(o.message.contains("merge-back"), "{}", o.message);
    assert_eq!(o.pr, Some(9));

    // Sem o PR, mas com main à frente da develop.
    gh.prs("main", "develop", vec![]);
    gh.compare(2, 3);
    let o = gh.release().promote(None, "x");
    assert_eq!(o.result, StepResult::Refused);
    assert!(o.message.contains("back-merge"), "{}", o.message);
    assert!(gh.fake.posted().is_empty());
}

fn with_promotion(gh: &Gh, sha: &str) {
    gh.prs(
        "develop",
        "main",
        vec![pr_json(10, "develop", "main", sha, "t")],
    );
    gh.pull(10, pr_json(10, "develop", "main", sha, "t"));
    gh.fake.set(
        &format!("{R}/pulls/10/merge"),
        json!({ "merged": true, "sha": "mc1" }),
    );
    gh.fake.set(
        &format!("{R}/rules/branches/main"),
        json!([{ "type": "required_status_checks",
                 "parameters": { "required_status_checks": [{ "context": "gate" }] } }]),
    );
    gh.fake.set(
        &format!("{R}/commits/{sha}/status"),
        json!({ "state": "pending", "statuses": [] }),
    );
}

fn check_runs(gh: &Gh, sha: &str, conclusion: &str) {
    gh.fake.set(
        &format!("{R}/commits/{sha}/check-runs?per_page=100"),
        json!({ "check_runs": [{ "id": 1, "name": "gate", "status": "completed", "conclusion": conclusion }] }),
    );
}

#[test]
fn merge_promotion_usa_merge_commit_preso_ao_sha() {
    let gh = setup();
    with_promotion(&gh, "d2");
    check_runs(&gh, "d2", "success");
    let o = gh.release().merge_promotion(Some("d2"));
    assert_eq!(o.result, StepResult::Ok, "{}", o.message);
    let puts = gh.puts();
    assert_eq!(puts.len(), 1);
    assert_eq!(puts[0].0, format!("{R}/pulls/10/merge"));
    assert_eq!(puts[0].1, json!({ "merge_method": "merge", "sha": "d2" }));
}

#[test]
fn merge_recusa_head_mudado_e_check_vermelho() {
    let gh = setup();
    with_promotion(&gh, "d3d3d3d3");
    check_runs(&gh, "d3d3d3d3", "success");
    let o = gh.release().merge_promotion(Some("d2d2d2d2"));
    assert_eq!(o.result, StepResult::Refused);
    assert!(o.message.contains("mudou"), "{}", o.message);

    check_runs(&gh, "d3d3d3d3", "failure");
    let o = gh.release().merge_promotion(None);
    assert_eq!(o.result, StepResult::Refused);
    assert!(
        o.message.contains("check obrigatório vermelho"),
        "{}",
        o.message
    );
    assert!(gh.puts().is_empty());

    // O head muda entre a conferência e o merge: o GitHub responde 409.
    check_runs(&gh, "d3d3d3d3", "success");
    gh.fake.fail_next_with(
        &format!("{R}/pulls/10/merge"),
        409,
        json!({ "message": "Head branch was modified" }),
    );
    let o = gh.release().merge_promotion(None);
    assert_eq!(o.result, StepResult::Refused);
    assert!(o.message.contains("mudou"), "{}", o.message);
}

#[test]
fn merge_recusa_conflito() {
    let gh = setup();
    with_promotion(&gh, "d2");
    let mut p = pr_json(10, "develop", "main", "d2", "t");
    p["mergeable"] = json!(false);
    p["mergeable_state"] = json!("dirty");
    gh.pull(10, p);
    let o = gh.release().merge_promotion(None);
    assert_eq!(o.result, StepResult::Refused);
    assert!(o.message.contains("conflito"), "{}", o.message);
}

#[test]
fn merge_ja_feito_e_noop() {
    let gh = setup();
    let mut p = pr_json(10, "develop", "main", "d2", "t");
    p["merged_at"] = json!("2026-11-12T10:00:00Z");
    gh.closed("develop", "main", vec![p]);
    let o = gh.release().merge_promotion(None);
    assert_eq!(o.result, StepResult::Noop, "{}", o.message);
    assert_eq!(o.pr, Some(10));

    gh.closed("develop", "main", vec![]);
    let o = gh.release().merge_promotion(None);
    assert_eq!(o.result, StepResult::Refused);
    assert!(o.message.contains("promote"));
}

#[test]
fn wait_release_pr_espera_o_workflow_e_o_pr() {
    let gh = setup();
    gh.branch("main", "m2", "tree-b");
    gh.runs("m2", vec![run(50, "release", "m2", "in_progress", None, 1)]);
    let fake = gh.fake.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        fake.set(
            &open_pulls_path("acme", "acme-web", "release-please--branches--main", "main"),
            json!([pr_json(
                11,
                "release-please--branches--main",
                "main",
                "r2",
                "chore(main): release 1.2.3"
            )]),
        );
        fake.set(
            &format!("{R}/actions/runs?head_sha=m2&per_page=100"),
            json!({ "workflow_runs": [run(50, "release", "m2", "completed", Some("success"), 1)] }),
        );
    });
    let o = gh.release().wait_release_pr();
    t.join().unwrap();
    assert_eq!(o.result, StepResult::Ok, "{}", o.message);
    assert_eq!(o.pr, Some(11));
    assert_eq!(o.details["version"], "1.2.3");
}

#[test]
fn wait_release_pr_prazo_esgotado() {
    let gh = setup();
    gh.runs("m1", vec![]);
    let o = gh.release().wait_release_pr();
    assert_eq!(o.result, StepResult::Failed);
    assert_eq!(o.step, ReleaseStep::WaitReleasePr);
    assert!(o.message.contains("prazo"), "{}", o.message);
}

#[test]
fn approve_runs_aprova_espera_e_reexecuta_uma_vez() {
    let gh = setup();
    gh.pull(
        11,
        pr_json(
            11,
            "release-please--branches--main",
            "main",
            "r2",
            "chore(main): release 1.2.3",
        ),
    );
    gh.runs(
        "r2",
        vec![
            run(60, "ci", "r2", "completed", Some("action_required"), 1),
            run(61, "ui", "r2", "action_required", None, 1),
        ],
    );
    gh.fake
        .set(&format!("{R}/actions/runs/60/approve"), json!({}));
    gh.fake
        .set(&format!("{R}/actions/runs/61/approve"), json!({}));
    gh.fake
        .set(&format!("{R}/actions/runs/60/rerun-failed-jobs"), json!({}));
    // O GitHub "roda" os runs: depois de aprovados, o ci falha (corrida
    // do check de versões) e o ui passa; reexecutado, o ci passa.
    let fake = gh.fake.clone();
    let t = std::thread::spawn(move || {
        let path = format!("{R}/actions/runs?head_sha=r2&per_page=100");
        let mut stage = 0;
        for _ in 0..400 {
            std::thread::sleep(Duration::from_millis(3));
            let posted: Vec<String> = fake.posted().into_iter().map(|(p, _)| p).collect();
            if stage == 0 && posted.iter().filter(|p| p.ends_with("/approve")).count() == 2 {
                fake.set(
                    &path,
                    json!({ "workflow_runs": [
                        run(60, "ci", "r2", "completed", Some("failure"), 1),
                        run(61, "ui", "r2", "completed", Some("success"), 1),
                    ]}),
                );
                stage = 1;
            }
            if stage == 1 && posted.iter().any(|p| p.ends_with("/rerun-failed-jobs")) {
                fake.set(
                    &path,
                    json!({ "workflow_runs": [
                        run(60, "ci", "r2", "completed", Some("success"), 2),
                        run(61, "ui", "r2", "completed", Some("success"), 1),
                    ]}),
                );
                return;
            }
        }
    });
    let o = gh.release().approve_runs(11);
    t.join().unwrap();
    assert_eq!(o.result, StepResult::Ok, "{}", o.message);
    assert_eq!(o.details["approved"], json!([60, 61]));
    assert_eq!(o.details["reran"], json!([60]));

    // De novo, tudo verde: nada a fazer.
    let o = gh.release().approve_runs(11);
    assert_eq!(o.result, StepResult::Noop, "{}", o.message);
}

#[test]
fn approve_runs_desiste_depois_da_segunda_falha() {
    let gh = setup();
    gh.pull(
        11,
        pr_json(11, "release-please--branches--main", "main", "r2", "t"),
    );
    gh.runs(
        "r2",
        vec![run(60, "ci", "r2", "completed", Some("failure"), 2)],
    );
    let o = gh.release().approve_runs(11);
    assert_eq!(o.result, StepResult::Failed);
    assert!(o.message.contains("ci #60"), "{}", o.message);
}

#[test]
fn approve_runs_recusa_pr_fora_do_fluxo() {
    let gh = setup();
    gh.pull(12, pr_json(12, "feat/x", "feat/y", "x1", "t"));
    let o = gh.release().approve_runs(12);
    assert_eq!(o.result, StepResult::Refused);
    assert!(gh.fake.posted().is_empty());
}

#[test]
fn merge_release_exige_runs_aprovados_e_verdes() {
    let gh = setup();
    let head = "release-please--branches--main";
    gh.prs(
        head,
        "main",
        vec![pr_json(
            11,
            head,
            "main",
            "r2",
            "chore(main): release 1.2.3",
        )],
    );
    gh.pull(
        11,
        pr_json(11, head, "main", "r2", "chore(main): release 1.2.3"),
    );
    gh.fake.set(
        &format!("{R}/pulls/11/merge"),
        json!({ "merged": true, "sha": "mc2" }),
    );
    gh.runs("r2", vec![run(60, "ci", "r2", "action_required", None, 1)]);
    let o = gh.release().merge_release(None);
    assert_eq!(o.result, StepResult::Refused);
    assert!(o.message.contains("approve-runs 11"), "{}", o.message);
    assert!(gh.puts().is_empty());

    gh.runs(
        "r2",
        vec![run(60, "ci", "r2", "completed", Some("success"), 1)],
    );
    let o = gh.release().merge_release(Some("r"));
    // Prefixo curto demais não conta como conferência.
    assert_eq!(o.result, StepResult::Refused);
    let o = gh.release().merge_release(Some("r2"));
    assert_eq!(o.result, StepResult::Ok, "{}", o.message);
    assert_eq!(gh.puts()[0].1["merge_method"], "merge");
}

#[test]
fn wait_publish_espera_sair_do_rascunho() {
    let gh = setup();
    let head = "release-please--branches--main";
    let mut p = pr_json(11, head, "main", "r2", "chore(main): release 1.2.3");
    p["merged_at"] = json!("2026-11-12T10:00:00Z");
    p["merge_commit_sha"] = json!("mc2");
    gh.closed(head, "main", vec![p.clone()]);
    gh.pull(11, p);
    gh.runs(
        "mc2",
        vec![run(70, "release", "mc2", "in_progress", None, 1)],
    );
    gh.fake.set(
        &format!("{R}/releases?per_page=20"),
        json!([{ "tag_name": "v1.2.3", "name": "v1.2.3", "html_url": "https://github.com/acme/acme-web/releases/tag/v1.2.3", "draft": true }]),
    );
    let fake = gh.fake.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        fake.set(
            &format!("{R}/releases?per_page=20"),
            json!([{ "tag_name": "v1.2.3", "name": "v1.2.3", "html_url": "https://github.com/acme/acme-web/releases/tag/v1.2.3", "draft": false }]),
        );
    });
    let o = gh.release().wait_publish();
    t.join().unwrap();
    assert_eq!(o.result, StepResult::Ok, "{}", o.message);
    assert!(o.message.contains("v1.2.3"));
    // Já publicada: noop.
    let o = gh.release().wait_publish();
    assert_eq!(o.result, StepResult::Noop);
}

#[test]
fn wait_publish_relata_job_que_falhou() {
    let gh = setup();
    let head = "release-please--branches--main";
    let mut p = pr_json(11, head, "main", "r2", "chore(main): release 1.2.3");
    p["merged_at"] = json!("2026-11-12T10:00:00Z");
    p["merge_commit_sha"] = json!("mc2");
    gh.closed(head, "main", vec![p.clone()]);
    gh.pull(11, p);
    gh.runs(
        "mc2",
        vec![run(70, "release", "mc2", "completed", Some("failure"), 1)],
    );
    gh.fake.set(
        &format!("{R}/releases?per_page=20"),
        json!([{ "tag_name": "v1.2.3", "name": "v1.2.3", "html_url": "u", "draft": true }]),
    );
    let o = gh.release().wait_publish();
    assert_eq!(o.result, StepResult::Failed);
    assert!(o.message.contains("release #70"), "{}", o.message);
}

#[test]
fn merge_back_aprova_e_mescla_com_merge_commit() {
    let gh = setup();
    gh.compare(2, 0);
    let p = pr_json(12, "main", "develop", "mc2abcd", "chore: back-merge v1.2.3");
    gh.prs("main", "develop", vec![p.clone()]);
    gh.pull(12, p);
    gh.fake.set(
        &format!("{R}/pulls/12/merge"),
        json!({ "merged": true, "sha": "bm1" }),
    );
    gh.runs(
        "mc2abcd",
        vec![run(80, "ci", "mc2abcd", "completed", Some("success"), 1)],
    );
    let o = gh.release().merge_back(None);
    assert_eq!(o.result, StepResult::Ok, "{}", o.message);
    assert_eq!(o.step, ReleaseStep::MergeBack);
    let puts = gh.puts();
    assert_eq!(puts.len(), 1);
    assert_eq!(puts[0].0, format!("{R}/pulls/12/merge"));
    assert_eq!(
        puts[0].1,
        json!({ "merge_method": "merge", "sha": "mc2abcd" })
    );

    // Depois do merge: sem PR e develop contém a main.
    gh.prs("main", "develop", vec![]);
    gh.compare(0, 0);
    let o = gh.release().merge_back(None);
    assert_eq!(o.result, StepResult::Noop, "{}", o.message);
}

#[test]
fn nenhuma_etapa_usa_squash_rebase_nem_bypass() {
    let gh = setup();
    with_promotion(&gh, "d2");
    check_runs(&gh, "d2", "success");
    gh.release().merge_promotion(None);
    for (path, body) in gh.fake.posted() {
        let s = body.to_string();
        assert!(
            !s.contains("squash") && !s.contains("rebase"),
            "{path}: {s}"
        );
        assert!(!path.contains("/git/refs"), "{path}");
    }
}

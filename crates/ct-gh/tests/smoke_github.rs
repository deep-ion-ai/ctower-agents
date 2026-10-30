//! Smoke contra o GitHub real (somente leitura), usando o `gh` logado:
//! confere o formato da API para issues, sub-issues, PRs e ETag no próprio
//! repositório do Control Tower.
//!
//! Rodar: cargo test -p ct-gh --test smoke_github -- --ignored --nocapture

use ct_core::{PrState, WorkItemKind, WorkItemStatus};
use ct_gh::sync::{fetch, SyncInput};
use ct_gh::{GhClient, GhConfig};
use ct_ledger::{Ledger, NewProject};

#[test]
#[ignore = "acessa api.github.com com o gh logado"]
fn smoke_com_github_real() {
    let api = GhClient::new(GhConfig::new("https://api.github.com", "gh"));
    let l = Ledger::open_in_memory().unwrap();
    let p = l
        .add_project(&NewProject {
            name: "ctower-agents".into(),
            gh_owner: "deep-ion-ai".into(),
            gh_repo: "ctower-agents".into(),
            repo_path: "/x".into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into(), "main".into()],
        })
        .unwrap();
    // Épico do Plano 2, entregue pelo PR #15.
    let wi = l.ensure_work_item(p.id, 4, WorkItemKind::Epic).unwrap();
    l.set_work_item_branch(wi.id, "feat/plano-2-sessoes-servidor", "/x")
        .unwrap();

    let now = ct_core::now_ms();
    let snap = fetch(&api, &SyncInput::load(&l, p.id, now).unwrap()).unwrap();
    println!(
        "issues: {}, prs: {}, etags: {}",
        snap.issues.len(),
        snap.prs.len(),
        snap.etags.len()
    );
    let applied = l.apply_github(&snap).unwrap();
    println!("aplicado: {applied:?}");

    let issues = l.list_issues(p.id).unwrap();
    let epico = issues
        .iter()
        .find(|i| i.gh_number == 4)
        .expect("#4 no cache");
    assert!(epico.has_sub_issues, "{epico:?}");
    for n in 5..=13 {
        let i = issues
            .iter()
            .find(|i| i.gh_number == n)
            .unwrap_or_else(|| panic!("#{n} no cache"));
        assert_eq!(i.parent_number, Some(4), "#{n}");
    }
    assert!(issues.iter().all(|i| i.gh_number != 15), "PR fora do cache");
    let prs = l.prs_for_work_item(wi.id).unwrap();
    let pr15 = prs.iter().find(|p| p.number == 15).expect("PR #15");
    assert_eq!(pr15.state, PrState::Merged);
    assert_eq!(pr15.base, "develop");
    assert_eq!(
        l.get_work_item(wi.id).unwrap().status,
        WorkItemStatus::Concluido
    );

    // Segunda rodada: as listagens voltam 304.
    let again = fetch(&api, &SyncInput::load(&l, p.id, now).unwrap()).unwrap();
    println!(
        "segunda rodada: issues {}, etags {}",
        again.issues.len(),
        again.etags.len()
    );
    assert!(again.issues.is_empty() && again.etags.is_empty());
}

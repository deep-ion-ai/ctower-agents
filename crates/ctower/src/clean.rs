//! `ctower clean [--dry-run] [--item <n>] [--repo <owner/repo>]` (issue
//! #287): limpeza do ambiente após a conclusão do work item, via ctowerd
//! (`POST /cleanup/plan` e `POST /cleanup/run`). Vale mesmo com o job de
//! verificação desligado.

use ct_sessions::cleanup::{
    human_bytes, CleanupItem, CleanupPlan, CleanupRun, StepResult, StepState,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;

/// Espera máxima de uma execução (apagar worktrees grandes demora).
const RUN_TIMEOUT: Duration = Duration::from_secs(30 * 60);

fn post<T: DeserializeOwned>(path: &str, body: Value) -> Result<T, String> {
    let (url, token) = crate::status::daemon()?;
    let agent = ureq::AgentBuilder::new().timeout(RUN_TIMEOUT).build();
    let resp = agent
        .post(&format!("{url}{path}"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string());
    let body = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, r)) => {
            return Err(r.into_string().unwrap_or_default().trim().to_string())
        }
        Err(e) => return Err(format!("ctowerd fora do ar ({e})")),
    };
    serde_json::from_str(&body).map_err(|e| format!("resposta inválida do ctowerd: {e}"))
}

fn item_name(owner: &str, repo: &str, n: i64) -> String {
    format!("{owner}/{repo}#{n}")
}

/// Work items do plano com o número `n` (e o repositório, se dado).
fn select<'a>(plan: &'a CleanupPlan, n: i64, repo: Option<&str>) -> Vec<&'a CleanupItem> {
    plan.items
        .iter()
        .filter(|i| i.gh_number == n)
        .filter(|i| repo.is_none_or(|r| r.eq_ignore_ascii_case(&format!("{}/{}", i.owner, i.repo))))
        .collect()
}

/// O work item de `--item`, ou erro se não estiver no plano ou se for ambíguo.
fn resolve(plan: &CleanupPlan, n: i64, repo: Option<&str>) -> Result<i64, String> {
    match select(plan, n, repo).as_slice() {
        [one] => Ok(one.work_item_id),
        [] => Err(format!(
            "#{n} não está no plano de limpeza (não concluído nem com target/ inativo, recente demais ou já limpo)"
        )),
        many => Err(format!(
            "#{n} existe em mais de um projeto ({}); escolha com --repo <owner/repo>",
            many.iter()
                .map(|i| format!("{}/{}", i.owner, i.repo))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

const HEADER: [&str; 5] = ["ITEM", "ETAPA", "ESTADO", "ESPAÇO", "MOTIVO / ALVOS"];

/// Tabela alinhada (a última coluna fica livre).
fn table(rows: &[[String; 5]]) -> String {
    let mut w = HEADER.map(str::len);
    for r in rows {
        for (i, c) in r.iter().enumerate().take(4) {
            w[i] = w[i].max(c.chars().count());
        }
    }
    let line = |r: [&str; 5]| {
        let mut s = String::new();
        for (i, c) in r.iter().enumerate().take(4) {
            s.push_str(c);
            s.push_str(&" ".repeat(w[i] - c.chars().count() + 2));
        }
        s.push_str(r[4]);
        s.trim_end().to_string()
    };
    let mut out = vec![line(HEADER)];
    out.extend(
        rows.iter()
            .map(|r| line([&r[0], &r[1], &r[2], &r[3], &r[4]])),
    );
    out.join("\n")
}

fn plan_rows(items: &[&CleanupItem]) -> Vec<[String; 5]> {
    let mut rows = Vec::new();
    for it in items {
        let mut first = true;
        for s in it.steps.iter().filter(|s| s.state != StepState::Feita) {
            let (estado, espaco, detalhe) = match s.state {
                StepState::Pendente => ("pendente", human_bytes(s.bytes), s.targets.join(", ")),
                _ => (
                    "bloqueada",
                    "-".to_string(),
                    s.reason.clone().unwrap_or_default(),
                ),
            };
            rows.push([
                if first {
                    item_name(&it.owner, &it.repo, it.gh_number)
                } else {
                    String::new()
                },
                s.kind.label().to_string(),
                estado.to_string(),
                espaco,
                detalhe,
            ]);
            first = false;
        }
    }
    rows
}

fn run_rows(run: &CleanupRun) -> Vec<[String; 5]> {
    let mut rows = Vec::new();
    for it in &run.items {
        for (i, s) in it.steps.iter().enumerate() {
            let estado = match s.result {
                StepResult::Feita => "feita",
                StepResult::Simulada => "simulada",
                StepResult::Bloqueada => "bloqueada",
                StepResult::Falhou => "falhou",
            };
            let espaco = match s.result {
                StepResult::Feita | StepResult::Simulada => human_bytes(s.bytes),
                _ => "-".into(),
            };
            let mut detalhe: Vec<String> = s.reason.iter().cloned().collect();
            detalhe.extend(s.removed.iter().cloned());
            detalhe.extend(s.kept.iter().map(|k| format!("ficou: {k}")));
            rows.push([
                if i == 0 {
                    item_name(&it.owner, &it.repo, it.gh_number)
                } else {
                    String::new()
                },
                s.kind.label().to_string(),
                estado.to_string(),
                espaco,
                detalhe.join("; "),
            ]);
        }
    }
    rows
}

/// `--dry-run`: o plano (recalculado agora), com os pendentes e os
/// bloqueados com o motivo. Nada é apagado.
fn dry_run(item: Option<i64>, repo: Option<&str>) -> Result<(), String> {
    let plan: CleanupPlan = post("/cleanup/plan", json!({}))?;
    let items: Vec<&CleanupItem> = match item {
        Some(n) => select(&plan, n, repo),
        None => plan
            .items
            .iter()
            .filter(|i| {
                repo.is_none_or(|r| r.eq_ignore_ascii_case(&format!("{}/{}", i.owner, i.repo)))
            })
            .collect(),
    };
    if let (Some(n), true) = (item, items.is_empty()) {
        return Err(resolve(&plan, n, repo).unwrap_err());
    }
    if items.is_empty() {
        println!("nenhum work item concluído com limpeza por fazer");
        return Ok(());
    }
    println!("{}", table(&plan_rows(&items)));
    let pending: Vec<&&CleanupItem> = items.iter().filter(|i| i.has_pending()).collect();
    let bytes: u64 = pending.iter().map(|i| i.pending_bytes()).sum();
    println!();
    if pending.is_empty() {
        println!("nenhuma limpeza pendente (dry-run: nada foi apagado)");
    } else {
        println!(
            "{} limpeza(s) pendente(s) · liberaria {} (dry-run: nada foi apagado)",
            pending.len(),
            human_bytes(bytes)
        );
    }
    Ok(())
}

/// Executa as etapas pendentes (do work item, com `--item`). Sai com 1 se
/// alguma etapa falhou.
fn run(item: Option<i64>, repo: Option<&str>) -> Result<(), String> {
    let mut body = json!({});
    if let Some(n) = item {
        let plan: CleanupPlan = post("/cleanup/plan", json!({}))?;
        body["work_item_id"] = json!(resolve(&plan, n, repo)?);
    }
    let r: CleanupRun = post("/cleanup/run", body)?;
    let rows = run_rows(&r);
    if rows.is_empty() {
        println!("nada a limpar");
        return Ok(());
    }
    println!("{}", table(&rows));
    println!();
    println!(
        "{} etapa(s) feita(s), {} bloqueada(s), {} com falha · liberado: {}",
        r.count(StepResult::Feita),
        r.count(StepResult::Bloqueada),
        r.count(StepResult::Falhou),
        human_bytes(r.freed_bytes)
    );
    if r.count(StepResult::Falhou) > 0 {
        return Err("alguma etapa falhou; o resto do work item ficou para a próxima".into());
    }
    Ok(())
}

pub fn clean(dry: bool, item: Option<i64>, repo: Option<String>) -> i32 {
    let r = if dry {
        dry_run(item, repo.as_deref())
    } else {
        run(item, repo.as_deref())
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("erro: {e}");
            1
        }
    }
}

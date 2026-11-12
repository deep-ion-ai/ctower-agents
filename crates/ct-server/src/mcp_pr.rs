//! `pr_feedback` e `pr_check_log` do MCP do ledger (#133, épico #136): o
//! agente lê sozinho os checks e o review do PR do work item da sessão.
//!
//! `pr_feedback` lê só o espelho (`pr_checks`, `pr_review_item`), sem
//! chamar o GitHub. `pr_check_log` baixa na hora o trecho final do log de
//! um check, com o token do GitHub do ambiente, mas só de um check que
//! está no espelho para aquele PR: não serve para ler logs arbitrários.
//!
//! Confiança: log e comentário são dados de terceiros e só aparecem no
//! campo `text`, entre os delimitadores de [`ct_ledger::pr_feedback_text`].
//! Os itens estruturados trazem só os metadados, e o comentário externo
//! nunca traz o texto.

use ct_core::{change_label, ForgeKind, PrState};
use ct_ledger::{
    check_log_text, pr_feedback_text, FeedbackTextOptions, Ledger, PrCheck, PrRow, Project,
    Session, WorkItem,
};
use ct_sessions::{GithubSync, Sessions};
use serde_json::{json, Value};

/// Trecho do log devolvido por padrão (bytes).
pub(crate) const LOG_DEFAULT_BYTES: usize = 24 * 1024;
/// Teto do trecho do log (bytes).
pub(crate) const LOG_MAX_BYTES: usize = 32 * 1024;
const LOG_MIN_BYTES: usize = 1024;

fn err(e: ct_ledger::LedgerError) -> String {
    e.to_string()
}

/// Projeto, work item e o PR da sessão: o aberto mais recente ou, sem
/// nenhum aberto, o mais recente.
fn session_pr(l: &Ledger, s: &Session) -> Result<(Project, WorkItem, Option<PrRow>), String> {
    let wi = l.get_work_item(s.work_item_id).map_err(err)?;
    let p = l.get_project(wi.project_id).map_err(err)?;
    let prs = l.prs_for_work_item(wi.id).map_err(err)?;
    let pr = prs
        .iter()
        .find(|p| p.state == PrState::Open)
        .or(prs.first())
        .cloned();
    Ok((p, wi, pr))
}

fn no_pr(p: &Project, wi: &WorkItem) -> Value {
    let label = change_label(p.forge.kind());
    let message = format!(
        "sem {label}: o work item {} ainda não tem {label} no espelho do Control Tower (o sync lê \
         os {label}s da branch {} a cada ciclo; um {label} recém-aberto aparece no próximo).",
        p.item_ref(wi.gh_number),
        wi.branch.as_deref().unwrap_or("do work item"),
    );
    json!({ "has_pr": false, "message": message, "text": message })
}

fn pr_json(pr: &PrRow) -> Value {
    json!({
        "number": pr.number,
        "url": pr.url,
        "state": pr.state,
        "head": pr.head,
        "base": pr.base,
        "head_sha": pr.head_sha,
    })
}

fn check_json(c: &PrCheck) -> Value {
    json!({
        "name": c.name,
        "source": c.source,
        "state": c.state(),
        "conclusion": c.conclusion,
        "url": c.url,
        "job_id": c.job_id,
    })
}

/// `pr_feedback`: checks e review do PR da sessão, do espelho.
pub(crate) fn pr_feedback(l: &Ledger, s: &Session) -> Result<Value, String> {
    let (p, wi, pr) = session_pr(l, s)?;
    let Some(pr) = pr else {
        return Ok(no_pr(&p, &wi));
    };
    let open = pr.state == PrState::Open;
    // Fechado ou mesclado: o espelho não guarda mais os checks.
    let checks = if open {
        l.pr_checks(p.id, pr.number).map_err(err)?
    } else {
        None
    };
    let feedback = l
        .pr_feedback(p.id, pr.number)
        .map_err(err)?
        .filter(|f| f.read_at_updated.is_some());
    let sync = l.sync_state(p.id).map_err(err)?;
    let now = ct_core::now_ms();
    let synced_at = sync.as_ref().and_then(|s| s.last_ok_at);
    let text = pr_feedback_text(
        &pr,
        checks.as_ref(),
        feedback.as_ref(),
        &FeedbackTextOptions::default(),
    );
    let mut notes = Vec::new();
    if !open {
        notes.push(format!(
            "o {} está {}: o espelho só acompanha checks e review dos abertos",
            change_label(p.forge.kind()),
            pr.state.as_str()
        ));
    }
    if p.forge.kind() != ForgeKind::Github {
        notes.push("checks e review do GitLab ainda não entram no espelho".to_string());
    }
    let checks_json = checks.as_ref().map(|c| {
        json!({
            "head_sha": c.head_sha,
            // Os checks são de outro head (o push novo ainda não foi lido).
            "stale": pr.head_sha.as_deref().is_some_and(|h| h != c.head_sha),
            "state": c.state,
            "summary": c.summary().text,
            "completed": c.completed(),
            "changed_at": c.synced_at,
            "items": c.checks.iter().map(check_json).collect::<Vec<_>>(),
        })
    });
    let review_json = feedback.as_ref().map(|f| {
        json!({
            "threads": f.threads.iter().map(|t| json!({
                "path": t.path,
                "line": t.line,
                "outdated": t.outdated,
                "author": t.author,
                "author_association": t.author_association,
                "external": t.external,
                "replies": t.replies,
                "url": t.url,
            })).collect::<Vec<_>>(),
            "changes_requested": f.changes_requested.iter().map(|c| json!({
                "author": c.author,
                "author_association": c.author_association,
                "external": c.external,
                "url": c.url,
            })).collect::<Vec<_>>(),
            "external_bodies_omitted": f.threads.iter().filter(|t| t.external).count()
                + f.changes_requested.iter().filter(|c| c.external).count(),
        })
    });
    Ok(json!({
        "has_pr": true,
        "pr": pr_json(&pr),
        // Idade da leitura: a última rodada bem-sucedida do sync do
        // projeto. Compare `pr.head_sha` com o seu último push.
        "mirror": {
            "synced_at": synced_at,
            "age_secs": synced_at.map(|t| (now - t).max(0) / 1000),
            "last_error": sync.as_ref().and_then(|s| s.last_error.clone()),
        },
        "checks": checks_json,
        "review": review_json,
        "notes": notes,
        "text": text,
    }))
}

/// O check pedido entre os do espelho: pelo `job_id` (número) ou pelo
/// nome; com nomes repetidos, o falho; ainda ambíguo, erro com os `job_id`.
fn find_check<'a>(checks: &'a [PrCheck], wanted: &Wanted) -> Result<&'a PrCheck, String> {
    let names = || {
        checks
            .iter()
            .map(|c| match c.job_id {
                Some(j) => format!("{} (job_id {j})", c.name),
                None => c.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let by_job = |id: i64| checks.iter().find(|c| c.job_id == Some(id));
    let found = match wanted {
        Wanted::Job(id) => by_job(*id),
        Wanted::Name(name) => {
            let same: Vec<&PrCheck> = checks.iter().filter(|c| c.name == *name).collect();
            let failed: Vec<&PrCheck> = same
                .iter()
                .copied()
                .filter(|c| c.state() == ct_ledger::ChecksState::Failure)
                .collect();
            let pick = if failed.is_empty() { same } else { failed };
            match pick.as_slice() {
                [] => name.trim().parse::<i64>().ok().and_then(by_job),
                [one] => Some(*one),
                many => {
                    let jobs = many
                        .iter()
                        .map(|c| c.job_id.map_or("sem job_id".into(), |j| j.to_string()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(format!(
                        "há mais de um check `{name}` neste PR: escolha pelo job_id ({jobs})"
                    ));
                }
            }
        }
    };
    found.ok_or_else(|| {
        format!(
            "check fora do espelho deste PR: {}. Os checks do head são: {}",
            match wanted {
                Wanted::Job(id) => format!("job_id {id}"),
                Wanted::Name(n) => format!("`{n}`"),
            },
            if checks.is_empty() {
                "nenhum".into()
            } else {
                names()
            }
        )
    })
}

enum Wanted {
    Job(i64),
    Name(String),
}

fn wanted(args: &Value) -> Result<Wanted, String> {
    if let Some(id) = args.get("job_id").and_then(Value::as_i64) {
        return Ok(Wanted::Job(id));
    }
    match args.get("check") {
        Some(Value::Number(n)) => n
            .as_i64()
            .map(Wanted::Job)
            .ok_or_else(|| "check inválido".to_string()),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Wanted::Name(s.clone())),
        _ => Err("parâmetro obrigatório: check (nome do check ou job_id)".into()),
    }
}

/// `pr_check_log`: trecho final do log de um check do PR da sessão, lido
/// na hora no GitHub Actions. Fora do `call_tool`: a rede corre sem
/// segurar o ledger.
pub(crate) fn pr_check_log(
    sessions: &Sessions,
    github: Option<&GithubSync>,
    s: &Session,
    args: &Value,
) -> Result<Value, String> {
    let wanted = wanted(args)?;
    let max_bytes = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map_or(LOG_DEFAULT_BYTES, |n| n as usize)
        .clamp(LOG_MIN_BYTES, LOG_MAX_BYTES);
    let (p, pr, check) = {
        let l = sessions.ledger();
        let (p, wi, pr) = session_pr(&l, s)?;
        let Some(pr) = pr.filter(|pr| pr.state == PrState::Open) else {
            let mut v = no_pr(&p, &wi);
            if let Some(m) = v["message"].as_str() {
                let m = format!("{m} Sem PR aberto não há checks para ler.");
                v["message"] = json!(m);
                v["text"] = json!(m);
            }
            return Ok(v);
        };
        let checks = l
            .pr_checks(p.id, pr.number)
            .map_err(err)?
            .map(|c| c.checks)
            .unwrap_or_default();
        let check = find_check(&checks, &wanted)?.clone();
        (p, pr, check)
    };
    let Some(job_id) = check.job_id else {
        let message = format!(
            "o check `{}` é um commit status externo, fora do GitHub Actions: não há log para \
             baixar. Veja a página do check{}.",
            check.name,
            check
                .url
                .as_deref()
                .map(|u| format!(" ({u})"))
                .unwrap_or_default()
        );
        return Ok(json!({
            "has_log": false,
            "pr": pr_json(&pr),
            "check": check_json(&check),
            "message": message,
            "text": message,
        }));
    };
    if p.forge.kind() != ForgeKind::Github {
        return Err("pr_check_log só vale para projetos no GitHub".into());
    }
    let gh = github.ok_or("integração com o GitHub desativada neste ambiente")?;
    let tail = gh
        .client()
        .job_log_tail(&p.gh_owner, &p.gh_repo, job_id, max_bytes)
        .map_err(|e| format!("falha ao ler o log do job {job_id}: {e}"))?;
    let Some(tail) = tail else {
        let message = format!(
            "o log do check `{}` (job {job_id}) não existe mais no GitHub ou o token do ambiente \
             não tem acesso a ele.",
            check.name
        );
        return Ok(json!({
            "has_log": false,
            "pr": pr_json(&pr),
            "check": check_json(&check),
            "message": message,
            "text": message,
        }));
    };
    Ok(json!({
        "has_log": true,
        "pr": pr_json(&pr),
        "check": check_json(&check),
        "bytes": tail.len(),
        "max_bytes": max_bytes,
        "text": check_log_text(pr.number, &check, &tail),
    }))
}

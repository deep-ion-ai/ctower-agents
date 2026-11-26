//! Partes puras da análise de dependências do piloto automático (#215):
//! a janela da rodada, a detecção de ciclos e os textos que vão à sessão de
//! análise (o prompt inicial e o pedido de reanálise). A regra fica em
//! [`super::plan`].

use super::plan::{ItemSnap, ProjectSnap};
use ct_core::plan::budget::{BlockReason, BudgetDecision};
use ct_core::session_templates::AnalysisVars;
use ct_core::{WorkItemKind, WorkItemStatus};
use std::collections::{BTreeMap, BTreeSet};

/// Issues listadas no pedido de reanálise; o resto vai na próxima.
pub const REANALYSIS_MAX_ITEMS: usize = 50;

/// A janela da rodada está aberta: a faixa atual da política permite
/// iniciar (o corte de início, o teto da 5h e a semanal só impedem
/// inícios, não fecham a janela).
pub fn window_open(d: &BudgetDecision) -> bool {
    !d.reasons
        .iter()
        .any(|r| matches!(r, BlockReason::BandClosed { .. } | BlockReason::NoBand))
}

/// Issues que fazem parte de algum ciclo no grafo `issue → bloqueantes`
/// (inclusive a que bloqueia a si mesma). Tarjan iterativo, em ordem.
pub fn cycle_members(edges: &BTreeMap<i64, BTreeSet<i64>>) -> BTreeSet<i64> {
    let mut nodes: BTreeSet<i64> = edges.keys().copied().collect();
    for bs in edges.values() {
        nodes.extend(bs.iter().copied());
    }
    let empty = BTreeSet::new();
    let next = |n: i64| edges.get(&n).unwrap_or(&empty);
    let mut index: BTreeMap<i64, usize> = BTreeMap::new();
    let mut low: BTreeMap<i64, usize> = BTreeMap::new();
    let mut on_stack: BTreeSet<i64> = BTreeSet::new();
    let mut stack: Vec<i64> = Vec::new();
    let mut out = BTreeSet::new();
    let mut counter = 0usize;
    for &root in &nodes {
        if index.contains_key(&root) {
            continue;
        }
        // Pilha de chamadas: (nó, vizinhos ainda por visitar).
        let mut calls: Vec<(i64, Vec<i64>)> = Vec::new();
        index.insert(root, counter);
        low.insert(root, counter);
        counter += 1;
        stack.push(root);
        on_stack.insert(root);
        calls.push((root, next(root).iter().rev().copied().collect()));
        while let Some((v, pending)) = calls.last_mut() {
            let v = *v;
            if let Some(w) = pending.pop() {
                if let std::collections::btree_map::Entry::Vacant(e) = index.entry(w) {
                    e.insert(counter);
                    low.insert(w, counter);
                    counter += 1;
                    stack.push(w);
                    on_stack.insert(w);
                    calls.push((w, next(w).iter().rev().copied().collect()));
                } else if on_stack.contains(&w) {
                    let lw = index[&w];
                    let lv = low.get_mut(&v).expect("visitado");
                    *lv = (*lv).min(lw);
                }
                continue;
            }
            calls.pop();
            if let Some((parent, _)) = calls.last() {
                let lv = low[&v];
                let lp = low.get_mut(parent).expect("visitado");
                *lp = (*lp).min(lv);
            }
            if low[&v] == index[&v] {
                let mut comp = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack.remove(&w);
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                if comp.len() > 1 || next(v).contains(&v) {
                    out.extend(comp);
                }
            }
        }
    }
    out
}

fn line(i: &ItemSnap) -> String {
    let title = i.title.as_deref().unwrap_or("(fora do espelho)");
    match i.priority {
        Some(p) => format!("- #{} {title} ({})", i.number, p.as_str().to_uppercase()),
        None => format!("- #{} {title}", i.number),
    }
}

fn lines<'a>(items: impl Iterator<Item = &'a ItemSnap>) -> String {
    let v: Vec<String> = items.map(line).collect();
    if v.is_empty() {
        "(nenhuma)".into()
    } else {
        v.join("\n")
    }
}

/// O prompt inicial da sessão de análise do projeto: o template
/// `analysis` do projeto (ou o padrão) com a fila, as em andamento e as
/// arestas já gravadas.
pub fn analysis_prompt(p: &ProjectSnap, candidates: &[&ItemSnap]) -> String {
    let in_progress = lines(p.items.iter().filter(|i| {
        i.kind == WorkItemKind::Issue
            && !matches!(
                i.status,
                WorkItemStatus::Backlog | WorkItemStatus::Concluido
            )
    }));
    let deps: Vec<String> = p
        .deps
        .iter()
        .filter(|e| !e.removed)
        .map(|e| {
            let origin = if e.source == ct_hub_store::DEP_SOURCE_MANUAL {
                "manual"
            } else {
                "análise"
            };
            let why = if e.reason.is_empty() {
                String::new()
            } else {
                format!(": {}", e.reason)
            };
            format!(
                "- #{} bloqueada por #{} ({origin}){why}",
                e.number, e.blocker
            )
        })
        .chain(p.deps.iter().filter(|e| e.removed).map(|e| {
            format!(
                "- #{} NÃO depende de #{} (removida pela pessoa)",
                e.number, e.blocker
            )
        }))
        .collect();
    let deps = if deps.is_empty() {
        "(nenhuma)".to_string()
    } else {
        deps.join("\n")
    };
    AnalysisVars {
        owner: &p.owner,
        repo: &p.repo,
        integration_branch: &p.integration,
        issues: &lines(candidates.iter().copied()),
        in_progress: &in_progress,
        dependencies: &deps,
        tracker_hint: &p.tracker_hint,
    }
    .render(p.templates.analysis_template())
}

/// O pedido de reanálise das issues novas ou alteradas (no máximo
/// [`REANALYSIS_MAX_ITEMS`]), pelo `session.continue`.
pub fn reanalysis_text(items: &[&ItemSnap]) -> String {
    format!(
        "Piloto automático do Control Tower: estas issues da fila são novas ou mudaram (corpo, \
         rótulos ou prioridade):\n{}\nLeia-as de novo no rastreador, reanalise só o necessário \
         e grave com autopilot_set_dependencies, incluindo cada uma delas em items (com a lista \
         vazia se nada a bloqueia). Não implemente nada.",
        lines(items.iter().copied())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::Priority;

    fn g(edges: &[(i64, &[i64])]) -> BTreeMap<i64, BTreeSet<i64>> {
        edges
            .iter()
            .map(|(n, bs)| (*n, bs.iter().copied().collect()))
            .collect()
    }

    #[test]
    fn ciclos() {
        assert!(cycle_members(&g(&[])).is_empty());
        // Cadeia sem ciclo.
        assert!(cycle_members(&g(&[(3, &[2]), (2, &[1])])).is_empty());
        // 1 → 2 → 3 → 1, e 4 depende do ciclo sem fazer parte.
        assert_eq!(
            cycle_members(&g(&[(1, &[2]), (2, &[3]), (3, &[1]), (4, &[1])])),
            BTreeSet::from([1, 2, 3])
        );
        // Laço próprio e dois ciclos separados.
        assert_eq!(
            cycle_members(&g(&[(5, &[5]), (6, &[7]), (7, &[6]), (8, &[6])])),
            BTreeSet::from([5, 6, 7])
        );
        // Grafo maior em cadeia: nada de recursão profunda.
        let long: BTreeMap<i64, BTreeSet<i64>> =
            (2..5000).map(|n| (n, BTreeSet::from([n - 1]))).collect();
        assert!(cycle_members(&long).is_empty());
    }

    #[test]
    fn texto_de_reanalise() {
        let i = ItemSnap {
            number: 12,
            title: Some("ACME-12".into()),
            kind: WorkItemKind::Issue,
            status: WorkItemStatus::Backlog,
            priority: Some(Priority::P1),
            model: Some("opus".into()),
            approved: true,
            fingerprint: String::new(),
        };
        let t = reanalysis_text(&[&i]);
        assert!(t.contains("- #12 ACME-12 (P1)"), "{t}");
        assert!(t.contains("autopilot_set_dependencies"), "{t}");
        assert!(t.len() < ct_core::autonomy::CONTINUE_TEXT_MAX_BYTES);
    }
}

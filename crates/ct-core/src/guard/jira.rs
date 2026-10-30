//! Perfil Jira do guarda-corpo (spec do Jira e GitLab §7.4, issue #351),
//! para sessões com `CTOWER_TRACKER=jira`. Soma-se ao de branch protegida e
//! ao do glab. O Control Tower lê, cria issues e comenta no Jira, sem
//! transição de status: bloqueia transição e edição, pede confirmação
//! (`Verdict::Ask`) para criar issue e épico (no exemplo, criar issue exige o ok
//! do usuário) e libera leitura e comentário. Como o resto do guarda-corpo, é
//! uma barreira contra erro, não contra adversário.
//!
//! O hook não roda o jira-cli nem avalia o `$(...)` do comando de
//! credencial que o agente passa no próprio comando
//! (`JIRA_API_TOKEN="$(pass ...)" jira ...`): olha só o argv.

use super::tokenize::split_segments;
use super::{skip_env_assignments, Verdict};

const MAX_DEPTH: u8 = 3;

/// Veredito do perfil Jira para um comando do shell.
pub fn check_jira(cmd: &str) -> Verdict {
    check_segments(cmd, 0, false)
}

/// Como [`check_jira`], com o perfil do gestor: o gestor também não comenta
/// no Jira (spec do gestor §10.2, "não comente issues").
pub(crate) fn check_jira_gestor(args: &[String]) -> Verdict {
    jira_verdict(args, true)
}

fn check_segments(cmd: &str, depth: u8, gestor: bool) -> Verdict {
    split_segments(cmd)
        .iter()
        .map(|seg| check_segment(seg, depth, gestor))
        .fold(Verdict::Allow, Verdict::max)
}

fn check_segment(words: &[String], depth: u8, gestor: bool) -> Verdict {
    let words = skip_env_assignments(words);
    let words: Vec<&String> = words
        .iter()
        .skip_while(|w| {
            matches!(
                w.as_str(),
                "{" | "}" | "!" | "then" | "do" | "else" | "elif" | "if"
            )
        })
        .collect();
    let Some(first) = words.first() else {
        return Verdict::Allow;
    };
    let prog = first.rsplit('/').next().unwrap_or(first);
    let rest: Vec<String> = words[1..].iter().map(|w| w.to_string()).collect();
    match prog {
        "jira" => jira_verdict(&rest, gestor),
        "bash" | "sh" | "zsh" | "dash" if depth < MAX_DEPTH => {
            match rest
                .iter()
                .position(|w| w.starts_with('-') && w.ends_with('c'))
            {
                Some(i) => rest.get(i + 1).map_or(Verdict::Allow, |inner| {
                    check_segments(inner, depth + 1, gestor)
                }),
                None => Verdict::Allow,
            }
        }
        "sudo" | "env" | "command" | "exec" | "nohup" | "time" | "nice" if depth < MAX_DEPTH => {
            let rest: Vec<String> = rest
                .into_iter()
                .skip_while(|w| w.starts_with('-'))
                .collect();
            check_segment(&rest, depth + 1, gestor)
        }
        _ => Verdict::Allow,
    }
}

/// Argumentos posicionais do jira-cli, pulando os valores das opções
/// globais (`-c/--config`, `-p/--project`). As opções dos subcomandos vêm
/// depois do subcomando, então não atrapalham os dois primeiros.
fn positional(args: &[String]) -> Vec<&str> {
    let mut pos = Vec::new();
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            "-c" | "--config" | "-p" | "--project" => {
                it.next();
            }
            _ if a.starts_with('-') => {}
            _ => pos.push(a),
        }
    }
    pos
}

/// Subcomandos de `jira issue` que mudam a issue (com os apelidos do
/// jira-cli 1.7.0): transição, edição, exclusão, atribuição, ligação,
/// clone, worklog e observadores.
const ISSUE_WRITES: &[&str] = &[
    "move",
    "transition",
    "mv",
    "edit",
    "update",
    "modify",
    "delete",
    "remove",
    "rm",
    "del",
    "assign",
    "asg",
    "link",
    "ln",
    "unlink",
    "uln",
    "clone",
    "worklog",
    "watch",
    "wat",
];

/// `jira epic add|remove` (ligar e desligar issues do épico) e apelidos.
const EPIC_WRITES: &[&str] = &["add", "assign", "remove", "rm", "unassign"];

/// `jira sprint add|close` e apelidos.
const SPRINT_WRITES: &[&str] = &["add", "assign", "close", "complete"];

fn jira_verdict(args: &[String], gestor: bool) -> Verdict {
    let pos = positional(args);
    let group = pos.first().copied().map(|g| g.trim_end_matches('s'));
    let sub = pos.get(1).copied();
    let block = |what: &str| {
        Verdict::Block(format!(
            "`jira {what}` é bloqueado: o Control Tower não faz transição de status nem \
             edita issues no Jira; peça ao usuário para fazer isso no Jira"
        ))
    };
    match (group, sub) {
        (Some("issue"), Some("create")) => Verdict::Ask(
            "criar issue no Jira exige o ok do usuário: confirme se você aprovou esta issue \
             na conversa"
                .into(),
        ),
        (Some("epic"), Some("create")) => Verdict::Ask(
            "criar épico no Jira exige o ok do usuário: confirme se você aprovou este épico \
             na conversa"
                .into(),
        ),
        (Some("issue"), Some("comment")) if gestor => Verdict::Block(
            "o gestor não comenta issues (`jira issue comment`): proponha na conversa".into(),
        ),
        (Some("issue"), Some(s)) if ISSUE_WRITES.contains(&s) => block(&format!("issue {s}")),
        (Some("epic"), Some(s)) if EPIC_WRITES.contains(&s) => block(&format!("epic {s}")),
        (Some("sprint"), Some(s)) if SPRINT_WRITES.contains(&s) => block(&format!("sprint {s}")),
        _ => Verdict::Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(cmd: &str) -> bool {
        matches!(check_jira(cmd), Verdict::Ask(_))
    }

    fn block(cmd: &str) -> bool {
        matches!(check_jira(cmd), Verdict::Block(_))
    }

    const TOKEN: &str = "JIRA_API_TOKEN=\"$(pass show jira | head -1)\"";

    /// Critério de aceite da #351: transição, edição, exclusão, atribuição
    /// e ligação são bloqueadas, com apelidos, prefixo de token e wrappers.
    #[test]
    fn transicao_e_edicao_sao_bloqueadas() {
        for cmd in [
            "jira issue move PROJ-5450 'Em andamento'",
            "jira issue transition PROJ-5450 Done",
            "jira issue mv PROJ-5450 Done",
            "jira issue edit PROJ-5450 -s 'novo'",
            "jira issue update PROJ-5450 -s x",
            "jira issue delete PROJ-5450",
            "jira issue rm PROJ-5450",
            "jira issue assign PROJ-5450 fulano",
            "jira issue link PROJ-5450 PROJ-1 Blocks",
            "jira issue unlink PROJ-5450 PROJ-1",
            "jira issue clone PROJ-5450",
            "jira issue worklog add PROJ-5450 2h",
            "jira issues move PROJ-5450 Done",
            "jira epic add PROJ-5014 PROJ-5450",
            "jira epic remove PROJ-5450",
            "jira sprint add 12 PROJ-5450",
            "jira -p PROJ issue move PROJ-5450 Done",
            "jira -c /tmp/c.yml issue edit PROJ-5450",
            "/opt/homebrew/bin/jira issue move PROJ-5450 Done",
            "cargo test && jira issue move PROJ-5450 Done",
            "bash -c 'jira issue move PROJ-5450 Done'",
            "env JIRA_CONFIG_FILE=/x jira issue move PROJ-5450 Done",
        ] {
            assert!(block(cmd), "{cmd}: {:?}", check_jira(cmd));
        }
        let cmd = format!("{TOKEN} jira issue move PROJ-5450 Done");
        assert!(block(&cmd), "{cmd}");
        let Verdict::Block(m) = check_jira("jira issue move PROJ-5450 Done") else {
            unreachable!()
        };
        assert!(m.contains("transição de status"), "{m}");
    }

    #[test]
    fn criar_issue_e_epico_pedem_confirmacao() {
        for cmd in [
            "jira issue create -p PROJ -t Tarefa -s 'Nova' -b 'corpo' -P PROJ-5014 --no-input",
            "jira issue create -t Sub-tarefa -s x -P PROJ-5450 --no-input",
            "jira epic create -n 'Épico' -s 'Épico' --no-input",
            &format!("{TOKEN} jira issue create -s x --no-input"),
        ] {
            assert!(ask(cmd), "{cmd}: {:?}", check_jira(cmd));
        }
        // Bloqueio vence a confirmação no mesmo comando.
        assert!(block(
            "jira issue create -s x && jira issue move PROJ-1 Done"
        ));
    }

    #[test]
    fn leitura_e_comentario_passam() {
        for cmd in [
            "jira issue view PROJ-5450 --comments 20 --plain",
            "jira issue list -p PROJ --plain",
            "jira epic list PROJ-5014 --plain",
            "jira issue comment add PROJ-5450 'MR aberto: https://git/x/-/merge_requests/3' --no-input",
            "jira me",
            "jira serverinfo",
            "jira version",
            "echo 'jira issue move PROJ-1 Done'",
            "git commit -m 'jira issue move'",
        ] {
            assert_eq!(check_jira(cmd), Verdict::Allow, "{cmd}");
        }
        let cmd = format!("{TOKEN} jira issue comment add PROJ-5450 'MR aberto: x' --no-input");
        assert_eq!(check_jira(&cmd), Verdict::Allow);
    }

    #[test]
    fn gestor_nao_comenta() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert!(matches!(
            check_jira_gestor(&args("issue comment add PROJ-1 oi")),
            Verdict::Block(_)
        ));
        assert!(matches!(
            check_jira_gestor(&args("issue create -s x")),
            Verdict::Ask(_)
        ));
        assert_eq!(
            check_jira_gestor(&args("issue view PROJ-1 --plain")),
            Verdict::Allow
        );
    }
}

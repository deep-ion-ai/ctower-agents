//! JSON do Jira → linha do espelho (spec §6.3, §6.4).

use ct_core::{JiraConfig, JiraTypeRole};
use ct_ledger::IssueRow;
use serde_json::Value;

/// Item da listagem (`issue list --raw`): só o que decide se a issue
/// precisa ser lida de novo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub key: String,
    pub number: i64,
    /// `fields.updated` em ms; `None` = ausente ou ilegível (lê de novo).
    pub updated_at: Option<i64>,
}

/// Item da listagem → chave, número e `updated`. `None` para a chave de
/// outro projeto (issue movida), que sai do espelho.
pub fn list_entry(v: &Value, cfg: &JiraConfig) -> Option<ListEntry> {
    let key = v.get("key")?.as_str()?;
    let number = cfg.number_from_key(key)?;
    Some(ListEntry {
        key: key.to_string(),
        number,
        updated_at: v
            .pointer("/fields/updated")
            .and_then(Value::as_str)
            .and_then(parse_jira_time),
    })
}

/// Issue da API v2 (`issue view --raw`) → linha do cache. `None` para a
/// chave de outro projeto, o tipo com papel `ignore` e o objeto incompleto.
///
/// - sub-task (`issuetype.subtask`) é sempre sub-issue do pai
///   (`fields.parent`), nunca épico;
/// - o papel vem do `type_map` pelo nome do tipo; sem entrada, é `epic` se o
///   Epic Name está preenchido (só épicos têm) e `issue` senão;
/// - `bug` e `chore` viram labels, como no GitHub (prefixo da branch);
/// - o pai é o Epic Link; sem ele, `fields.parent` (Jira com épico por
///   `parent`);
/// - fechada = categoria do status `done` (o nome do status vem traduzido);
/// - as sprints vêm do `sprint_field` (vazio = detectado, ver [`crate::sprint`]);
/// - os metadados são os campos padrão ([`crate::meta::STANDARD_FIELDS`]) e
///   depois os campos próprios do cadastro (`extra_fields`).
pub fn issue_row(v: &Value, cfg: &JiraConfig) -> Option<IssueRow> {
    let key = v.get("key")?.as_str()?;
    let number = cfg.number_from_key(key)?;
    let f = v.get("fields")?;
    let ty = f.get("issuetype");
    let type_name = ty
        .and_then(|t| t.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let subtask = ty
        .and_then(|t| t.get("subtask"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mapped = cfg.type_map.get(type_name).copied();
    if mapped == Some(JiraTypeRole::Ignore) {
        return None;
    }
    let field = |name: &str| -> Option<&Value> {
        (!name.is_empty())
            .then(|| f.get(name))
            .flatten()
            .filter(|v| !v.is_null())
    };
    let role = match mapped {
        // A sub-task nunca é épico: fica como issue, com o label do papel.
        Some(JiraTypeRole::Epic) if subtask => JiraTypeRole::Issue,
        Some(r) => r,
        None if !subtask && field(&cfg.epic_name_field).is_some() => JiraTypeRole::Epic,
        None => JiraTypeRole::Issue,
    };

    let mut labels: Vec<String> = f
        .get("labels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let extra = match role {
        JiraTypeRole::Bug => Some("bug"),
        JiraTypeRole::Chore => Some("chore"),
        _ => None,
    };
    if let Some(l) = extra {
        if !labels.iter().any(|x| x == l) {
            labels.push(l.to_string());
        }
    }

    let parent_key = || {
        f.get("parent")
            .and_then(|p| p.get("key"))
            .and_then(Value::as_str)
            .and_then(|k| cfg.number_from_key(k))
    };
    let parent_number = if subtask {
        parent_key()
    } else {
        field(&cfg.epic_link_field)
            .and_then(Value::as_str)
            .and_then(|k| cfg.number_from_key(k))
            .or_else(parent_key)
    }
    // Ninguém é pai de si mesmo (dado inconsistente faria um ciclo na árvore).
    .filter(|p| *p != number);

    let has_subtasks = f
        .get("subtasks")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());

    let done = f
        .pointer("/status/statusCategory/key")
        .and_then(Value::as_str)
        == Some("done");

    Some(IssueRow {
        gh_number: number,
        title: f.get("summary")?.as_str()?.to_string(),
        state: if done { "closed" } else { "open" }.to_string(),
        labels,
        parent_number,
        has_sub_issues: role == JiraTypeRole::Epic || has_subtasks,
        url: format!("{}/browse/{key}", cfg.url.trim_end_matches('/')),
        updated_at: f
            .get("updated")
            .and_then(Value::as_str)
            .and_then(parse_jira_time)
            .unwrap_or(0),
        sprints: crate::sprint::issue_sprints(f, &cfg.sprint_field),
        meta: crate::meta::issue_meta(f, &cfg.extra_fields),
    })
}

/// Data do Jira (`2026-10-28T10:15:30.000-0300`; também aceita `Z` e
/// `-03:00`, com ou sem os milissegundos) → ms Unix.
pub fn parse_jira_time(s: &str) -> Option<i64> {
    ct_core::parse_iso8601(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::default_type_map;
    use serde_json::json;

    fn cfg() -> JiraConfig {
        JiraConfig {
            url: "https://jira.example.com/".into(),
            project_key: "PROJ".into(),
            epic_link_field: "customfield_10601".into(),
            epic_name_field: "customfield_10604".into(),
            type_map: default_type_map(),
            ..JiraConfig::default()
        }
    }

    fn issue(key: &str, ty: &str, subtask: bool, cat: &str, extra: Value) -> Value {
        let mut f = json!({
            "summary": format!("Resumo de {key}"),
            "issuetype": { "name": ty, "subtask": subtask },
            "status": { "name": "Aberto", "statusCategory": { "id": 2, "key": cat } },
            "labels": [],
            "updated": "2026-10-28T10:15:30.000-0300",
            "customfield_10601": null,
            "customfield_10604": null,
            "subtasks": [],
        });
        for (k, v) in extra.as_object().unwrap() {
            f[k] = v.clone();
        }
        json!({ "key": key, "fields": f })
    }

    #[test]
    fn datas_do_jira() {
        // 2026-10-28T13:15:30Z
        let utc = 1_793_193_330_000;
        assert_eq!(parse_jira_time("2026-10-28T10:15:30.000-0300"), Some(utc));
        assert_eq!(
            parse_jira_time("2026-10-28T10:15:30.250-03:00"),
            Some(utc + 250)
        );
        assert_eq!(parse_jira_time("2026-10-28T13:15:30Z"), Some(utc));
        assert_eq!(parse_jira_time("2026-10-28T14:15:30+0100"), Some(utc));
        assert_eq!(parse_jira_time("ontem"), None);
        assert_eq!(parse_jira_time("2026-10-28T10:15:30.000-03"), None);
    }

    #[test]
    fn tarefa_com_epic_link_aberta() {
        let v = issue(
            "PROJ-5450",
            "Tarefa",
            false,
            "indeterminate",
            json!({ "customfield_10601": "PROJ-5014", "labels": ["fonetica"] }),
        );
        let r = issue_row(&v, &cfg()).unwrap();
        assert_eq!(r.gh_number, 5450);
        assert_eq!(r.title, "Resumo de PROJ-5450");
        assert_eq!(r.state, "open");
        assert_eq!(r.labels, vec!["fonetica"]);
        assert_eq!(r.parent_number, Some(5014));
        assert!(!r.has_sub_issues);
        assert_eq!(r.url, "https://jira.example.com/browse/PROJ-5450");
        assert_eq!(r.updated_at, 1_793_193_330_000);
        assert!(r.sprints.is_empty());
    }

    /// Os 5 campos padrão, na ordem da tela, com o texto de cada um.
    #[test]
    fn metadados_dos_campos_padrao() {
        let v = issue(
            "PROJ-9",
            "Tarefa",
            false,
            "indeterminate",
            json!({
                "reporter": { "name": "beltrano", "displayName": "Beltrano Silva" },
                "assignee": { "name": "fulano", "displayName": "Fulano de Tal" },
                "priority": { "name": "Alta", "id": "2" },
            }),
        );
        let meta: Vec<_> = issue_row(&v, &cfg())
            .unwrap()
            .meta
            .into_iter()
            .map(|m| (m.field, m.label, m.value))
            .collect();
        let m = |f: &str, l: &str, v: &str| (f.to_string(), l.to_string(), v.to_string());
        assert_eq!(
            meta,
            vec![
                m("status", "Situação", "Aberto"),
                m("issuetype", "Tipo", "Tarefa"),
                m("priority", "Prioridade", "Alta"),
                m("assignee", "Responsável", "Fulano de Tal"),
                m("reporter", "Criador", "Beltrano Silva"),
            ]
        );
    }

    /// Campo nulo ou ausente fica de fora; os demais mantêm a ordem.
    #[test]
    fn metadados_sem_os_vazios() {
        let v = issue(
            "PROJ-9",
            "Tarefa",
            false,
            "new",
            json!({ "assignee": null, "reporter": { "name": "beltrano" } }),
        );
        let fields: Vec<_> = issue_row(&v, &cfg())
            .unwrap()
            .meta
            .into_iter()
            .map(|m| (m.field, m.value))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("status".to_string(), "Aberto".to_string()),
                ("issuetype".to_string(), "Tarefa".to_string()),
                ("reporter".to_string(), "beltrano".to_string()),
            ]
        );
    }

    /// Campos do cadastro: customfield de usuário e de option, com o rótulo
    /// cadastrado, depois dos padrão; o vazio fica de fora.
    #[test]
    fn metadados_com_campos_do_projeto() {
        let v = issue(
            "PROJ-9",
            "Tarefa",
            false,
            "indeterminate",
            json!({
                "assignee": { "name": "qa", "displayName": "Quem Testa" },
                "customfield_10099": { "name": "dev", "displayName": "Quem Implementa", "active": true },
                "customfield_11000": { "self": "x", "value": "Alta", "id": "10001" },
                "customfield_11001": null,
            }),
        );
        let cfg = JiraConfig {
            extra_fields: vec![
                ct_core::JiraExtraField {
                    id: "customfield_10099".into(),
                    label: "Responsável pela implementação".into(),
                },
                ct_core::JiraExtraField {
                    id: "customfield_11001".into(),
                    label: "Vazio".into(),
                },
                ct_core::JiraExtraField {
                    id: "customfield_11000".into(),
                    label: "Criticidade".into(),
                },
            ],
            ..cfg()
        };
        let meta: Vec<_> = issue_row(&v, &cfg)
            .unwrap()
            .meta
            .into_iter()
            .map(|m| (m.field, m.label, m.value))
            .collect();
        let m = |f: &str, l: &str, v: &str| (f.to_string(), l.to_string(), v.to_string());
        assert_eq!(
            meta,
            vec![
                m("status", "Situação", "Aberto"),
                m("issuetype", "Tipo", "Tarefa"),
                m("assignee", "Responsável", "Quem Testa"),
                m(
                    "customfield_10099",
                    "Responsável pela implementação",
                    "Quem Implementa"
                ),
                m("customfield_11000", "Criticidade", "Alta"),
            ]
        );
    }

    /// Cadastro anterior, sem `extra_fields`: só os campos padrão.
    #[test]
    fn cadastro_anterior_sem_campos_do_projeto() {
        let raw = json!({
            "url": "https://jira.example.com",
            "project_key": "PROJ",
            "jql": "project = PROJ",
            "credential_cmd": "",
            "config_file": "",
            "epic_link_field": "",
            "epic_name_field": "",
            "type_map": {},
        });
        let cfg: JiraConfig = serde_json::from_value(raw).unwrap();
        assert!(cfg.extra_fields.is_empty());
        let v = issue(
            "PROJ-9",
            "Tarefa",
            false,
            "new",
            json!({ "customfield_10099": { "displayName": "Quem Implementa" } }),
        );
        let fields: Vec<_> = issue_row(&v, &cfg)
            .unwrap()
            .meta
            .into_iter()
            .map(|m| m.field)
            .collect();
        assert_eq!(fields, vec!["status", "issuetype"]);
    }

    #[test]
    fn sprints_pelo_campo_do_cadastro_ou_detectado() {
        let gh = "com.atlassian.greenhopper.service.sprint.Sprint@1[id=7,rapidViewId=2,state=ACTIVE,name=Sprint 48,startDate=<null>,endDate=<null>,completeDate=<null>,sequence=7,goal=]";
        let v = issue(
            "PROJ-10",
            "Tarefa",
            false,
            "new",
            json!({ "customfield_10600": [gh] }),
        );
        let r = issue_row(&v, &cfg()).unwrap();
        assert_eq!(r.sprints.len(), 1, "detectado");
        assert_eq!(r.sprints[0].name, "Sprint 48");
        let mut c = cfg();
        c.sprint_field = "customfield_10999".into();
        assert!(issue_row(&v, &c).unwrap().sprints.is_empty());
        c.sprint_field = "customfield_10600".into();
        assert_eq!(issue_row(&v, &c).unwrap().sprints[0].id, 7);
    }

    #[test]
    fn epico_pelo_mapa_ou_pelo_epic_name() {
        let r = issue_row(&issue("PROJ-1", "Épico", false, "new", json!({})), &cfg()).unwrap();
        assert!(r.has_sub_issues && r.parent_number.is_none());
        // Tipo fora do mapa, com Epic Name: épico.
        let v = issue(
            "PROJ-2",
            "Iniciativa",
            false,
            "new",
            json!({ "customfield_10604": "Fonética" }),
        );
        assert!(issue_row(&v, &cfg()).unwrap().has_sub_issues);
        // Story com sub-tasks: épico no Control Tower.
        let v = issue(
            "PROJ-3",
            "História",
            false,
            "new",
            json!({ "subtasks": [{ "key": "PROJ-4" }] }),
        );
        assert!(issue_row(&v, &cfg()).unwrap().has_sub_issues);
    }

    #[test]
    fn sub_task_e_filha_do_parent() {
        let v = issue(
            "PROJ-4",
            "Sub-tarefa",
            true,
            "new",
            json!({ "parent": { "key": "PROJ-3" }, "customfield_10604": "x" }),
        );
        let r = issue_row(&v, &cfg()).unwrap();
        assert_eq!(r.parent_number, Some(3));
        assert!(!r.has_sub_issues, "sub-task nunca é épico");
    }

    #[test]
    fn done_fecha_pela_categoria_e_nao_pelo_nome() {
        let mut v = issue("PROJ-9", "Tarefa", false, "done", json!({}));
        v["fields"]["status"]["name"] = json!("Concluído");
        assert_eq!(issue_row(&v, &cfg()).unwrap().state, "closed");
        let mut v = issue("PROJ-9", "Tarefa", false, "indeterminate", json!({}));
        v["fields"]["status"]["name"] = json!("Done");
        assert_eq!(issue_row(&v, &cfg()).unwrap().state, "open");
    }

    #[test]
    fn papeis_bug_chore_e_ignore() {
        let mut c = cfg();
        c.type_map.insert("Melhoria".into(), JiraTypeRole::Chore);
        c.type_map
            .insert("Impedimento".into(), JiraTypeRole::Ignore);
        let r = issue_row(&issue("PROJ-5", "Erro", false, "new", json!({})), &c).unwrap();
        assert_eq!(r.labels, vec!["bug"]);
        let v = issue(
            "PROJ-6",
            "Melhoria",
            false,
            "new",
            json!({ "labels": ["chore"] }),
        );
        assert_eq!(issue_row(&v, &c).unwrap().labels, vec!["chore"]);
        assert!(issue_row(&issue("PROJ-7", "Impedimento", false, "new", json!({})), &c).is_none());
    }

    #[test]
    fn outra_chave_e_descartada() {
        assert!(issue_row(&issue("OUTRO-5", "Tarefa", false, "new", json!({})), &cfg()).is_none());
        assert!(issue_row(&issue("PROJ-X", "Tarefa", false, "new", json!({})), &cfg()).is_none());
        // Epic Link para outro projeto: sem pai.
        let v = issue(
            "PROJ-8",
            "Tarefa",
            false,
            "new",
            json!({ "customfield_10601": "OUTRO-1" }),
        );
        assert_eq!(issue_row(&v, &cfg()).unwrap().parent_number, None);
        let e = list_entry(
            &json!({ "key": "PROJ-8", "fields": { "updated": "2026-10-28T13:15:30Z" } }),
            &cfg(),
        )
        .unwrap();
        assert_eq!((e.number, e.updated_at), (8, Some(1_793_193_330_000)));
        assert!(list_entry(&json!({ "key": "OUTRO-8" }), &cfg()).is_none());
    }
}

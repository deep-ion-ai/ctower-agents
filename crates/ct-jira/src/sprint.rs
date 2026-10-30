//! Campo da sprint do Jira Software → `Vec<Sprint>`.
//!
//! O valor do campo é uma lista, com as sprints fechadas por onde a issue
//! passou e a atual. Há dois formatos:
//!
//! - Jira Server/Data Center (API v2, conferido no 8.5.2): cada item é o
//!   `toString()` do objeto do GreenHopper,
//!   `com.atlassian.greenhopper.service.sprint.Sprint@151c6c2d[id=2254,
//!   rapidViewId=402,state=CLOSED,name=Sprint 47,startDate=...,
//!   completeDate=<null>,sequence=2254,goal=]`, às vezes com o `<null>`
//!   escapado em HTML (`&lt;null&gt;`);
//! - Jira Cloud e versões novas: objetos `{id, name, state, boardId,
//!   startDate, endDate, completeDate}`.
//!
//! O nome e a meta são texto livre (podem ter vírgula e `=`): o texto do
//! GreenHopper é cortado pelas chaves conhecidas (`,id=`, `,name=`, ...),
//! não por vírgula.

use ct_core::{Sprint, SprintState};
use serde_json::Value;

/// Prefixo do texto do GreenHopper.
const GREENHOPPER: &str = "com.atlassian.greenhopper.service.sprint.Sprint@";

/// Chaves do texto do GreenHopper, âncoras do corte.
const KEYS: &[&str] = &[
    "id",
    "rapidViewId",
    "state",
    "name",
    "startDate",
    "endDate",
    "completeDate",
    "activatedDate",
    "sequence",
    "goal",
    "autoStartStop",
    "synced",
    "incompleteIssuesDestinationId",
];

/// Sprints do valor do campo, da mais antiga para a mais recente (pela
/// `sequence`, ou pelo id). Item ilegível fica de fora; `null`, lista vazia
/// e valor de outro formato dão lista vazia.
pub fn parse_sprints(v: &Value) -> Vec<Sprint> {
    let Some(items) = v.as_array() else {
        return Vec::new();
    };
    let mut out: Vec<(i64, Sprint)> = items
        .iter()
        .filter_map(|i| match i {
            Value::String(s) => from_greenhopper(s),
            Value::Object(_) => from_object(i),
            _ => None,
        })
        .collect();
    out.sort_by_key(|(seq, s)| (*seq, s.id));
    let mut seen = std::collections::HashSet::new();
    out.into_iter()
        .map(|(_, s)| s)
        .filter(|s| seen.insert(s.id))
        .collect()
}

/// O valor tem o formato do campo de sprint (para a autodetecção): lista
/// de textos do GreenHopper ou de objetos com `id`, `name`, `state` e o
/// quadro (`boardId`/`rapidViewId`/`originBoardId`).
pub fn looks_like_sprints(v: &Value) -> bool {
    let Some(items) = v.as_array().filter(|a| !a.is_empty()) else {
        return false;
    };
    items.iter().all(|i| match i {
        Value::String(s) => s.starts_with(GREENHOPPER),
        Value::Object(o) => {
            ["id", "name", "state"].iter().all(|k| o.contains_key(*k))
                && ["boardId", "rapidViewId", "originBoardId"]
                    .iter()
                    .any(|k| o.contains_key(*k))
        }
        _ => false,
    })
}

/// O primeiro `customfield_*` (em ordem de nome) com valor de sprint.
pub fn detect_sprint_field(fields: &Value) -> Option<String> {
    let mut keys: Vec<&String> = fields
        .as_object()?
        .iter()
        .filter(|(k, v)| k.starts_with("customfield_") && looks_like_sprints(v))
        .map(|(k, _)| k)
        .collect();
    keys.sort();
    keys.first().map(|k| k.to_string())
}

/// Sprints da issue (`fields` do `issue view --raw`): o campo do cadastro,
/// ou o detectado quando ele está vazio.
pub fn issue_sprints(fields: &Value, sprint_field: &str) -> Vec<Sprint> {
    let field = match sprint_field.trim() {
        "" => detect_sprint_field(fields),
        f => Some(f.to_string()),
    };
    field
        .and_then(|f| fields.get(&f))
        .map(parse_sprints)
        .unwrap_or_default()
}

fn state(s: &str) -> SprintState {
    match s.trim().to_ascii_lowercase().as_str() {
        "active" => SprintState::Active,
        "closed" => SprintState::Closed,
        // `FUTURE` e qualquer outro: ainda não começou.
        _ => SprintState::Future,
    }
}

/// `&lt;` e afins (o jira-cli devolve o texto escapado em HTML).
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Valor ausente: vazio ou `<null>`.
fn present(s: &str) -> Option<&str> {
    let s = s.trim();
    (!s.is_empty() && s != "<null>").then_some(s)
}

fn date(s: Option<&str>) -> Option<i64> {
    s.and_then(ct_core::parse_iso8601)
}

/// `(sequence, sprint)` do texto do GreenHopper.
fn from_greenhopper(raw: &str) -> Option<(i64, Sprint)> {
    let raw = unescape(raw);
    let body = raw.strip_prefix(GREENHOPPER)?;
    let body = &body[body.find('[')? + 1..];
    let body = body.strip_suffix(']').unwrap_or(body);
    // Cada chave conhecida, na primeira vez que aparece no começo do texto
    // ou depois de uma vírgula; o valor vai até a chave seguinte.
    let mut anchors: Vec<(usize, &str)> = Vec::new();
    for key in KEYS {
        let pat = format!("{key}=");
        let at = if body.starts_with(&pat) {
            Some(0)
        } else {
            body.find(&format!(",{pat}")).map(|i| i + 1)
        };
        if let Some(at) = at {
            anchors.push((at, key));
        }
    }
    anchors.sort();
    let value = |key: &str| -> Option<&str> {
        let i = anchors.iter().position(|(_, k)| *k == key)?;
        let start = anchors[i].0 + key.len() + 1;
        let end = anchors.get(i + 1).map_or(body.len(), |(at, _)| at - 1);
        body.get(start..end.max(start))
    };
    let num = |key: &str| value(key).and_then(present).and_then(|v| v.parse().ok());
    let id: i64 = num("id")?;
    let sprint = Sprint {
        id,
        name: value("name").unwrap_or("").to_string(),
        state: state(value("state").unwrap_or("")),
        board_id: num("rapidViewId"),
        start_date: date(value("startDate").and_then(present)),
        end_date: date(value("endDate").and_then(present)),
        complete_date: date(value("completeDate").and_then(present)),
    };
    Some((num("sequence").unwrap_or(id), sprint))
}

/// `(sequence, sprint)` do objeto (Jira Cloud e versões novas).
fn from_object(v: &Value) -> Option<(i64, Sprint)> {
    let int = |k: &str| {
        v.get(k).and_then(|x| {
            x.as_i64()
                .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
        })
    };
    let text = |k: &str| v.get(k).and_then(Value::as_str).and_then(present);
    let id = int("id")?;
    let sprint = Sprint {
        id,
        name: text("name").map(unescape).unwrap_or_default(),
        state: state(text("state").unwrap_or("")),
        board_id: int("boardId")
            .or_else(|| int("originBoardId"))
            .or_else(|| int("rapidViewId")),
        start_date: date(text("startDate")),
        end_date: date(text("endDate")),
        complete_date: date(text("completeDate")),
    };
    Some((int("sequence").unwrap_or(id), sprint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// O valor real do `customfield_10600` (Jira 8.5.2, PROJ-5392).
    fn real() -> Value {
        json!([
            "com.atlassian.greenhopper.service.sprint.Sprint@151c6c2d[id=2254,rapidViewId=402,state=CLOSED,name=Sprint 47,startDate=2026-10-13T13:51:37.829-03:00,endDate=2026-10-24T00:51:00.000-02:00,completeDate=2026-10-28T09:30:58.135-02:00,sequence=2254,goal=]",
            "com.atlassian.greenhopper.service.sprint.Sprint@1befa4d5[id=2273,rapidViewId=402,state=ACTIVE,name=Sprint 48,startDate=2026-10-26T11:37:15.379-02:00,endDate=2026-11-06T21:37:00.000-02:00,completeDate=&lt;null&gt;,sequence=2273,goal=]"
        ])
    }

    #[test]
    fn formato_do_greenhopper_com_null_escapado() {
        let s = parse_sprints(&real());
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].id, s[0].name.as_str()), (2254, "Sprint 47"));
        assert_eq!(s[0].state, SprintState::Closed);
        assert_eq!(s[0].board_id, Some(402));
        assert_eq!(
            s[0].start_date,
            ct_core::parse_iso8601("2026-10-13T13:51:37.829-03:00")
        );
        assert!(s[0].complete_date.is_some());
        assert_eq!((s[1].id, s[1].state), (2273, SprintState::Active));
        assert_eq!(s[1].complete_date, None, "&lt;null&gt; = ausente");
        assert_eq!(
            s[1].end_date,
            ct_core::parse_iso8601("2026-11-06T21:37:00.000-02:00")
        );
    }

    #[test]
    fn nome_com_virgula_e_igual_e_ordem_pela_sequence() {
        let v = json!([
            "com.atlassian.greenhopper.service.sprint.Sprint@1[id=9,rapidViewId=1,state=FUTURE,name=Fase 2, parte=1 (a=b),startDate=<null>,endDate=<null>,completeDate=<null>,sequence=20,goal=entregar, com vírgula]",
            "com.atlassian.greenhopper.service.sprint.Sprint@2[id=10,rapidViewId=1,state=CLOSED,name=Fase &amp; 1,startDate=<null>,endDate=<null>,completeDate=<null>,sequence=5,goal=]",
        ]);
        let s = parse_sprints(&v);
        assert_eq!(s.iter().map(|x| x.id).collect::<Vec<_>>(), [10, 9]);
        assert_eq!(s[0].name, "Fase & 1");
        assert_eq!(s[1].name, "Fase 2, parte=1 (a=b)");
        assert_eq!(s[1].state, SprintState::Future);
        assert_eq!((s[1].start_date, s[1].end_date), (None, None));
    }

    #[test]
    fn formato_objeto() {
        let v = json!([
            { "id": 31, "name": "S2", "state": "active", "boardId": 7,
              "startDate": "2026-10-26T14:37:15.379Z", "endDate": "2026-11-06T23:37:00.000Z" },
            { "id": 30, "name": "S1", "state": "closed", "boardId": 7,
              "completeDate": "2026-10-25T10:00:00.000Z" },
        ]);
        let s = parse_sprints(&v);
        assert_eq!(s.iter().map(|x| x.id).collect::<Vec<_>>(), [30, 31]);
        assert_eq!(s[1].state, SprintState::Active);
        assert_eq!(s[1].board_id, Some(7));
        assert!(s[1].start_date.is_some() && s[1].complete_date.is_none());
        assert!(looks_like_sprints(&v));
    }

    #[test]
    fn vazio_null_e_ilegivel() {
        assert!(parse_sprints(&Value::Null).is_empty());
        assert!(parse_sprints(&json!([])).is_empty());
        assert!(parse_sprints(&json!("Sprint 1")).is_empty());
        assert!(parse_sprints(&json!([
            "com.atlassian.greenhopper.service.sprint.Sprint@1[name=sem id]",
            42
        ]))
        .is_empty());
        assert!(!looks_like_sprints(&Value::Null));
        assert!(!looks_like_sprints(&json!([])));
        assert!(!looks_like_sprints(&json!(["texto"])));
        assert!(!looks_like_sprints(
            &json!([{ "id": 1, "name": "x", "state": "y" }])
        ));
    }

    #[test]
    fn autodeteccao_do_campo() {
        let fields = json!({
            "summary": "x",
            "customfield_10000": ["texto qualquer"],
            "customfield_10601": "PROJ-1",
            "customfield_10600": real(),
            "customfield_10700": null,
        });
        assert_eq!(
            detect_sprint_field(&fields).as_deref(),
            Some("customfield_10600")
        );
        assert_eq!(issue_sprints(&fields, "").len(), 2);
        // O campo do cadastro vale sem detectar (aqui, um sem sprint).
        assert!(issue_sprints(&fields, "customfield_10700").is_empty());
        assert_eq!(issue_sprints(&fields, " customfield_10600 ").len(), 2);
        // Sem sprint em nenhum campo.
        let sem = json!({ "customfield_10600": null, "customfield_1": [] });
        assert_eq!(detect_sprint_field(&sem), None);
        assert!(issue_sprints(&sem, "").is_empty());
    }
}

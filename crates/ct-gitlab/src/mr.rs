//! MR do GitLab → linha do espelho (`pr_cache`), como o PR do GitHub.

use ct_core::PrState;
use ct_ledger::PrRow;
use serde_json::Value;

/// `opened` → aberto, `merged` → mesclado, `closed` e `locked` → fechado.
/// `None` para um estado desconhecido.
pub fn mr_state(state: &str) -> Option<PrState> {
    match state {
        "opened" => Some(PrState::Open),
        "merged" => Some(PrState::Merged),
        "closed" | "locked" => Some(PrState::Closed),
        _ => None,
    }
}

/// MR da API (o `mr list -F json` do glab devolve os objetos da API v4) →
/// linha do cache, ligada ao work item da branch: `number` = `iid`, `head`
/// = `source_branch`, `base` = `target_branch`, `url` = `web_url` e
/// `head_sha` = `sha`. `None` para objeto incompleto.
pub fn mr_row(v: &Value, work_item_id: i64) -> Option<PrRow> {
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    Some(PrRow {
        number: v.get("iid")?.as_i64()?,
        work_item_id,
        head: s("source_branch")?.to_string(),
        base: s("target_branch")?.to_string(),
        state: mr_state(s("state")?)?,
        url: s("web_url").unwrap_or_default().to_string(),
        updated_at: s("updated_at")
            .and_then(ct_core::parse_iso8601)
            .unwrap_or(0),
        head_sha: s("sha").filter(|h| !h.is_empty()).map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mr_vira_linha_do_espelho() {
        let v = json!({
            "id": 99001, "iid": 12, "state": "merged",
            "source_branch": "PROJ-5450", "target_branch": "develop",
            "web_url": "https://gitlab.example.com/grupo/projeto/app/-/merge_requests/12",
            "updated_at": "2026-10-28T13:15:30.123Z",
            "sha": "abc123",
        });
        let r = mr_row(&v, 7).unwrap();
        assert_eq!(r.number, 12, "o iid, não o id global");
        assert_eq!((r.head.as_str(), r.base.as_str()), ("PROJ-5450", "develop"));
        assert_eq!(r.state, PrState::Merged);
        assert_eq!(r.work_item_id, 7);
        assert_eq!(r.head_sha.as_deref(), Some("abc123"));
        assert_eq!(
            r.updated_at,
            ct_core::parse_iso8601("2026-10-28T13:15:30.123Z").unwrap()
        );
        assert!(r.url.ends_with("/-/merge_requests/12"));

        for (s, e) in [
            ("opened", Some(PrState::Open)),
            ("closed", Some(PrState::Closed)),
            ("locked", Some(PrState::Closed)),
            ("draft", None),
        ] {
            assert_eq!(mr_state(s), e, "{s}");
        }
        assert!(mr_row(&json!({ "iid": 1 }), 7).is_none());
    }
}

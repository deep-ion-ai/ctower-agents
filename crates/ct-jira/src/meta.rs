//! Metadados do Jira para a tela da issue: valor do campo → texto.

use ct_core::JiraExtraField;
use ct_ledger::IssueMeta;
use serde_json::Value;

/// Campos padrão do Jira (ID fixo em qualquer instância), na ordem da tela.
pub const STANDARD_FIELDS: &[(&str, &str)] = &[
    ("status", "Situação"),
    ("issuetype", "Tipo"),
    ("priority", "Prioridade"),
    ("assignee", "Responsável"),
    ("reporter", "Criador"),
];

/// Valor de um campo do Jira → texto para exibir. `None` = nulo ou vazio
/// (o campo é omitido).
///
/// - usuário: `displayName`, com `name` como alternativa;
/// - option ou objeto com nome: `value` ou `name`;
/// - lista: os itens unidos por ", ";
/// - string, número e booleano: o próprio valor.
pub fn field_text(v: &Value) -> Option<String> {
    let text = match v {
        Value::Null => return None,
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(a) => a
            .iter()
            .filter_map(field_text)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(o) => ["displayName", "value", "name"]
            .iter()
            .find_map(|k| o.get(*k).and_then(field_text))?,
    };
    (!text.is_empty()).then_some(text)
}

/// `fields` da issue → metadados: os campos padrão, na ordem, e depois os
/// campos próprios do cadastro (`extra`), na ordem do cadastro, com o
/// rótulo cadastrado. Os vazios ficam de fora.
pub fn issue_meta(fields: &Value, extra: &[JiraExtraField]) -> Vec<IssueMeta> {
    let standard = STANDARD_FIELDS.iter().map(|(id, label)| (*id, *label));
    let extra = extra
        .iter()
        .map(|e| (e.id.trim(), e.label.trim()))
        .filter(|(id, _)| !id.is_empty());
    standard
        .chain(extra)
        .filter_map(|(id, label)| {
            Some(IssueMeta {
                field: id.to_string(),
                label: label.to_string(),
                value: field_text(fields.get(id)?)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usuario_usa_display_name_e_depois_name() {
        let u = json!({ "name": "fulano", "displayName": "Fulano de Tal", "active": true });
        assert_eq!(field_text(&u).as_deref(), Some("Fulano de Tal"));
        let u = json!({ "name": "fulano", "displayName": "" });
        assert_eq!(field_text(&u).as_deref(), Some("fulano"));
    }

    #[test]
    fn option_e_objeto_com_nome() {
        let o = json!({ "self": "x", "value": "Alta", "id": "10001" });
        assert_eq!(field_text(&o).as_deref(), Some("Alta"));
        let p = json!({ "self": "x", "name": "Crítica", "id": "1" });
        assert_eq!(field_text(&p).as_deref(), Some("Crítica"));
    }

    #[test]
    fn lista_une_os_itens() {
        let l = json!([{ "value": "A" }, null, "B", { "name": "C" }, ""]);
        assert_eq!(field_text(&l).as_deref(), Some("A, B, C"));
        assert_eq!(field_text(&json!([])), None);
        assert_eq!(field_text(&json!([null, ""])), None);
    }

    #[test]
    fn escalares() {
        assert_eq!(field_text(&json!(8)).as_deref(), Some("8"));
        assert_eq!(field_text(&json!(2.5)).as_deref(), Some("2.5"));
        assert_eq!(field_text(&json!(true)).as_deref(), Some("true"));
        assert_eq!(field_text(&json!(" texto ")).as_deref(), Some("texto"));
    }

    #[test]
    fn nulo_e_vazio_sao_omitidos() {
        assert_eq!(field_text(&Value::Null), None);
        assert_eq!(field_text(&json!("  ")), None);
        assert_eq!(field_text(&json!({ "value": null })), None);
    }

    fn extra(id: &str, label: &str) -> JiraExtraField {
        JiraExtraField {
            id: id.into(),
            label: label.into(),
        }
    }

    /// Padrão primeiro, extras depois na ordem do cadastro, sem os vazios.
    #[test]
    fn extras_depois_dos_padrao() {
        let fields = json!({
            "status": { "name": "Aberto" },
            "customfield_2": { "value": "Alta" },
            "customfield_1": { "name": "fulano", "displayName": "Fulano" },
            "customfield_3": null,
        });
        let extras = [
            extra("customfield_1", "Implementação"),
            extra("customfield_3", "Vazio"),
            extra("customfield_2", "Criticidade"),
            extra("customfield_9", "Ausente"),
        ];
        let got: Vec<_> = issue_meta(&fields, &extras)
            .into_iter()
            .map(|m| (m.field, m.label, m.value))
            .collect();
        let m = |f: &str, l: &str, v: &str| (f.to_string(), l.to_string(), v.to_string());
        assert_eq!(
            got,
            vec![
                m("status", "Situação", "Aberto"),
                m("customfield_1", "Implementação", "Fulano"),
                m("customfield_2", "Criticidade", "Alta"),
            ]
        );
    }

    #[test]
    fn objeto_desconhecido_e_omitido() {
        assert_eq!(field_text(&json!({ "id": "1", "self": "x" })), None);
        assert_eq!(field_text(&json!({})), None);
    }
}

//! Conexão com o hub (`cthubd`, spec do hub como plano de controle §8),
//! separada das conexões com os ambientes: URL e token de UI do hub, em
//! `hub.json` no diretório de dados do app. O token nunca vai para o
//! JavaScript.

use crate::bridge::{Sink, PUSH_EVENT, STATUS_EVENT};
use crate::envs::{EnvEntry, EnvKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Id da conexão do hub na ponte dele (não colide com os ambientes: a
/// ponte é outra).
pub const HUB_ID: &str = "hub";
pub const HUB_STATUS_EVENT: &str = "hub://status";
pub const HUB_PUSH_EVENT: &str = "hub://push";

/// Como fica em disco.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct HubEntry {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl std::fmt::Debug for HubEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubEntry")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// Como o JavaScript vê: sem token.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HubView {
    pub url: String,
    pub has_token: bool,
}

impl From<&HubEntry> for HubView {
    fn from(h: &HubEntry) -> Self {
        HubView {
            url: h.url.clone(),
            has_token: h.token.as_deref().is_some_and(|t| !t.is_empty()),
        }
    }
}

/// Edição vinda da UI. Token vazio ou ausente mantém o atual.
#[derive(Debug, Clone, Deserialize)]
pub struct HubInput {
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
}

/// `http://host:7879` (como o `hub_url` do ambiente) vira
/// `ws://host:7879/ws`; sem caminho, ganha `/ws`.
pub fn normalize_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    let (scheme, rest) = if let Some(r) = url.strip_prefix("http://") {
        ("ws://", r)
    } else if let Some(r) = url.strip_prefix("https://") {
        ("wss://", r)
    } else if let Some(r) = url.strip_prefix("ws://") {
        ("ws://", r)
    } else if let Some(r) = url.strip_prefix("wss://") {
        ("wss://", r)
    } else {
        return Err(format!(
            "URL do hub deve começar com ws://, wss://, http:// ou https://: {url}"
        ));
    };
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() {
        return Err(format!("URL do hub sem endereço: {url}"));
    }
    Ok(if rest.contains('/') {
        format!("{scheme}{rest}")
    } else {
        format!("{scheme}{rest}/ws")
    })
}

/// Aplica a edição: `None` desliga a conexão com o hub.
pub fn merge(
    current: Option<&HubEntry>,
    input: Option<HubInput>,
) -> Result<Option<HubEntry>, String> {
    let Some(i) = input else { return Ok(None) };
    let url = normalize_url(&i.url)?;
    let token = match i.token.filter(|t| !t.trim().is_empty()) {
        Some(t) => Some(t.trim().to_string()),
        // O token gravado só vale para o mesmo esquema, host e porta: não
        // pode seguir para outro destino (ex.: o `/ws` de um ambiente).
        None => current
            .filter(|c| crate::envs::same_origin(&c.url, &url))
            .and_then(|c| c.token.clone()),
    };
    if token.is_none() {
        return Err(match current {
            Some(_) => "a URL do hub mudou de host ou porta: informe o token de UI do hub de novo \
                        (~/.config/cthub/ui.token na máquina do hub)"
                .into(),
            None => {
                "informe o token de UI do hub (~/.config/cthub/ui.token na máquina do hub)".into()
            }
        });
    }
    Ok(Some(HubEntry { url, token }))
}

/// A entrada da ponte do hub (o resolvedor lê o token dela).
pub fn bridge_entry(h: &HubEntry) -> EnvEntry {
    EnvEntry {
        id: HUB_ID.into(),
        name: "hub".into(),
        url: h.url.clone(),
        kind: EnvKind::Remote,
        distro: None,
        token: h.token.clone(),
    }
}

/// Repassa os eventos da ponte do hub com nomes próprios (`hub://status`,
/// `hub://push`), para a UI não os confundir com os de um ambiente.
pub struct HubSink(pub Arc<dyn Sink>);

impl Sink for HubSink {
    fn emit(&self, event: &str, payload: Value) {
        let event = match event {
            STATUS_EVENT => HUB_STATUS_EVENT,
            PUSH_EVENT => HUB_PUSH_EVENT,
            other => other,
        };
        self.0.emit(event, payload);
    }

    /// A queda do hub aparece como faixa na UI; sem notificação do sistema.
    fn notify(&self, _title: &str, _body: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::tests::Collect;
    use serde_json::json;

    #[test]
    fn url_do_hub() {
        assert_eq!(
            normalize_url("http://mac-mini:7879").unwrap(),
            "ws://mac-mini:7879/ws"
        );
        assert_eq!(
            normalize_url(" https://h:7879/ ").unwrap(),
            "wss://h:7879/ws"
        );
        assert_eq!(
            normalize_url("ws://127.0.0.1:7879/ws").unwrap(),
            "ws://127.0.0.1:7879/ws"
        );
        assert_eq!(normalize_url("wss://h/x/ws").unwrap(), "wss://h/x/ws");
        assert!(normalize_url("mac-mini:7879").is_err());
        assert!(normalize_url("ws://").is_err());
    }

    #[test]
    fn token_vazio_mantem_o_atual_e_none_desliga() {
        let input = |token: Option<&str>| HubInput {
            url: "http://h:7879".into(),
            token: token.map(String::from),
        };
        assert!(merge(None, Some(input(None))).is_err(), "sem token");
        let a = merge(None, Some(input(Some("t1")))).unwrap().unwrap();
        assert_eq!(a.token.as_deref(), Some("t1"));
        let b = merge(Some(&a), Some(input(Some("")))).unwrap().unwrap();
        assert_eq!(b.token.as_deref(), Some("t1"));
        let c = merge(Some(&a), Some(input(Some("t2")))).unwrap().unwrap();
        assert_eq!(c.token.as_deref(), Some("t2"));
        assert_eq!(merge(Some(&a), None).unwrap(), None);
        // Mesmo destino com outro caminho ou na forma http: mantém.
        let same = HubInput {
            url: "ws://h:7879/ws".into(),
            token: None,
        };
        assert_eq!(
            merge(Some(&a), Some(same))
                .unwrap()
                .unwrap()
                .token
                .as_deref(),
            Some("t1")
        );
        // Outro host ou porta (ex.: o /ws de um ambiente): exige o token de novo.
        for url in ["http://h:7878", "http://outro:7879", "https://h:7879"] {
            let other = HubInput {
                url: url.into(),
                token: None,
            };
            let err = merge(Some(&a), Some(other)).unwrap_err();
            assert!(err.contains("informe o token"), "{url}: {err}");
        }
    }

    #[test]
    fn visao_e_debug_sem_token() {
        let h = HubEntry {
            url: "ws://h/ws".into(),
            token: Some("segredo".into()),
        };
        let v = HubView::from(&h);
        assert!(v.has_token);
        assert!(!serde_json::to_string(&v).unwrap().contains("segredo"));
        assert!(!format!("{h:?}").contains("segredo"));
    }

    #[test]
    fn eventos_renomeados_e_sem_notificacao() {
        let c = Arc::new(Collect::default());
        let s = HubSink(c.clone());
        s.emit(STATUS_EVENT, json!({ "env": HUB_ID }));
        s.emit(PUSH_EVENT, json!({ "event": "hub_sync_status" }));
        s.notify("Ambiente desconectado", "hub");
        let got: Vec<String> = c.0.lock().unwrap().iter().map(|(e, _)| e.clone()).collect();
        assert_eq!(got, [HUB_STATUS_EVENT, HUB_PUSH_EVENT]);
        assert!(c.1.lock().unwrap().is_empty());
    }
}

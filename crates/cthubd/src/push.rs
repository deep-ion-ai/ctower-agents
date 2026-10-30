//! Web Push da PWA (issue #203, spec da PWA §7): chave VAPID, cifragem do
//! corpo (RFC 8291, `aes128gcm`), assinatura VAPID (RFC 8292, ES256) e o
//! motor que observa o `hub.db` e envia.
//!
//! Tudo com `ring`, que o hub já usa (rustls do `ureq` e `jsonwebtoken`):
//! ECDH P-256 com chave efêmera por mensagem, HKDF-SHA256, AES-128-GCM e
//! ECDSA P-256. O envio é um `POST` pelo `ureq`, o mesmo cliente HTTP do
//! resto do hub, sem outro stack TLS.
//!
//! O `env_stale` de ambiente com canal reverso usa o estado do canal, em
//! tempo real ([`WebPush::with_channels`]).
//!
//! O motor varre a cada [`SCAN_EVERY`] e logo depois de um `/sync/push` ou
//! de uma proposta nova ([`WebPush::kick`]). O que notificar e a
//! deduplicação ficam no `ct-ledger` (`Hub::push_collect`).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ct_ledger::{PushEvent, PushNotice, PushTarget, SharedHub};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use ring::{aead, agreement, hkdf};
use serde::Serialize;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Intervalo da varredura periódica (o `env_stale` só aparece nela).
pub const SCAN_EVERY: Duration = Duration::from_secs(30);
/// Até quanto tempo atrás uma sessão, evento ou proposta ainda vira push.
pub const WINDOW_MS: i64 = 3600 * 1000;
/// `TTL` da mensagem no serviço de push (spec §7.2).
pub const TTL_SECS: u64 = 3600;
/// Validade do JWT VAPID (o máximo do RFC 8292 é 24 h).
const JWT_SECS: i64 = 12 * 3600;
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Tamanho do registro do `aes128gcm` (uma mensagem cabe num só).
const RECORD_SIZE: u32 = 4096;

/// Erro de `hub.push.*`: Web Push desligada (web ou `[web.push]` desligada).
pub const PUSH_UNAVAILABLE: i64 = -32030;

fn b64(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

/// base64url com ou sem `=`; aceita também o alfabeto padrão.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s: String = s
        .trim()
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    URL_SAFE_NO_PAD.decode(s).ok()
}

/// Par de chaves VAPID do hub (P-256), em `<config>/vapid.key` (0600, fora
/// do `hub.db` e dos snapshots).
pub struct Vapid {
    key: EcdsaKeyPair,
    subject: String,
    rng: SystemRandom,
}

impl Vapid {
    /// Lê a chave de `path` ou, se não existe, gera e grava (0600).
    pub fn load_or_create(path: &Path, subject: &str) -> io::Result<Self> {
        let rng = SystemRandom::new();
        let bad = |m: &str| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {m}", path.display()),
            )
        };
        let pkcs8 = match std::fs::read_to_string(path) {
            Ok(s) => b64_decode(&s).ok_or_else(|| bad("chave VAPID ilegível"))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                    .map_err(|_| io::Error::other("falha ao gerar a chave VAPID"))?;
                let bytes = doc.as_ref().to_vec();
                ct_core::files::write_private_atomic(
                    path,
                    format!("{}\n", b64(&bytes)).as_bytes(),
                )?;
                tracing::info!("chave VAPID gerada em {}", path.display());
                bytes
            }
            Err(e) => return Err(e),
        };
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8, &rng)
            .map_err(|_| bad("chave VAPID inválida"))?;
        Ok(Vapid {
            key,
            subject: subject.to_string(),
            rng,
        })
    }

    /// Chave pública (ponto não comprimido, 65 bytes) em base64url: o
    /// `applicationServerKey` da inscrição.
    pub fn public_key(&self) -> String {
        b64(self.key.public_key().as_ref())
    }

    /// `Authorization: vapid t=<jwt>, k=<chave>` para o `endpoint`.
    pub fn authorization(&self, endpoint: &str, now_secs: i64) -> Option<String> {
        let aud = origin_of(endpoint)?;
        let header = b64(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = b64(
            serde_json::json!({ "aud": aud, "exp": now_secs + JWT_SECS, "sub": self.subject })
                .to_string()
                .as_bytes(),
        );
        let msg = format!("{header}.{claims}");
        let sig = self.key.sign(&self.rng, msg.as_bytes()).ok()?;
        Some(format!(
            "vapid t={msg}.{}, k={}",
            b64(sig.as_ref()),
            self.public_key()
        ))
    }
}

/// `scheme://host[:porta]` do endpoint (o `aud` do JWT).
pub fn origin_of(endpoint: &str) -> Option<String> {
    let (scheme, rest) = endpoint.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty() && matches!(scheme, "https" | "http")).then(|| format!("{scheme}://{host}"))
}

/// `sub` do JWT (spec §7.2): `[web.push] subject`, senão `mailto:` do
/// primeiro e-mail do Access, senão o primeiro origin `https://`.
pub fn subject(cfg: &crate::config::WebConfig) -> String {
    if !cfg.push.subject.trim().is_empty() {
        return cfg.push.subject.trim().to_string();
    }
    if let Some(e) = cfg
        .cloudflare
        .as_ref()
        .and_then(|c| c.emails.iter().map(|e| e.trim()).find(|e| !e.is_empty()))
    {
        return format!("mailto:{e}");
    }
    cfg.origins
        .iter()
        .map(|o| o.trim().trim_end_matches('/'))
        .find(|o| o.starts_with("https://"))
        .map(str::to_string)
        // Sem nenhum dos dois: um mailto válido, que os serviços aceitam.
        .unwrap_or_else(|| "mailto:cthubd@localhost".into())
}

struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
    let mut out = vec![0u8; len];
    prk.expand(&[info], Len(len))
        .and_then(|okm| okm.fill(&mut out))
        .expect("tamanho do HKDF dentro do limite");
    out
}

/// Chave e nonce do conteúdo (RFC 8291 §3.3 e §3.4, RFC 8188 §2.2).
pub fn derive_cek(
    ecdh_secret: &[u8],
    auth: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8],
) -> (Vec<u8>, Vec<u8>) {
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let ikm = hkdf_sha256(auth, ecdh_secret, &key_info, 32);
    let cek = hkdf_sha256(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
    let nonce = hkdf_sha256(salt, &ikm, b"Content-Encoding: nonce\0", 12);
    (cek, nonce)
}

/// Cifra `plain` para a inscrição (`p256dh`, `auth` já decodificados):
/// cabeçalho `salt || rs || idlen || keyid` e um registro só.
pub fn encrypt(plain: &[u8], ua_public: &[u8], auth: &[u8]) -> Result<Vec<u8>, String> {
    let rng = SystemRandom::new();
    if ua_public.len() != 65 || auth.len() != 16 {
        return Err("chaves da inscrição com tamanho inválido".into());
    }
    if plain.len() + 17 + 86 > RECORD_SIZE as usize {
        return Err("notificação grande demais".into());
    }
    let eph = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|_| "falha ao gerar a chave efêmera")?;
    let as_public = eph
        .compute_public_key()
        .map_err(|_| "falha na chave efêmera")?;
    let secret = agreement::agree_ephemeral(
        eph,
        &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, ua_public),
        |s| s.to_vec(),
    )
    .map_err(|_| "chave p256dh inválida")?;
    let mut salt = [0u8; 16];
    rng.fill(&mut salt).map_err(|_| "sem aleatoriedade")?;
    let (cek, nonce) = derive_cek(&secret, auth, ua_public, as_public.as_ref(), &salt);
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_128_GCM, &cek).expect("chave de 16 bytes"),
    );
    // Delimitador do último registro, sem preenchimento.
    let mut buf = plain.to_vec();
    buf.push(2);
    key.seal_in_place_append_tag(
        aead::Nonce::try_assume_unique_for_key(&nonce).expect("nonce de 12 bytes"),
        aead::Aad::empty(),
        &mut buf,
    )
    .map_err(|_| "falha ao cifrar")?;
    let mut out = Vec::with_capacity(16 + 4 + 1 + 65 + buf.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(as_public.as_ref().len() as u8);
    out.extend_from_slice(as_public.as_ref());
    out.extend_from_slice(&buf);
    Ok(out)
}

/// Resultado de uma entrega.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// 2xx do serviço de push.
    Ok(u16),
    /// 404/410: a inscrição não existe mais (apagada).
    Gone(u16),
    /// Outra resposta ou falha de rede (conta em `failures`).
    Failed(String),
}

/// Resposta de `hub.push.test`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PushTestResult {
    /// O serviço de push aceitou a mensagem (não garante que o aparelho mostrou).
    pub delivered: bool,
    /// Status HTTP do serviço de push, se houve resposta.
    pub status: Option<u16>,
    /// A inscrição foi apagada (404/410: expirou no navegador).
    pub removed: bool,
    pub error: Option<String>,
}

/// Resposta de `hub.push.vapid_key`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PushVapidKey {
    /// Chave pública VAPID em base64url (`applicationServerKey`).
    pub key: String,
}

/// Resposta de `hub.push.remove`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PushRemoved {
    pub removed: bool,
}

/// Opções do motor.
#[derive(Debug, Clone)]
pub struct PushOptions {
    /// `env_stale_minutes` em ms; `0` = sem `env_stale`.
    pub stale_ms: i64,
    /// Aceita endpoint `http://` (só nos testes, com o serviço local).
    pub allow_http: bool,
}

/// Web Push do hub: chave, opções e o gatilho da varredura.
pub struct WebPush {
    pub vapid: Vapid,
    hub: SharedHub,
    opts: PushOptions,
    kick: tokio::sync::Notify,
    /// Uma varredura por vez (a periódica e a do gatilho).
    scanning: Mutex<()>,
    /// Canais reversos (#198): o `env_stale` em tempo real (spec §7.1).
    channels: Option<Arc<crate::channel::EnvChannels>>,
}

impl WebPush {
    pub fn new(vapid: Vapid, hub: SharedHub, opts: PushOptions) -> Self {
        WebPush {
            vapid,
            hub,
            opts,
            kick: tokio::sync::Notify::new(),
            scanning: Mutex::new(()),
            channels: None,
        }
    }

    /// Usa o estado dos canais reversos no `env_stale`: ambiente com canal
    /// fica parado quando o canal fecha há mais de 1 min (spec §7.1, fase 2).
    pub fn with_channels(mut self, channels: Arc<crate::channel::EnvChannels>) -> Self {
        self.channels = Some(channels);
        self
    }

    /// Pede uma varredura já (depois de um sync ou de uma proposta).
    pub fn kick(&self) {
        self.kick.notify_one();
    }

    /// Confere o `PushSubscription` vindo do navegador.
    pub fn validate(&self, endpoint: &str, p256dh: &str, auth: &str) -> Result<(), String> {
        let e = endpoint.trim();
        let ok_scheme =
            e.starts_with("https://") || (self.opts.allow_http && e.starts_with("http://"));
        if !ok_scheme || origin_of(e).is_none() {
            return Err("endpoint de push precisa ser https://".into());
        }
        if b64_decode(p256dh).map(|k| k.len()) != Some(65) {
            return Err("keys.p256dh inválida (P-256 não comprimida, base64url)".into());
        }
        if b64_decode(auth).map(|k| k.len()) != Some(16) {
            return Err("keys.auth inválida (16 bytes, base64url)".into());
        }
        Ok(())
    }

    /// Cifra e envia `notice` para `t` (bloqueante: rede).
    pub fn send(&self, t: &PushTarget, notice: &PushNotice) -> Delivery {
        let plain = serde_json::to_vec(notice).expect("PushNotice serializa");
        let (Some(ua), Some(auth)) = (b64_decode(&t.p256dh), b64_decode(&t.auth)) else {
            return Delivery::Failed("chaves da inscrição ilegíveis".into());
        };
        let body = match encrypt(&plain, &ua, &auth) {
            Ok(b) => b,
            Err(e) => return Delivery::Failed(e),
        };
        let Some(authz) = self
            .vapid
            .authorization(&t.endpoint, ct_core::now_ms() / 1000)
        else {
            return Delivery::Failed("endpoint inválido".into());
        };
        let urgency = match notice.event {
            PushEvent::NeedsHuman | PushEvent::SessionError => "high",
            _ => "normal",
        };
        let agent = ureq::AgentBuilder::new()
            .timeout(SEND_TIMEOUT)
            .redirects(0)
            .build();
        let r = agent
            .post(&t.endpoint)
            .set("Authorization", &authz)
            .set("Content-Encoding", "aes128gcm")
            .set("Content-Type", "application/octet-stream")
            .set("TTL", &TTL_SECS.to_string())
            .set("Urgency", urgency)
            .send_bytes(&body);
        match r {
            Ok(resp) if (200..300).contains(&resp.status()) => Delivery::Ok(resp.status()),
            Ok(resp) => Delivery::Failed(format!("HTTP {}", resp.status())),
            Err(ureq::Error::Status(c @ (404 | 410), _)) => Delivery::Gone(c),
            Err(ureq::Error::Status(c, _)) => Delivery::Failed(format!("HTTP {c}")),
            Err(e) => Delivery::Failed(e.to_string()),
        }
    }

    /// Grava o resultado da entrega no `hub.db`.
    fn record(&self, t: &PushTarget, d: &Delivery) {
        let h = self.hub.lock();
        let now = ct_core::now_ms();
        let r = match d {
            Delivery::Ok(_) => h.push_delivered(t.id, now),
            Delivery::Gone(c) => {
                tracing::info!("inscrição de push {} expirou (HTTP {c}): apagada", t.id);
                h.remove_push_subscription(t.id).map(|_| ())
            }
            Delivery::Failed(e) => {
                tracing::warn!("push para a inscrição {} falhou: {e}", t.id);
                h.push_failed(t.id, now).map(|off| {
                    if off {
                        tracing::warn!(
                            "inscrição de push {} desativada depois de {} falhas seguidas",
                            t.id,
                            ct_ledger::PUSH_FAILURES_MAX
                        );
                    }
                })
            }
        };
        if let Err(e) = r {
            tracing::error!("push: gravar a entrega: {e}");
        }
    }

    /// Envia e grava o resultado.
    pub fn deliver(&self, t: &PushTarget, notice: &PushNotice) -> Delivery {
        let d = self.send(t, notice);
        self.record(t, &d);
        d
    }

    /// Uma varredura (bloqueante): o que há de novo, para quem quer.
    /// Devolve quantas entregas foram tentadas.
    pub fn scan_blocking(&self) -> usize {
        let _one = self.scanning.lock().unwrap_or_else(|p| p.into_inner());
        let channels = self
            .channels
            .as_ref()
            .map(|c| c.states())
            .unwrap_or_default();
        let jobs: Vec<(PushNotice, Vec<PushTarget>)> = {
            let h = self.hub.lock();
            let notices = match h.push_collect(WINDOW_MS, self.opts.stale_ms, &channels) {
                Ok(n) => n,
                Err(e) => {
                    tracing::error!("push: varredura: {e}");
                    return 0;
                }
            };
            notices
                .into_iter()
                .map(|n| {
                    let t = h.push_targets(n.event).unwrap_or_else(|e| {
                        tracing::error!("push: inscrições: {e}");
                        Vec::new()
                    });
                    (n, t)
                })
                .collect()
        };
        let mut sent = 0;
        for (n, targets) in jobs {
            for t in targets {
                self.deliver(&t, &n);
                sent += 1;
            }
        }
        sent
    }

    /// Varredura fora do runtime.
    pub async fn scan(self: &Arc<Self>) -> usize {
        let me = Arc::clone(self);
        tokio::task::spawn_blocking(move || me.scan_blocking())
            .await
            .unwrap_or(0)
    }

    /// Notificação de teste para o dispositivo `id` (`hub.push.test`).
    pub fn test_blocking(&self, id: i64) -> Result<PushTestResult, String> {
        let t = self
            .hub
            .lock()
            .push_target(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("dispositivo {id} não está inscrito"))?;
        let notice = PushNotice {
            key: String::new(),
            event: PushEvent::NeedsHuman,
            title: "Control Tower".into(),
            body: "Notificação de teste: este dispositivo recebe os avisos do hub.".into(),
            url: "/?go=hub:devices".into(),
            tag: "test".into(),
        };
        Ok(match self.deliver(&t, &notice) {
            Delivery::Ok(s) => PushTestResult {
                delivered: true,
                status: Some(s),
                removed: false,
                error: None,
            },
            Delivery::Gone(s) => PushTestResult {
                delivered: false,
                status: Some(s),
                removed: true,
                error: Some("a inscrição expirou no navegador e foi apagada".into()),
            },
            Delivery::Failed(e) => PushTestResult {
                delivered: false,
                status: None,
                removed: false,
                error: Some(e),
            },
        })
    }

    /// Varredura periódica e a cada [`WebPush::kick`].
    pub fn spawn(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                self.scan().await;
                tokio::select! {
                    _ = tokio::time::sleep(SCAN_EVERY) => {}
                    _ = self.kick.notified() => {}
                }
            }
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};

    /// Navegador falso: a chave efêmera do lado do navegador vale para uma
    /// mensagem só (o `ring` não guarda chave ECDH estática), o bastante
    /// para conferir uma entrega.
    pub(crate) struct FakeBrowser {
        pub ua_priv: agreement::EphemeralPrivateKey,
        pub ua_pub: Vec<u8>,
        pub auth: [u8; 16],
    }

    impl FakeBrowser {
        pub(crate) fn new() -> Self {
            let rng = SystemRandom::new();
            let ua_priv =
                agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
            let ua_pub = ua_priv.compute_public_key().unwrap().as_ref().to_vec();
            let mut auth = [0u8; 16];
            rng.fill(&mut auth).unwrap();
            FakeBrowser {
                ua_priv,
                ua_pub,
                auth,
            }
        }

        pub(crate) fn keys(&self) -> (String, String) {
            (b64(&self.ua_pub), b64(&self.auth))
        }

        /// Decifra o corpo `aes128gcm` como o navegador.
        pub(crate) fn decrypt(self, body: &[u8]) -> Vec<u8> {
            let salt = &body[..16];
            let rs = u32::from_be_bytes(body[16..20].try_into().unwrap());
            assert_eq!(rs, RECORD_SIZE);
            let idlen = body[20] as usize;
            assert_eq!(idlen, 65);
            let as_pub = &body[21..21 + idlen];
            let mut ct = body[21 + idlen..].to_vec();
            let secret = agreement::agree_ephemeral(
                self.ua_priv,
                &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, as_pub),
                |s| s.to_vec(),
            )
            .unwrap();
            let (cek, nonce) = derive_cek(&secret, &self.auth, &self.ua_pub, as_pub, salt);
            let key =
                aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
            let plain = key
                .open_in_place(
                    aead::Nonce::try_assume_unique_for_key(&nonce).unwrap(),
                    aead::Aad::empty(),
                    &mut ct,
                )
                .unwrap();
            assert_eq!(plain.last(), Some(&2), "delimitador do último registro");
            plain[..plain.len() - 1].to_vec()
        }
    }

    /// Confere o `Authorization: vapid t=..., k=...` e devolve as claims.
    pub(crate) fn check_vapid(authz: &str, public_key: &str) -> serde_json::Value {
        let rest = authz.strip_prefix("vapid t=").expect("esquema vapid");
        let (jwt, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, public_key);
        let (msg, sig) = jwt.rsplit_once('.').unwrap();
        let pk = b64_decode(k).unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &pk)
            .verify(msg.as_bytes(), &b64_decode(sig).unwrap())
            .expect("assinatura ES256 válida");
        let (h, c) = msg.split_once('.').unwrap();
        let h: serde_json::Value = serde_json::from_slice(&b64_decode(h).unwrap()).unwrap();
        assert_eq!(h["alg"], "ES256");
        serde_json::from_slice(&b64_decode(c).unwrap()).unwrap()
    }

    #[test]
    fn chave_vapid_gerada_uma_vez_com_0600() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vapid.key");
        let a = Vapid::load_or_create(&p, "mailto:eu@exemplo.com").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let b = Vapid::load_or_create(&p, "mailto:eu@exemplo.com").unwrap();
        assert_eq!(a.public_key(), b.public_key());
        assert_eq!(b64_decode(&a.public_key()).unwrap().len(), 65);
        std::fs::write(&p, "lixo!").unwrap();
        assert!(Vapid::load_or_create(&p, "x").is_err());
    }

    #[test]
    fn jwt_vapid_assinado_para_a_origem_do_endpoint() {
        let d = tempfile::tempdir().unwrap();
        let v = Vapid::load_or_create(&d.path().join("k"), "mailto:eu@exemplo.com").unwrap();
        let a = v
            .authorization("https://fcm.googleapis.com/fcm/send/abc", 1000)
            .unwrap();
        let c = check_vapid(&a, &v.public_key());
        assert_eq!(c["aud"], "https://fcm.googleapis.com");
        assert_eq!(c["sub"], "mailto:eu@exemplo.com");
        assert_eq!(c["exp"], 1000 + JWT_SECS);
        assert!(v.authorization("ftp://x/y", 1).is_none());
    }

    #[test]
    fn cifra_e_decifra_como_o_navegador() {
        let b = FakeBrowser::new();
        let body = encrypt(b"{\"title\":\"oi\"}", &b.ua_pub, &b.auth).unwrap();
        assert_eq!(b.decrypt(&body), b"{\"title\":\"oi\"}");
        let b = FakeBrowser::new();
        assert!(encrypt(b"x", &b.ua_pub[..64], &b.auth).is_err());
        assert!(encrypt(&[b'x'; 5000], &b.ua_pub, &b.auth).is_err());
    }

    /// Vetor do RFC 8291 §5 (derivação da chave e do nonce).
    #[test]
    fn derivacao_do_rfc_8291() {
        let d = |s: &str| b64_decode(s).unwrap();
        let ecdh = d("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs");
        let auth = d("BTBZMqHH6r4Tts7J_aSIgg");
        let ua = d("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let as_ = d("BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8");
        let salt = d("DGv6ra1nlYgDCS1FRnbzlw");
        let (cek, nonce) = derive_cek(&ecdh, &auth, &ua, &as_, &salt);
        assert_eq!(b64(&cek), "oIhVW04MRdy2XN9CiKLxTg");
        assert_eq!(b64(&nonce), "4h_95klXJ5E_qnoN");
    }

    #[test]
    fn subject_do_jwt() {
        let mut w = crate::config::WebConfig::default();
        assert_eq!(subject(&w), "mailto:cthubd@localhost");
        w.origins = vec!["http://127.0.0.1:7879".into(), "https://hub.ts.net/".into()];
        assert_eq!(subject(&w), "https://hub.ts.net");
        w.cloudflare = Some(crate::config::CloudflareConfig {
            team_domain: "t".into(),
            aud: "a".into(),
            emails: vec!["eu@exemplo.com".into()],
        });
        assert_eq!(subject(&w), "mailto:eu@exemplo.com");
        w.push.subject = "mailto:outro@exemplo.com".into();
        assert_eq!(subject(&w), "mailto:outro@exemplo.com");
    }
}

//! Destinos remotos do backup do hub (#263): S3 e compatíveis (inclui o
//! Backblaze B2 pela API S3) e Google Drive. Clientes próprios sobre `ureq`:
//! sem `rclone` no hub e sem runtime async novo (decisão registrada no
//! ledger da issue).
//!
//! Os objetos são os snapshots do `hub.db` (`hub-<ts>.db`, ou `.db.age`
//! cifrado), a cópia do `secrets.age` (`secrets-<ts>.age`, já cifrado) e o
//! pacote dos docs (`docs-<ts>.tar.zst`, ou `.tar.zst.age` cifrado, #442).

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::time::Duration;

/// Teto de um objeto lido do destino (snapshot do `hub.db`).
const MAX_DOWNLOAD: u64 = 4 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum RemoteKind {
    /// Só o disco local (o comportamento de antes).
    #[default]
    None,
    Drive,
    B2,
    S3,
}

/// Parte não secreta do destino remoto, guardada em `backup.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct RemoteSettings {
    pub kind: RemoteKind,
    /// Cifra com `age` (chave do hub + recuperação) antes de subir.
    pub encrypt: bool,
    /// S3/B2.
    pub bucket: String,
    /// S3/B2: prefixo das chaves (ex.: `cthub/`).
    pub prefix: String,
    pub region: String,
    /// S3/B2: endpoint customizado (MinIO, Wasabi, R2, B2). Vazio = AWS.
    pub endpoint: String,
    /// Drive: id da pasta dedicada onde os arquivos ficam.
    pub drive_folder_id: String,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        RemoteSettings {
            kind: RemoteKind::None,
            encrypt: true,
            bucket: String::new(),
            prefix: String::new(),
            region: "us-east-1".into(),
            endpoint: String::new(),
            drive_folder_id: String::new(),
        }
    }
}

/// Credenciais de um destino (nunca saem do hub nem aparecem em log).
#[derive(Default, Clone)]
pub struct Credentials {
    pub access_key: String,
    pub secret_key: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials(..)")
    }
}

/// Nomes das entradas `CTHUB_*` do store por destino e campo.
pub fn secret_name(kind: RemoteKind, field: &str) -> Option<&'static str> {
    Some(match (kind, field) {
        (RemoteKind::S3, "access_key") => "CTHUB_BACKUP_S3_ACCESS_KEY",
        (RemoteKind::S3, "secret_key") => "CTHUB_BACKUP_S3_SECRET_KEY",
        (RemoteKind::B2, "access_key") => "CTHUB_BACKUP_B2_ACCESS_KEY",
        (RemoteKind::B2, "secret_key") => "CTHUB_BACKUP_B2_SECRET_KEY",
        (RemoteKind::Drive, "client_id") => "CTHUB_BACKUP_DRIVE_CLIENT_ID",
        (RemoteKind::Drive, "client_secret") => "CTHUB_BACKUP_DRIVE_CLIENT_SECRET",
        (RemoteKind::Drive, "refresh_token") => "CTHUB_BACKUP_DRIVE_REFRESH_TOKEN",
        _ => return None,
    })
}

/// Campos de credencial que cada destino exige.
pub fn credential_fields(kind: RemoteKind) -> &'static [&'static str] {
    match kind {
        RemoteKind::None => &[],
        RemoteKind::S3 | RemoteKind::B2 => &["access_key", "secret_key"],
        RemoteKind::Drive => &["client_id", "client_secret", "refresh_token"],
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RemoteObject {
    pub name: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: u64,
    /// ms desde a época; 0 se o destino não informa.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub modified_ms: i64,
}

pub trait RemoteStore: Send + Sync {
    fn put(&self, name: &str, data: &[u8]) -> Result<(), String>;
    /// Objetos do destino (só o nível da pasta/prefixo configurado).
    fn list(&self) -> Result<Vec<RemoteObject>, String>;
    fn get(&self, name: &str) -> Result<Vec<u8>, String>;
    fn delete(&self, name: &str) -> Result<(), String>;
}

/// Abre o cliente do destino. Erro claro se falta configuração ou credencial.
pub fn open(s: &RemoteSettings, c: &Credentials) -> Result<Box<dyn RemoteStore>, String> {
    open_with(s, c, None)
}

/// Como [`open`], com a URL base do Drive/OAuth trocada (testes).
pub(crate) fn open_with(
    s: &RemoteSettings,
    c: &Credentials,
    drive_base: Option<&str>,
) -> Result<Box<dyn RemoteStore>, String> {
    let need = |v: &str, what: &str| {
        if v.trim().is_empty() {
            Err(format!("{what} não configurado"))
        } else {
            Ok(())
        }
    };
    match s.kind {
        RemoteKind::None => Err("sem destino remoto".into()),
        RemoteKind::S3 | RemoteKind::B2 => {
            need(&s.bucket, "bucket")?;
            need(&c.access_key, "access key")?;
            need(&c.secret_key, "secret key")?;
            if s.kind == RemoteKind::B2 {
                need(
                    &s.endpoint,
                    "endpoint do B2 (ex.: https://s3.us-west-004.backblazeb2.com)",
                )?;
            }
            Ok(Box::new(S3Store::new(s, c)))
        }
        RemoteKind::Drive => {
            need(&s.drive_folder_id, "pasta do Drive")?;
            need(&c.client_id, "client id")?;
            need(&c.client_secret, "client secret")?;
            need(&c.refresh_token, "refresh token")?;
            Ok(Box::new(DriveStore::new(s, c, drive_base)))
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .build()
}

/// Mensagem sem URL com query nem corpo: só o essencial.
fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let body: String = body.chars().take(300).collect();
            format!("HTTP {code}: {}", body.trim())
        }
        ureq::Error::Transport(t) => format!("rede: {}", t.kind()),
    }
}

fn read_all(resp: ureq::Response) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    resp.into_reader()
        .take(MAX_DOWNLOAD)
        .read_to_end(&mut out)
        .map_err(|e| format!("leitura: {e}"))?;
    Ok(out)
}

// ───────────────────────── S3 / B2 ─────────────────────────

pub struct S3Store {
    endpoint: String,
    region: String,
    bucket: String,
    prefix: String,
    access_key: String,
    secret_key: String,
    agent: ureq::Agent,
}

impl S3Store {
    fn new(s: &RemoteSettings, c: &Credentials) -> Self {
        let region = if s.region.trim().is_empty() {
            "us-east-1".to_string()
        } else {
            s.region.trim().to_string()
        };
        let endpoint = match s.endpoint.trim().trim_end_matches('/') {
            "" => format!("https://s3.{region}.amazonaws.com"),
            e => e.to_string(),
        };
        let prefix = s.prefix.trim().trim_matches('/');
        S3Store {
            endpoint,
            region,
            bucket: s.bucket.trim().to_string(),
            prefix: if prefix.is_empty() {
                String::new()
            } else {
                format!("{prefix}/")
            },
            access_key: c.access_key.clone(),
            secret_key: c.secret_key.clone(),
            agent: agent(),
        }
    }

    fn host(&self) -> &str {
        self.endpoint
            .split_once("://")
            .map_or(self.endpoint.as_str(), |(_, h)| h)
    }

    /// Requisição assinada (SigV4, path-style). `query` já em pares crus.
    fn request(
        &self,
        method: &str,
        key: &str,
        query: &[(&str, &str)],
        body: &[u8],
    ) -> Result<ureq::Response, String> {
        let amz_date = amz_date(now_secs());
        let day = &amz_date[..8];
        let payload_hash = hex(&Sha256::digest(body));
        let path = if key.is_empty() {
            format!("/{}", self.bucket)
        } else {
            format!("/{}/{}", self.bucket, key)
        };
        let canon_path = uri_encode(&path, false);
        let mut q: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| (uri_encode(k, true), uri_encode(v, true)))
            .collect();
        q.sort();
        let canon_query = q
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let host = self.host();
        let signed = "host;x-amz-content-sha256;x-amz-date";
        let canon = format!(
            "{method}\n{canon_path}\n{canon_query}\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n\n{signed}\n{payload_hash}"
        );
        let scope = format!("{day}/{}/s3/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex(&Sha256::digest(canon.as_bytes()))
        );
        let k = hmac(
            format!("AWS4{}", self.secret_key).as_bytes(),
            day.as_bytes(),
        );
        let k = hmac(&k, self.region.as_bytes());
        let k = hmac(&k, b"s3");
        let k = hmac(&k, b"aws4_request");
        let sig = hex(&hmac(&k, to_sign.as_bytes()));
        let auth = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={sig}",
            self.access_key
        );
        let mut url = format!("{}{canon_path}", self.endpoint);
        if !canon_query.is_empty() {
            url.push('?');
            url.push_str(&canon_query);
        }
        self.agent
            .request(method, &url)
            .set("x-amz-date", &amz_date)
            .set("x-amz-content-sha256", &payload_hash)
            .set("authorization", &auth)
            .send_bytes(body)
            .map_err(http_err)
    }

    fn key(&self, name: &str) -> String {
        format!("{}{name}", self.prefix)
    }
}

impl RemoteStore for S3Store {
    fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.request("PUT", &self.key(name), &[], data).map(|_| ())
    }

    fn list(&self) -> Result<Vec<RemoteObject>, String> {
        let mut out = Vec::new();
        let mut token = String::new();
        loop {
            let mut q = vec![("list-type", "2"), ("prefix", self.prefix.as_str())];
            if !token.is_empty() {
                q.push(("continuation-token", token.as_str()));
            }
            let body = self.request("GET", "", &q, b"")?;
            let xml = String::from_utf8_lossy(&read_all(body)?).into_owned();
            for blk in xml.split("<Contents>").skip(1) {
                let blk = blk.split("</Contents>").next().unwrap_or_default();
                let (Some(key), size) = (xml_tag(blk, "Key"), xml_tag(blk, "Size")) else {
                    continue;
                };
                let Some(name) = key.strip_prefix(&self.prefix) else {
                    continue;
                };
                if name.is_empty() || name.contains('/') {
                    continue;
                }
                out.push(RemoteObject {
                    name: name.to_string(),
                    size: size.and_then(|s| s.parse().ok()).unwrap_or(0),
                    modified_ms: xml_tag(blk, "LastModified")
                        .and_then(|d| parse_iso_ms(&d))
                        .unwrap_or(0),
                });
            }
            if xml_tag(&xml, "IsTruncated").as_deref() == Some("true") {
                match xml_tag(&xml, "NextContinuationToken") {
                    Some(t) => token = t,
                    None => break,
                }
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        read_all(self.request("GET", &self.key(name), &[], b"")?)
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        self.request("DELETE", &self.key(name), &[], b"")
            .map(|_| ())
    }
}

fn xml_tag(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let a = s.find(&open)? + open.len();
    let b = s[a..].find(&format!("</{tag}>"))? + a;
    Some(
        s[a..b]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("HMAC aceita qualquer tamanho");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Codificação da SigV4: só `A-Za-z0-9-_.~` passam (e `/` no caminho).
fn uri_encode(s: &str, encode_slash: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if !encode_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn now_secs() -> i64 {
    ct_core::now_ms() / 1000
}

/// `YYYYMMDDTHHMMSSZ` (UTC).
fn amz_date(secs: i64) -> String {
    let (y, m, d) = civil(secs.div_euclid(86400));
    let t = secs.rem_euclid(86400);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

/// Dias desde 1970-01-01 → (ano, mês, dia) (algoritmo de Hinnant).
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `2026-10-23T10:11:12.000Z` → ms.
fn parse_iso_ms(s: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let days = days_from_civil(n(0..4)?, n(5..7)?, n(8..10)?);
    Some(((days * 86400) + n(11..13)? * 3600 + n(14..16)? * 60 + n(17..19)?) * 1000)
}

// ───────────────────────── Google Drive ─────────────────────────

pub struct DriveStore {
    folder: String,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    api: String,
    upload: String,
    token_url: String,
    agent: ureq::Agent,
}

impl DriveStore {
    fn new(s: &RemoteSettings, c: &Credentials, base: Option<&str>) -> Self {
        let (api, upload, token_url) = match base {
            Some(b) => (
                format!("{b}/drive/v3"),
                format!("{b}/upload/drive/v3"),
                format!("{b}/token"),
            ),
            None => (
                "https://www.googleapis.com/drive/v3".into(),
                "https://www.googleapis.com/upload/drive/v3".into(),
                "https://oauth2.googleapis.com/token".into(),
            ),
        };
        DriveStore {
            folder: s.drive_folder_id.trim().to_string(),
            client_id: c.client_id.clone(),
            client_secret: c.client_secret.clone(),
            refresh_token: c.refresh_token.clone(),
            api,
            upload,
            token_url,
            agent: agent(),
        }
    }

    /// Token de acesso novo a cada operação (o ciclo é de horas). Token
    /// revogado vira erro com a causa, para o aviso de credencial inválida.
    fn token(&self) -> Result<String, String> {
        let r = self
            .agent
            .post(&self.token_url)
            .send_form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("refresh_token", &self.refresh_token),
            ])
            .map_err(|e| match e {
                ureq::Error::Status(400 | 401, _) => {
                    "credencial do Drive inválida ou revogada (gere um refresh token novo)".into()
                }
                e => http_err(e),
            })?;
        let v: serde_json::Value = r
            .into_json()
            .map_err(|e| format!("resposta do OAuth: {e}"))?;
        v["access_token"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "resposta do OAuth sem access_token".into())
    }

    fn find(&self, tok: &str, name: &str) -> Result<Vec<String>, String> {
        let q = format!(
            "'{}' in parents and name = '{}' and trashed = false",
            self.folder,
            name.replace('\\', "\\\\").replace('\'', "\\'")
        );
        let r = self
            .agent
            .get(&format!("{}/files", self.api))
            .set("authorization", &format!("Bearer {tok}"))
            .query("q", &q)
            .query("fields", "files(id)")
            .call()
            .map_err(http_err)?;
        let v: serde_json::Value = r.into_json().map_err(|e| e.to_string())?;
        Ok(v["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f["id"].as_str().map(str::to_string))
            .collect())
    }
}

impl RemoteStore for DriveStore {
    fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let tok = self.token()?;
        // Mesmo nome = substitui (um reenvio não duplica).
        for id in self.find(&tok, name)? {
            self.delete_id(&tok, &id)?;
        }
        let boundary = "ctower-backup-boundary-7f3a";
        let meta = serde_json::json!({ "name": name, "parents": [self.folder] });
        let mut body = format!(
            "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());
        self.agent
            .post(&format!("{}/files", self.upload))
            .set("authorization", &format!("Bearer {tok}"))
            .set(
                "content-type",
                &format!("multipart/related; boundary={boundary}"),
            )
            .query("uploadType", "multipart")
            .send_bytes(&body)
            .map(|_| ())
            .map_err(http_err)
    }

    fn list(&self) -> Result<Vec<RemoteObject>, String> {
        let tok = self.token()?;
        let mut out = Vec::new();
        let mut page = String::new();
        loop {
            let mut req = self
                .agent
                .get(&format!("{}/files", self.api))
                .set("authorization", &format!("Bearer {tok}"))
                .query(
                    "q",
                    &format!("'{}' in parents and trashed = false", self.folder),
                )
                .query("fields", "nextPageToken,files(name,size,modifiedTime)")
                .query("pageSize", "1000");
            if !page.is_empty() {
                req = req.query("pageToken", &page);
            }
            let v: serde_json::Value = req
                .call()
                .map_err(http_err)?
                .into_json()
                .map_err(|e| e.to_string())?;
            for f in v["files"].as_array().into_iter().flatten() {
                let Some(name) = f["name"].as_str() else {
                    continue;
                };
                out.push(RemoteObject {
                    name: name.to_string(),
                    size: f["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
                    modified_ms: f["modifiedTime"]
                        .as_str()
                        .and_then(parse_iso_ms)
                        .unwrap_or(0),
                });
            }
            match v["nextPageToken"].as_str() {
                Some(t) => page = t.to_string(),
                None => break,
            }
        }
        Ok(out)
    }

    fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        let tok = self.token()?;
        let id = self
            .find(&tok, name)?
            .into_iter()
            .next()
            .ok_or_else(|| format!("{name} não existe no Drive"))?;
        read_all(
            self.agent
                .get(&format!("{}/files/{id}", self.api))
                .set("authorization", &format!("Bearer {tok}"))
                .query("alt", "media")
                .call()
                .map_err(http_err)?,
        )
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        let tok = self.token()?;
        for id in self.find(&tok, name)? {
            self.delete_id(&tok, &id)?;
        }
        Ok(())
    }
}

impl DriveStore {
    fn delete_id(&self, tok: &str, id: &str) -> Result<(), String> {
        self.agent
            .delete(&format!("{}/files/{id}", self.api))
            .set("authorization", &format!("Bearer {tok}"))
            .call()
            .map(|_| ())
            .map_err(http_err)
    }
}

// ───────────────────────── cifra e nomes ─────────────────────────

/// Cifra para o hub e a chave de recuperação (formato `age`).
pub fn seal(data: &[u8], recipients: &[ct_secrets::Recipient]) -> Result<Vec<u8>, String> {
    ct_secrets::encrypt(data, recipients).map_err(|e| format!("cifra do backup: {e}"))
}

/// Objetos de snapshot do `hub.db`, do mais novo para o mais velho.
pub fn hub_snapshots(mut objs: Vec<RemoteObject>) -> Vec<RemoteObject> {
    objs.retain(|o| o.name.starts_with("hub-"));
    // O nome carrega o timestamp: ordem lexicográfica = cronológica.
    objs.sort_by(|a, b| b.name.cmp(&a.name));
    objs
}

/// Pacotes dos docs (`docs-<ts>.tar.zst[.age]`, #442), do mais novo para
/// o mais velho.
pub fn docs_packages(mut objs: Vec<RemoteObject>) -> Vec<RemoteObject> {
    objs.retain(|o| o.name.starts_with(crate::docs_backup::PREFIX));
    objs.sort_by(|a, b| b.name.cmp(&a.name));
    objs
}

/// Nome remoto do snapshot local `file` (acrescenta `.age` se cifrado).
pub fn remote_name(file: &str, encrypted: bool) -> String {
    if encrypted {
        format!("{file}.age")
    } else {
        file.to_string()
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! Servidor HTTP mínimo em memória que fala o suficiente de S3 e Drive
    //! para os testes (sem checar a assinatura além de existir).
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub struct State {
        /// chave/nome → conteúdo
        pub objects: BTreeMap<String, Vec<u8>>,
        pub saw_auth: bool,
        /// Falha tudo com 403.
        pub deny: bool,
    }

    pub fn serve() -> (String, Arc<Mutex<State>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let st = Arc::new(Mutex::new(State::default()));
        let s2 = Arc::clone(&st);
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let s = Arc::clone(&s2);
                std::thread::spawn(move || handle(c, s));
            }
        });
        (url, st)
    }

    fn handle(mut c: std::net::TcpStream, st: Arc<Mutex<State>>) {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        let head_end = loop {
            let n = c.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break p + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let mut lines = head.lines();
        let mut first = lines.next().unwrap().split(' ');
        let (method, target) = (first.next().unwrap(), first.next().unwrap().to_string());
        let hdr = |k: &str| {
            head.lines()
                .find_map(|l| {
                    let (a, b) = l.split_once(':')?;
                    a.eq_ignore_ascii_case(k).then(|| b.trim().to_string())
                })
                .unwrap_or_default()
        };
        let len: usize = hdr("content-length").parse().unwrap_or(0);
        while buf.len() < head_end + len {
            let n = c.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let body = buf[head_end..].to_vec();
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        let auth = hdr("authorization");
        let mut g = st.lock().unwrap();
        let (code, ctype, resp): (u16, &str, Vec<u8>) = if g.deny {
            (403, "text/plain", b"AccessDenied".to_vec())
        } else if path.starts_with("/token") {
            (
                200,
                "application/json",
                br#"{"access_token":"tok"}"#.to_vec(),
            )
        } else if path.starts_with("/drive/v3/files") || path.starts_with("/upload/drive") {
            drive(
                &mut g,
                method,
                path,
                query,
                &auth,
                &hdr("content-type"),
                &body,
            )
        } else {
            s3(&mut g, method, path, query, &auth, &body)
        };
        let _ = write!(
            c,
            "HTTP/1.1 {code} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            resp.len()
        );
        let _ = c.write_all(&resp);
    }

    fn s3(
        g: &mut State,
        method: &str,
        path: &str,
        query: &str,
        auth: &str,
        body: &[u8],
    ) -> (u16, &'static str, Vec<u8>) {
        g.saw_auth |= auth.starts_with("AWS4-HMAC-SHA256 Credential=");
        let rest = path.trim_start_matches('/');
        let (_bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
        match method {
            "PUT" => {
                g.objects.insert(key.to_string(), body.to_vec());
                (200, "text/plain", vec![])
            }
            "DELETE" => {
                g.objects.remove(key);
                (204, "text/plain", vec![])
            }
            "GET" if key.is_empty() => {
                let prefix = query
                    .split('&')
                    .find_map(|p| p.strip_prefix("prefix="))
                    .unwrap_or("")
                    .replace("%2F", "/");
                let mut x = String::from("<ListBucketResult><IsTruncated>false</IsTruncated>");
                for (k, v) in g.objects.iter().filter(|(k, _)| k.starts_with(&prefix)) {
                    x.push_str(&format!(
                        "<Contents><Key>{k}</Key><LastModified>2026-10-23T10:11:12.000Z</LastModified><Size>{}</Size></Contents>",
                        v.len()
                    ));
                }
                x.push_str("</ListBucketResult>");
                (200, "application/xml", x.into_bytes())
            }
            "GET" => match g.objects.get(key) {
                Some(v) => (200, "application/octet-stream", v.clone()),
                None => (404, "text/plain", b"NoSuchKey".to_vec()),
            },
            _ => (405, "text/plain", vec![]),
        }
    }

    fn drive(
        g: &mut State,
        method: &str,
        path: &str,
        query: &str,
        auth: &str,
        ctype: &str,
        body: &[u8],
    ) -> (u16, &'static str, Vec<u8>) {
        g.saw_auth |= auth == "Bearer tok";
        let dec = |s: &str| {
            let mut o = Vec::new();
            let b = s.as_bytes();
            let mut i = 0;
            while i < b.len() {
                match b[i] {
                    b'%' if i + 2 < b.len() => {
                        o.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?'));
                        i += 3;
                    }
                    b'+' => {
                        o.push(b' ');
                        i += 1;
                    }
                    c => {
                        o.push(c);
                        i += 1;
                    }
                }
            }
            String::from_utf8_lossy(&o).to_string()
        };
        let q = |k: &str| {
            query
                .split('&')
                .find_map(|p| p.strip_prefix(&format!("{k}=")))
                .map(dec)
                .unwrap_or_default()
        };
        if path.starts_with("/upload") && method == "POST" {
            let boundary = ctype.split("boundary=").nth(1).unwrap_or_default();
            let text = String::from_utf8_lossy(body).to_string();
            let parts: Vec<&str> = text.split(&format!("--{boundary}")).collect();
            let meta: serde_json::Value =
                serde_json::from_str(parts[1].split("\r\n\r\n").nth(1).unwrap().trim()).unwrap();
            let p2 = parts[2];
            let data = p2.split_once("\r\n\r\n").unwrap().1;
            let data = data.strip_suffix("\r\n").unwrap_or(data);
            g.objects.insert(
                meta["name"].as_str().unwrap().into(),
                data.as_bytes().to_vec(),
            );
            return (200, "application/json", b"{}".to_vec());
        }
        let id = path.strip_prefix("/drive/v3/files/");
        match (method, id) {
            ("GET", None) => {
                let qq = q("q");
                let name = qq
                    .split("name = '")
                    .nth(1)
                    .and_then(|s| s.split('\'').next());
                let files: Vec<_> = g
                    .objects
                    .iter()
                    .filter(|(k, _)| name.is_none_or(|n| n == k.as_str()))
                    .map(|(k, v)| {
                        serde_json::json!({"id": k, "name": k, "size": v.len().to_string(),
                        "modifiedTime": "2026-10-23T10:11:12.000Z"})
                    })
                    .collect();
                (
                    200,
                    "application/json",
                    serde_json::json!({ "files": files })
                        .to_string()
                        .into_bytes(),
                )
            }
            ("GET", Some(id)) => match g.objects.get(id) {
                Some(v) => (200, "application/octet-stream", v.clone()),
                None => (404, "text/plain", vec![]),
            },
            ("DELETE", Some(id)) => {
                g.objects.remove(id);
                (204, "text/plain", vec![])
            }
            _ => (405, "text/plain", vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s3_settings(url: &str, kind: RemoteKind) -> RemoteSettings {
        RemoteSettings {
            kind,
            bucket: "bk".into(),
            prefix: "cthub/".into(),
            endpoint: url.into(),
            ..Default::default()
        }
    }

    fn creds() -> Credentials {
        Credentials {
            access_key: "AK".into(),
            secret_key: "SK".into(),
            client_id: "cid".into(),
            client_secret: "cs".into(),
            refresh_token: "rt".into(),
        }
    }

    #[test]
    fn datas_e_codificacao() {
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(amz_date(1_792_760_485), "20261023T130125Z");
        assert_eq!(parse_iso_ms("1970-01-02T00:00:01.000Z"), Some(86_401_000));
        assert_eq!(uri_encode("/a b/ç.db", false), "/a%20b/%C3%A7.db");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
    }

    /// Vetor oficial da AWS (get-vanilla): a assinatura da cadeia de HMACs.
    #[test]
    fn cadeia_sigv4() {
        let k = hmac(b"AWS4wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", b"20150830");
        let k = hmac(&k, b"us-east-1");
        let k = hmac(&k, b"iam");
        let k = hmac(&k, b"aws4_request");
        let to_sign = "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/iam/aws4_request\nf536975d06c0309214f805bb90ccff089219ecd68b2577efef23edd43b7e1a59";
        assert_eq!(
            hex(&hmac(&k, to_sign.as_bytes())),
            "5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    #[test]
    fn s3_e_b2_ida_e_volta() {
        for kind in [RemoteKind::S3, RemoteKind::B2] {
            let (url, st) = fake::serve();
            let r = open(&s3_settings(&url, kind), &creds()).unwrap();
            r.put("hub-2.db", b"dois").unwrap();
            r.put("hub-1.db", b"um").unwrap();
            assert!(st.lock().unwrap().saw_auth);
            assert!(st.lock().unwrap().objects.contains_key("cthub/hub-1.db"));
            let objs = hub_snapshots(r.list().unwrap());
            assert_eq!(
                objs.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
                ["hub-2.db", "hub-1.db"]
            );
            assert_eq!((objs[0].size, objs[0].modified_ms > 0), (4, true));
            assert_eq!(r.get("hub-1.db").unwrap(), b"um");
            r.delete("hub-1.db").unwrap();
            assert_eq!(r.list().unwrap().len(), 1);
        }
    }

    #[test]
    fn s3_erro_do_provedor_e_config_faltando() {
        let (url, st) = fake::serve();
        st.lock().unwrap().deny = true;
        let r = open(&s3_settings(&url, RemoteKind::S3), &creds()).unwrap();
        let e = r.put("x", b"1").unwrap_err();
        assert!(e.contains("403"), "{e}");
        // A chave não aparece na mensagem.
        assert!(!e.contains("SK"));

        let mut s = s3_settings(&url, RemoteKind::S3);
        s.bucket.clear();
        assert!(open(&s, &creds()).is_err());
        assert!(open(&s3_settings(&url, RemoteKind::S3), &Credentials::default()).is_err());
        let mut b2 = s3_settings(&url, RemoteKind::B2);
        b2.endpoint.clear();
        assert!(open(&b2, &creds()).err().unwrap().contains("endpoint"));
    }

    #[test]
    fn drive_ida_e_volta() {
        let (url, st) = fake::serve();
        let s = RemoteSettings {
            kind: RemoteKind::Drive,
            drive_folder_id: "pasta".into(),
            ..Default::default()
        };
        let r = open_with(&s, &creds(), Some(&url)).unwrap();
        r.put("hub-1.db", b"um").unwrap();
        // Reenvio com o mesmo nome substitui.
        r.put("hub-1.db", b"um-novo").unwrap();
        r.put("hub-2.db", b"dois").unwrap();
        assert!(st.lock().unwrap().saw_auth);
        let objs = hub_snapshots(r.list().unwrap());
        assert_eq!(objs.len(), 2);
        assert_eq!(r.get("hub-1.db").unwrap(), b"um-novo");
        r.delete("hub-1.db").unwrap();
        assert_eq!(r.list().unwrap().len(), 1);
        assert!(r.get("hub-1.db").is_err());
    }

    #[test]
    fn cifra_so_abre_com_a_chave() {
        let id = ct_secrets::Identity::generate();
        let out = seal(b"segredo do hub", &[id.to_public()]).unwrap();
        assert!(!out.windows(6).any(|w| w == b"segred"));
        assert_eq!(&*ct_secrets::decrypt(&out, &id).unwrap(), b"segredo do hub");
    }
}

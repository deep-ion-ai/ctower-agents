//! `glab` falso para testes (spec §12): script que entra pelo `glab_bin` do
//! `config.toml`, como o `jira` falso entra pelo `jira_bin`.
//!
//! Tudo fica em `<dir>/glab-fake/`:
//! - `glab-calls`: uma linha por chamada, `argv` separado por tab, mais
//!   `token=ok|outra|ausente` (o token em si nunca é gravado) e
//!   `host=<GITLAB_HOST>|ausente`;
//! - `mrs/<branch>.json`: a resposta do `mr list -s <branch> -F json` (sem o
//!   arquivo, `[]`);
//! - `branches/<branch codificada>.json`: a branch do `glab api
//!   projects/<p>/repository/branches/<b>` (sem o arquivo, 404). O `DELETE`
//!   apaga o arquivo e registra o caminho em `deletes`;
//! - `mr/<iid>.json`: o `glab api projects/<p>/merge_requests/<iid>`;
//! - falhas por arquivo: `fail-401` (o GitLab recusa o token),
//!   `echo-token` (o erro ecoa o token, para conferir a redação) e `sleep`
//!   (segundos antes de responder, para o prazo).
//!
//! Os comandos que falam com o GitLab só respondem com `GITLAB_TOKEN` igual
//! ao esperado e `GITLAB_HOST` igual ao host do cadastro; senão, dão o 401
//! do glab.

use crate::encode;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct FakeGlab {
    dir: PathBuf,
    bin: PathBuf,
}

impl FakeGlab {
    /// Cria o script em `<dir>/glab-fake/glab`, esperando o host `host` e o
    /// token `token`.
    pub fn new(dir: &Path, host: &str, token: &str) -> Self {
        let dir = dir.join("glab-fake");
        for d in ["mrs", "branches", "mr"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("token"), token).unwrap();
        std::fs::write(dir.join("host"), host).unwrap();
        let bin = dir.join("glab");
        write_exec(&bin, &script(&dir));
        FakeGlab { dir, bin }
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Os MRs do `mr list` da branch.
    pub fn set_mrs(&self, branch: &str, mrs: &[Value]) {
        let f = self.dir.join("mrs").join(format!("{branch}.json"));
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, Value::Array(mrs.to_vec()).to_string()).unwrap();
    }

    /// O MR no `glab api projects/<p>/merge_requests/<iid>`.
    pub fn set_mr(&self, mr: &Value) {
        let iid = mr["iid"].as_i64().expect("MR sem iid");
        std::fs::write(
            self.dir.join("mr").join(format!("{iid}.json")),
            mr.to_string(),
        )
        .unwrap();
    }

    /// A branch publicada no GitLab falso, na ponta `sha`.
    pub fn set_branch(&self, branch: &str, sha: &str) {
        std::fs::write(
            self.dir
                .join("branches")
                .join(format!("{}.json", encode(branch))),
            json!({ "name": branch, "commit": { "id": sha } }).to_string(),
        )
        .unwrap();
    }

    /// Os caminhos apagados com `DELETE`, na ordem.
    pub fn deletes(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("deletes"))
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default()
    }

    fn flag(&self, name: &str, on: bool) {
        let p = self.dir.join(name);
        if on {
            std::fs::write(p, "").unwrap();
        } else {
            let _ = std::fs::remove_file(p);
        }
    }

    pub fn fail_401(&self, on: bool) {
        self.flag("fail-401", on);
    }

    /// O `mr list` falha ecoando o token na mensagem de erro.
    pub fn echo_token_on_error(&self, on: bool) {
        self.flag("echo-token", on);
    }

    /// Segundos de espera antes de cada resposta; `None` = responde já.
    pub fn set_sleep(&self, secs: Option<u32>) {
        match secs {
            Some(s) => std::fs::write(self.dir.join("sleep"), s.to_string()).unwrap(),
            None => {
                let _ = std::fs::remove_file(self.dir.join("sleep"));
            }
        }
    }

    /// As chamadas registradas, uma por linha.
    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("glab-calls"))
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default()
    }

    pub fn clear_calls(&self) {
        let _ = std::fs::remove_file(self.dir.join("glab-calls"));
    }
}

/// MR da API v4 com os campos que o sync lê. `state` é o do GitLab
/// (`opened`, `merged`, `closed`, `locked`).
pub fn api_mr(host: &str, path: &str, iid: i64, branch: &str, state: &str, sha: &str) -> Value {
    json!({
        "id": 90_000 + iid,
        "iid": iid,
        "project_id": 42,
        "title": format!("MR {iid}"),
        "state": state,
        "source_branch": branch,
        "target_branch": "develop",
        "web_url": format!("https://{host}/{path}/-/merge_requests/{iid}"),
        "updated_at": "2026-10-28T13:15:30.123Z",
        "sha": sha,
    })
}

fn script(dir: &Path) -> String {
    format!(
        r#"#!/bin/sh
d='{dir}'
tok=ausente
if [ -n "${{GITLAB_TOKEN+x}}" ]; then
  if [ "$GITLAB_TOKEN" = "$(cat "$d/token")" ]; then tok=ok; else tok=outra; fi
fi
# Uma escrita só por linha: as chamadas em paralelo não se misturam.
line=glab
tab=$(printf '\t')
for a in "$@"; do line="$line$tab$a"; done
line="$line${{tab}}token=$tok${{tab}}host=${{GITLAB_HOST:-ausente}}"
printf '%s\n' "$line" >> "$d/glab-calls"
if [ -f "$d/sleep" ]; then sleep "$(cat "$d/sleep")"; fi
if [ "$1" = version ]; then
  echo 'glab 1.124.0 (fake)'; exit 0
fi
if [ -f "$d/echo-token" ]; then
  echo "glab: falha com o token $GITLAB_TOKEN" >&2; exit 1
fi
if [ -f "$d/fail-401" ] || [ "$tok" != ok ] || [ "${{GITLAB_HOST:-}}" != "$(cat "$d/host")" ]; then
  echo 'glab: 401 Unauthorized (HTTP 401)' >&2; exit 1
fi
case "$1" in
  mr)
    if [ "$2" = list ]; then
      branch=; prev=
      for a in "$@"; do
        if [ "$prev" = -s ]; then branch="$a"; fi
        prev="$a"
      done
      f="$d/mrs/$branch.json"
      if [ -f "$f" ]; then cat "$f"; else echo '[]'; fi
      exit 0
    fi ;;
  api)
    ep="$2"; method=GET; prev=
    for a in "$@"; do
      if [ "$prev" = -X ]; then method="$a"; fi
      prev="$a"
    done
    case "$ep" in
      version)
        echo '{{"version":"17.5.0","revision":"fake"}}'; exit 0 ;;
      projects/*/repository/branches/*)
        f="$d/branches/${{ep##*/}}.json"
        if [ ! -f "$f" ]; then echo 'glab: 404 Branch Not Found (HTTP 404)' >&2; exit 1; fi
        if [ "$method" = DELETE ]; then
          rm -f "$f"; printf '%s\n' "$ep" >> "$d/deletes"; exit 0
        fi
        cat "$f"; exit 0 ;;
      projects/*/merge_requests/*)
        f="$d/mr/${{ep##*/}}.json"
        if [ -f "$f" ]; then cat "$f"; exit 0; fi
        echo 'glab: 404 Not found (HTTP 404)' >&2; exit 1 ;;
    esac ;;
esac
echo "comando não suportado pelo glab falso: $*" >&2
exit 2
"#,
        dir = dir.display()
    )
}

/// Grava o script executável por um `sh` filho, e não por este processo: um
/// `fork` concorrente de outro teste herdaria o descritor de escrita até o
/// seu `exec`, e executar o script nesse intervalo dá ETXTBSY ("Text file
/// busy") no Linux.
fn write_exec(path: &Path, body: &str) {
    let ok = std::process::Command::new("sh")
        .args([
            "-c",
            "printf '%s' \"$2\" > \"$1\" && chmod 755 \"$1\"",
            "sh",
        ])
        .arg(path)
        .arg(body)
        .status()
        .unwrap()
        .success();
    assert!(ok, "gravar {}", path.display());
}

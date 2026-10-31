//! `jira` falso para testes (spec §12): script que entra pelo `jira_bin` do
//! `config.toml`, como o `claude` falso entra pelo `claude_bin`.
//!
//! Tudo fica em `<dir>/jira-fake/`:
//! - `jira-calls`: uma linha por chamada, `argv` separado por tab, mais
//!   `token=senha-ok|outra|ausente` (a senha em si nunca é gravada) e
//!   `config=<JIRA_CONFIG_FILE>|ausente`;
//! - `pages/<n>.json`: a página `n` do `issue list --raw` com `--reverse`
//!   (ordem crescente de `updated`; sem o arquivo, a página vem vazia, com o
//!   "No result found" do jira-cli), e `pages-desc/<n>.json` a mesma
//!   listagem sem o `--reverse` (decrescente);
//! - `queries`: `<pasta>\t<jql>` por linha, o `issue list -q <jql>` que lê
//!   as páginas de `<pasta>/` em vez de `pages/` (`<pasta>/fail` = falha);
//! - `<pasta>/<n>.flaky`: `<vezes> <modo>`, as próximas `<vezes>` chamadas
//!   da página `n` falham antes de responder, por `sleep:<s>` (prazo), `503`
//!   (Jira fora do ar) ou `401` (senha recusada);
//! - `issues/<KEY>.json`: o JSON da API do `issue view <KEY> --raw`;
//! - `issues/<KEY>.401`: o `issue view <KEY>` dá o 401 (senha recusada no
//!   meio do ciclo);
//! - falhas por arquivo: `fail-401` (o Jira recusa a senha), `fail-me`,
//!   `no-config` (configuração ausente) e `sleep` (segundos antes de
//!   responder, para o prazo).
//!
//! Os comandos que falam com o Jira só respondem com `JIRA_API_TOKEN` igual
//! à senha esperada; senão, dão o 401 do jira-cli.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct FakeJira {
    dir: PathBuf,
    bin: PathBuf,
}

/// Servidor do `.config.yml` falso.
pub const FAKE_SERVER: &str = "https://jira.exemplo.test";

impl FakeJira {
    /// Cria o script em `<dir>/jira-fake/jira`, esperando a senha `token`.
    pub fn new(dir: &Path, token: &str) -> Self {
        let dir = dir.join("jira-fake");
        std::fs::create_dir_all(dir.join("pages")).unwrap();
        std::fs::create_dir_all(dir.join("issues")).unwrap();
        std::fs::write(dir.join("token"), token).unwrap();
        std::fs::write(
            dir.join("config.yml"),
            format!("installation: Local\nserver: {FAKE_SERVER}\nlogin: usuario\n"),
        )
        .unwrap();
        let bin = dir.join("jira");
        write_exec(&bin, &script(&dir));
        FakeJira { dir, bin }
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// `.config.yml` do jira-cli falso (com `server: FAKE_SERVER`).
    pub fn config_file(&self) -> PathBuf {
        self.dir.join("config.yml")
    }

    /// Publica as issues (JSON da API): cada uma no `issue view` e, na
    /// ordem dada (a crescente de `updated`, do `--reverse`), nas páginas
    /// do `issue list` (na forma reduzida do jira-cli, com chave, resumo,
    /// tipo e `updated`); sem o `--reverse`, na ordem inversa.
    pub fn set_issues(&self, issues: &[Value]) {
        for d in ["pages", "pages-desc"] {
            let _ = std::fs::remove_dir_all(self.dir.join(d));
            std::fs::create_dir_all(self.dir.join(d)).unwrap();
        }
        for v in issues {
            self.set_view(v);
        }
        for (n, page) in issues.chunks(crate::PAGE_SIZE).enumerate() {
            let reduced: Vec<Value> = page.iter().map(reduce).collect();
            self.set_page(n, &Value::Array(reduced));
        }
        let desc: Vec<Value> = issues.iter().rev().map(reduce).collect();
        for (n, page) in desc.chunks(crate::PAGE_SIZE).enumerate() {
            std::fs::write(
                self.dir.join("pages-desc").join(format!("{n}.json")),
                Value::Array(page.to_vec()).to_string(),
            )
            .unwrap();
        }
    }

    /// Só o `issue view` da issue (sem mexer nas páginas).
    pub fn set_view(&self, v: &Value) {
        let key = v["key"].as_str().expect("issue sem key");
        std::fs::write(
            self.dir.join("issues").join(format!("{key}.json")),
            v.to_string(),
        )
        .unwrap();
    }

    /// Tira a issue do `issue view` (dá o 404 do jira-cli), sem mexer nas
    /// páginas.
    pub fn remove_view(&self, key: &str) {
        let _ = std::fs::remove_file(self.dir.join("issues").join(format!("{key}.json")));
    }

    /// O `issue view` da chave dá o 401 (`on`) ou volta ao normal.
    pub fn fail_view_401(&self, key: &str, on: bool) {
        self.flag(&format!("issues/{key}.401"), on);
    }

    /// O `issue list` com exatamente este `-q` devolve só estas issues (na
    /// forma reduzida), ou falha (`None`); os outros filtros seguem com as
    /// páginas do `set_issues`.
    pub fn set_list_for(&self, jql: &str, issues: Option<&[Value]>) {
        let queries = self.dir.join("queries");
        let lines = std::fs::read_to_string(&queries).unwrap_or_default();
        let n = lines.lines().count();
        let name = lines
            .lines()
            .find_map(|l| {
                l.split_once('\t')
                    .filter(|(_, q)| *q == jql)
                    .map(|(d, _)| d.to_string())
            })
            .unwrap_or_else(|| {
                let d = format!("q{n}");
                std::fs::write(&queries, format!("{lines}{d}\t{jql}\n")).unwrap();
                d
            });
        let dir = self.dir.join(&name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        match issues {
            None => std::fs::write(dir.join("fail"), "").unwrap(),
            Some(issues) => {
                for (n, page) in issues.chunks(crate::PAGE_SIZE).enumerate() {
                    let reduced: Vec<Value> = page.iter().map(reduce).collect();
                    std::fs::write(
                        dir.join(format!("{n}.json")),
                        Value::Array(reduced).to_string(),
                    )
                    .unwrap();
                }
            }
        }
    }

    /// As próximas `times` chamadas da página `n` do `issue list` (das
    /// páginas do `set_issues`, nas duas ordens, cada uma com a sua conta)
    /// falham com `mode`: `"sleep:<s>"` (dorme,
    /// para estourar o prazo), `"503"` ou `"401"`. Chame depois do
    /// `set_issues`, que apaga as páginas.
    pub fn flaky_page(&self, n: usize, times: u32, mode: &str) {
        for d in ["pages", "pages-desc"] {
            let _ = std::fs::write(
                self.dir.join(d).join(format!("{n}.flaky")),
                format!("{times} {mode}"),
            );
        }
    }

    /// Chamadas do `issue list` da página com este `--paginate` offset.
    pub fn page_calls(&self, offset: usize) -> usize {
        let p = format!("\t--paginate\t{offset}:");
        self.calls()
            .iter()
            .filter(|c| c.contains("\tissue\tlist\t") && c.contains(&p))
            .count()
    }

    pub fn set_page(&self, n: usize, page: &Value) {
        std::fs::write(
            self.dir.join("pages").join(format!("{n}.json")),
            page.to_string(),
        )
        .unwrap();
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

    pub fn fail_me(&self, on: bool) {
        self.flag("fail-me", on);
    }

    pub fn no_config(&self, on: bool) {
        self.flag("no-config", on);
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
        std::fs::read_to_string(self.dir.join("jira-calls"))
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default()
    }

    pub fn clear_calls(&self) {
        let _ = std::fs::remove_file(self.dir.join("jira-calls"));
    }

    /// Os `-q` das listagens registradas.
    pub fn list_filters(&self) -> Vec<String> {
        self.calls()
            .iter()
            .filter(|c| c.contains("\tissue\tlist\t"))
            .filter_map(|c| {
                let parts: Vec<&str> = c.split('\t').collect();
                let i = parts.iter().position(|p| *p == "-q")?;
                parts.get(i + 1).map(|s| s.to_string())
            })
            .collect()
    }

    /// Chaves lidas com `issue view`.
    pub fn viewed(&self) -> Vec<String> {
        self.calls()
            .iter()
            .filter_map(|c| {
                let parts: Vec<&str> = c.split('\t').collect();
                (parts.get(1..3) == Some(&["issue", "view"][..])).then(|| parts[3].to_string())
            })
            .collect()
    }
}

/// Issue da API v2 com os campos que o sync lê. `category` é a
/// `statusCategory.key` (`new`, `indeterminate`, `done`); o status vem em
/// português; prioridade e criador preenchidos, sem responsável. Mude o
/// resto com `v["fields"][...] = ...`.
pub fn api_issue(key: &str, type_name: &str, category: &str) -> Value {
    let status = match category {
        "done" => "Concluído",
        "indeterminate" => "Em andamento",
        _ => "Aberto",
    };
    json!({
        "key": key,
        "fields": {
            "summary": format!("Resumo de {key}"),
            "issuetype": { "name": type_name, "subtask": false },
            "status": { "name": status, "statusCategory": { "key": category } },
            "priority": { "name": "Média", "id": "3" },
            "assignee": null,
            "reporter": { "name": "autor", "displayName": "Autor da Issue" },
            "labels": [],
            "updated": "2026-10-28T10:15:30.000-0300",
            "customfield_10601": null,
            "customfield_10604": null,
            "subtasks": [],
            "parent": null,
        }
    })
}

/// A forma do `issue list --raw` do jira-cli 1.7.0: a estrutura própria
/// dele, sem a categoria do status nem os campos do épico.
fn reduce(v: &Value) -> Value {
    let f = &v["fields"];
    json!({
        "key": v["key"],
        "fields": {
            "summary": f["summary"],
            "labels": f["labels"],
            "issueType": { "id": "", "name": f["issuetype"]["name"],
                           "subtask": f["issuetype"]["subtask"] },
            "parent": f["parent"],
            "status": { "name": f["status"]["name"] },
            "Subtasks": f["subtasks"],
            "updated": f["updated"],
        }
    })
}

fn script(dir: &Path) -> String {
    format!(
        r#"#!/bin/sh
d='{dir}'
tok=ausente
if [ -n "${{JIRA_API_TOKEN+x}}" ]; then
  if [ "$JIRA_API_TOKEN" = "$(cat "$d/token")" ]; then tok=senha-ok; else tok=outra; fi
fi
# Uma escrita só por linha: as chamadas em paralelo não se misturam.
line=jira
tab=$(printf '\t')
for a in "$@"; do line="$line$tab$a"; done
line="$line${{tab}}token=$tok${{tab}}config=${{JIRA_CONFIG_FILE:-ausente}}"
printf '%s\n' "$line" >> "$d/jira-calls"
if [ -f "$d/sleep" ]; then sleep "$(cat "$d/sleep")"; fi
if [ "$1" = version ]; then
  echo '(Version="1.7.0", GitCommit="fake", Platform="test")'; exit 0
fi
if [ -f "$d/no-config" ]; then
  printf '\033[0;31m✗\033[0m Missing configuration file.\nRun '"'"'jira init'"'"' to configure the tool.\n' >&2
  exit 1
fi
if [ -f "$d/fail-401" ] || [ "$tok" != senha-ok ]; then
  printf '\njira: Received unexpected response '"'"'401 Unauthorized'"'"'.\nPlease check the parameters you supplied and try again.\n' >&2
  exit 1
fi
case "$1" in
  me)
    if [ -f "$d/fail-me" ]; then echo 'falha no me' >&2; exit 1; fi
    echo usuario; exit 0 ;;
  serverinfo)
    printf 'SERVER INFO\n-----------\n\nVersion: \t 8.5.2\nBuild Number: \t 805002\nDeployment Type: Server\n'
    exit 0 ;;
  issue)
    case "$2" in
      list)
        from=0; prev=; q=; rev=
        for a in "$@"; do
          if [ "$prev" = --paginate ]; then from="${{a%%:*}}"; fi
          if [ "$prev" = -q ]; then q="$a"; fi
          if [ "$a" = --reverse ]; then rev=1; fi
          prev="$a"
        done
        pd="$d/pages"
        if [ -z "$rev" ] && [ -d "$d/pages-desc" ]; then pd="$d/pages-desc"; fi
        if [ -f "$d/queries" ]; then
          while IFS="$tab" read -r name jq; do
            if [ "$jq" = "$q" ]; then pd="$d/$name"; fi
          done < "$d/queries"
        fi
        if [ -f "$pd/fail" ]; then
          echo "jira: Received unexpected response '500 Internal Server Error'." >&2
          exit 1
        fi
        fl="$pd/$((from / 100)).flaky"
        if [ -f "$fl" ]; then
          read -r left mode < "$fl"
          if [ "$left" -gt 0 ]; then
            echo "$((left - 1)) $mode" > "$fl"
            case "$mode" in
              sleep:*) sleep "${{mode#sleep:}}" ;;
              401)
                printf '\njira: Received unexpected response '"'"'401 Unauthorized'"'"'.\n' >&2
                exit 1 ;;
              *) echo "jira: Received unexpected response '503 Service Unavailable'." >&2
                exit 1 ;;
            esac
          fi
        fi
        f="$pd/$((from / 100)).json"
        if [ -f "$f" ]; then cat "$f"; exit 0; fi
        printf '\n\033[0;31m✗\033[0m No result found for given query in project\n' >&2
        exit 1 ;;
      view)
        if [ -f "$d/issues/$3.401" ]; then
          printf '\njira: Received unexpected response '"'"'401 Unauthorized'"'"'.\n' >&2
          exit 1
        fi
        f="$d/issues/$3.json"
        if [ -f "$f" ]; then cat "$f"; exit 0; fi
        echo "jira: Received unexpected response '404 Not Found'." >&2
        exit 1 ;;
    esac ;;
esac
echo "comando não suportado pelo jira falso: $*" >&2
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

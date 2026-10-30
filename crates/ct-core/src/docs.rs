//! Pasta de docs por projeto e por épico (épico #445, issue #440): o modelo
//! do projeto (vem do hub), os nomes das pastas e o `README.md` do épico.
//! Só regras puras; quem cria as pastas é o ambiente (`ct-sessions::docs`).
//!
//! ```text
//! <docs_root>/<owner>__<repo>/
//!   _projeto/<project_dirs>
//!   epicos/<n>-<slug>/
//!     README.md
//!     <epic_dirs>
//! ```

use crate::branch::slugify;
use crate::{item_ref, item_short, item_url, TrackerRef};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Pasta dos docs do projeto (fora dos épicos).
pub const PROJECT_DOCS_DIR: &str = "_projeto";
/// Pasta dos épicos.
pub const EPICS_DOCS_DIR: &str = "epicos";
/// Arquivo gerado uma vez na pasta do épico.
pub const EPIC_README: &str = "README.md";
/// Tamanho máximo do slug, o mesmo da branch ([`crate::branch::branch_from_pattern`]).
pub const DOCS_SLUG_LEN: usize = 40;
/// Quantas pastas cada modelo aceita.
pub const MAX_DOCS_DIRS: usize = 64;
/// Marcador na raiz de `docs_dir`/`docs_root` (#441): o `cthubd` o cria na
/// pasta real, e o ambiente só considera `docs_root` acessível com ele. Sem
/// o marcador, o ponto de montagem de um compartilhamento caído (pasta local
/// vazia) não é confundido com a pasta de docs.
pub const DOCS_ROOT_MARKER: &str = ".ct-docs-root";
/// Conteúdo do [`DOCS_ROOT_MARKER`] criado pelo `cthubd`.
pub const DOCS_ROOT_MARKER_TEXT: &str = "Raiz da pasta de docs dos projetos do Control Tower \
(docs_dir do cthubd). Os ambientes só usam esta pasta (docs_root) se este arquivo existir: \
sem ele, um ponto de montagem vazio (compartilhamento caído) seria tomado pela pasta real. \
Não apague.\n";

/// Docs do projeto no hub (`HubProject.docs`). Ausente num hub anterior à
/// #440: desligado, sem pastas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DocsConfig {
    /// Desligado, o ambiente não cria nada para o projeto.
    pub enabled: bool,
    /// Pastas criadas em `_projeto/` (caminhos relativos, ex.: `analises`).
    pub project_dirs: Vec<String>,
    /// Pastas criadas em cada `epicos/<n>-<slug>/`.
    pub epic_dirs: Vec<String>,
}

impl DocsConfig {
    /// JSON da coluna `docs_config`.
    pub fn config_json(&self) -> String {
        serde_json::to_string(self).expect("DocsConfig serializa")
    }

    /// Aparado, sem barras nas pontas, sem vazios nem repetidos (na ordem).
    pub fn normalized(&self) -> DocsConfig {
        fn clean(v: &[String]) -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for d in v {
                let d = d.trim().trim_matches('/').trim().to_string();
                if !d.is_empty() && !out.contains(&d) {
                    out.push(d);
                }
            }
            out
        }
        DocsConfig {
            enabled: self.enabled,
            project_dirs: clean(&self.project_dirs),
            epic_dirs: clean(&self.epic_dirs),
        }
    }

    /// Confere o modelo (já normalizado): cada pasta é um caminho relativo
    /// sem `..` nem `.`, sem componente oculto, e o épico não tem pasta
    /// chamada `README.md`.
    pub fn validate(&self) -> Result<(), String> {
        for (label, dirs) in [
            ("project_dirs", &self.project_dirs),
            ("epic_dirs", &self.epic_dirs),
        ] {
            if dirs.len() > MAX_DOCS_DIRS {
                return Err(format!(
                    "docs.{label}: no máximo {MAX_DOCS_DIRS} pastas ({} informadas)",
                    dirs.len()
                ));
            }
            for d in dirs {
                valid_docs_dir(d).map_err(|m| format!("docs.{label}: {m}"))?;
            }
        }
        if self.epic_dirs.iter().any(|d| d == EPIC_README) {
            return Err(format!(
                "docs.epic_dirs: {EPIC_README} é o arquivo do épico"
            ));
        }
        Ok(())
    }
}

/// Uma pasta do modelo: caminho relativo, só componentes normais e visíveis.
fn valid_docs_dir(d: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("pasta inválida {d:?}: {why}"));
    if d.is_empty() {
        return bad("vazia");
    }
    if d.contains('\\') || d.chars().any(|c| c.is_control()) {
        return bad("use / como separador, sem caracteres de controle");
    }
    if Path::new(d).is_absolute() {
        return bad("use um caminho relativo");
    }
    // Por `/`, não por `Path::components`, que esconde o `.` do meio.
    for c in d.split('/') {
        match c {
            "" | "." | ".." => return bad("sem .., . nem // no caminho"),
            c if c.starts_with('.') => return bad("sem pasta oculta (começando por .)"),
            _ => {}
        }
    }
    Ok(())
}

/// Pasta do projeto em `docs_root`: `<owner>__<repo>`. No GitLab, as barras
/// do grupo também viram `__` (`grupo__projeto__api`).
pub fn project_docs_dir_name(owner: &str, repo: &str) -> String {
    format!("{}__{}", owner.replace('/', "__"), repo)
}

/// Pasta nova do épico: `<n>-<slug>`, com o slug da branch do épico
/// (`slugify(título, 40)`); título sem slug = `<n>`.
pub fn epic_docs_dir_name(number: i64, title: &str) -> String {
    match slugify(title, DOCS_SLUG_LEN) {
        s if s.is_empty() => number.to_string(),
        s => format!("{number}-{s}"),
    }
}

/// A pasta `name` é a do épico `number` (`<n>` ou `<n>-*`): o título pode
/// ter mudado depois que ela nasceu.
pub fn is_epic_docs_dir(name: &str, number: i64) -> bool {
    let n = number.to_string();
    name == n || name.strip_prefix(&n).is_some_and(|r| r.starts_with('-'))
}

/// Sub-issue listada no `README.md` do épico.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicSubIssue {
    pub number: i64,
    pub title: String,
}

/// O que o `README.md` do épico mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicDocs {
    pub number: i64,
    /// Título do épico; vazio = desconhecido (épico fora do espelho).
    pub title: String,
    /// Link da issue; vazio = o link padrão do rastreador.
    pub url: String,
    pub sub_issues: Vec<EpicSubIssue>,
}

/// `README.md` do épico, gerado uma vez: título, link da issue e
/// sub-issues (as do espelho no momento da criação).
pub fn epic_readme(t: &TrackerRef, epic: &EpicDocs) -> String {
    let reference = item_ref(t, epic.number);
    let url = match epic.url.trim() {
        "" => item_url(t, epic.number),
        u => u.to_string(),
    };
    let title = match epic.title.trim() {
        "" => format!("Épico {reference}"),
        s => s.to_string(),
    };
    let mut out = format!("# {title}\n\nIssue: [{reference}]({url})\n\n## Sub-issues\n\n");
    if epic.sub_issues.is_empty() {
        out.push_str("Nenhuma sub-issue no espelho quando esta pasta foi criada.\n");
    }
    for s in &epic.sub_issues {
        out.push_str(&format!(
            "- [{}]({}) {}\n",
            item_short(t, s.number),
            item_url(t, s.number),
            s.title.trim()
        ));
    }
    out.push_str(
        "\n---\nGerado pelo Control Tower na criação da pasta; não é atualizado depois.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrackerConfig;

    fn dirs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ausente_e_desligado_e_parcial_completa_com_padrao() {
        let d: DocsConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(d, DocsConfig::default());
        assert!(!d.enabled);
        let d: DocsConfig = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(d.enabled && d.project_dirs.is_empty());
    }

    #[test]
    fn normaliza_e_valida_as_pastas() {
        let d = DocsConfig {
            enabled: true,
            project_dirs: dirs(&[" analises/ ", "", "analises", "specs/telas"]),
            epic_dirs: dirs(&["prompts", "evidencias"]),
        }
        .normalized();
        assert_eq!(d.project_dirs, dirs(&["analises", "specs/telas"]));
        assert!(d.validate().is_ok());
        for bad in ["../fora", "/abs", "a/../b", ".oculta", "a/./b", "a\\b"] {
            let d = DocsConfig {
                project_dirs: dirs(&[bad]),
                ..Default::default()
            };
            assert!(d.validate().is_err(), "{bad}");
        }
        let d = DocsConfig {
            epic_dirs: dirs(&["README.md"]),
            ..Default::default()
        };
        assert!(d.validate().is_err());
    }

    #[test]
    fn nomes_das_pastas() {
        assert_eq!(
            project_docs_dir_name("deep-ion-ai", "ctower-agents"),
            "deep-ion-ai__ctower-agents"
        );
        assert_eq!(
            project_docs_dir_name("grupo/projeto", "api"),
            "grupo__projeto__api"
        );
        // O mesmo slug da branch do épico (`wip/445-epico-docs-...`).
        assert_eq!(
            epic_docs_dir_name(
                445,
                "Épico: docs dos projetos fora do repositório — pasta no hub"
            ),
            "445-epico-docs-dos-projetos-fora-do-reposito"
        );
        assert_eq!(epic_docs_dir_name(7, "!!!"), "7");
        assert!(is_epic_docs_dir("445-outro-titulo", 445));
        assert!(is_epic_docs_dir("445", 445));
        assert!(!is_epic_docs_dir("4450-x", 445));
        assert!(!is_epic_docs_dir("44-x", 445));
    }

    #[test]
    fn readme_do_epico() {
        let t = TrackerRef::new("o", "r", &TrackerConfig::Github);
        let r = epic_readme(
            &t,
            &EpicDocs {
                number: 9,
                title: "Docs".into(),
                url: String::new(),
                sub_issues: vec![EpicSubIssue {
                    number: 10,
                    title: "Pastas".into(),
                }],
            },
        );
        assert!(r.starts_with("# Docs\n"), "{r}");
        assert!(
            r.contains("[o/r#9](https://github.com/o/r/issues/9)"),
            "{r}"
        );
        assert!(
            r.contains("- [#10](https://github.com/o/r/issues/10) Pastas"),
            "{r}"
        );
    }
}

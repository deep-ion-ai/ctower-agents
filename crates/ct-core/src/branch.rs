//! Convenções de nomes de branch e diretório de worktree.

use crate::{TrackerKind, WorkItemKind};

/// Remove acentos dos caracteres latinos mais comuns; os demais passam inalterados.
fn fold(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

/// Slug ASCII (`[a-z0-9-]`), sem hífens repetidos nem nas pontas, com no máximo `max_len` bytes.
pub fn slugify(title: &str, max_len: usize) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in title.chars().flat_map(char::to_lowercase).map(fold) {
        if ('\u{300}'..='\u{36f}').contains(&c) {
            // Acento combinante (texto em NFD): descarta sem separar a palavra.
            continue;
        }
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    out.truncate(max_len.min(out.len()));
    out.trim_end_matches('-').to_string()
}

/// Padrão de branch do GitHub, igual ao de sempre: `feat/12-slug`.
pub const GITHUB_BRANCH_PATTERN: &str = "{prefix}/{n}-{slug}";
/// Padrão de branch do Jira: `feat/PROJ-5450-slug`.
pub const JIRA_BRANCH_PATTERN: &str = "{prefix}/{key}-{slug}";

/// Padrão usado quando o projeto não define `branch_pattern`.
pub fn default_branch_pattern(tracker: TrackerKind) -> &'static str {
    match tracker {
        TrackerKind::Github => GITHUB_BRANCH_PATTERN,
        TrackerKind::Jira => JIRA_BRANCH_PATTERN,
    }
}

/// Padrão de branch inválido, ou nome gerado que o git recusaria.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BranchPatternError {
    #[error("marcador desconhecido no padrão de branch: {{{0}}} (use {{prefix}}, {{n}}, {{key}} ou {{slug}})")]
    UnknownMarker(String),
    #[error("chave sem par no padrão de branch: {0}")]
    Unbalanced(String),
    #[error("o padrão de branch precisa de {{n}} ou {{key}}: {0}")]
    NoNumber(String),
    #[error("nome de branch inválido para o git: {0}")]
    InvalidRef(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    Prefix,
    N,
    Key,
    Slug,
}

enum Piece<'a> {
    Text(&'a str),
    Marker(Marker),
}

fn parse_pattern(pattern: &str) -> Result<Vec<Piece<'_>>, BranchPatternError> {
    let mut out = Vec::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        let open = rest.find('{');
        let close = rest.find('}');
        match (open, close) {
            (None, None) => {
                out.push(Piece::Text(rest));
                break;
            }
            (Some(o), Some(c)) if o < c && !rest[o + 1..c].contains('{') => {
                if o > 0 {
                    out.push(Piece::Text(&rest[..o]));
                }
                let marker = match &rest[o + 1..c] {
                    "prefix" => Marker::Prefix,
                    "n" => Marker::N,
                    "key" => Marker::Key,
                    "slug" => Marker::Slug,
                    other => return Err(BranchPatternError::UnknownMarker(other.to_string())),
                };
                out.push(Piece::Marker(marker));
                rest = &rest[c + 1..];
            }
            _ => return Err(BranchPatternError::Unbalanced(pattern.to_string())),
        }
    }
    Ok(out)
}

/// `{prefix}` de um work item: `wip` (épico), `fix` (label `bug`), `chore`
/// (label `chore`) ou `feat`.
fn prefix_for(kind: WorkItemKind, labels: &[String]) -> &'static str {
    let has = |name: &str| labels.iter().any(|l| l.eq_ignore_ascii_case(name));
    match kind {
        WorkItemKind::Epic => "wip",
        WorkItemKind::Issue if has("bug") => "fix",
        WorkItemKind::Issue if has("chore") => "chore",
        WorkItemKind::Issue => "feat",
    }
}

/// Branch de trabalho de um work item pelo padrão do projeto (spec do Jira e
/// GitLab §4). Marcadores: `{prefix}`, `{n}` (o número), `{key}` (o
/// [`crate::item_key`]: `12` no GitHub, `PROJ-5450` no Jira) e `{slug}`
/// (`slugify(title, 40)`). Com slug vazio, o separador antes de `{slug}`
/// cai (`feat/5`).
pub fn branch_from_pattern(
    pattern: &str,
    kind: WorkItemKind,
    number: i64,
    key: &str,
    title: &str,
    labels: &[String],
) -> Result<String, BranchPatternError> {
    let pieces = parse_pattern(pattern)?;
    if !pieces
        .iter()
        .any(|p| matches!(p, Piece::Marker(Marker::N | Marker::Key)))
    {
        return Err(BranchPatternError::NoNumber(pattern.to_string()));
    }
    let slug = slugify(title, 40);
    let mut out = String::new();
    for p in &pieces {
        match p {
            Piece::Text(t) => out.push_str(t),
            Piece::Marker(Marker::Prefix) => out.push_str(prefix_for(kind, labels)),
            Piece::Marker(Marker::N) => out.push_str(&number.to_string()),
            Piece::Marker(Marker::Key) => out.push_str(key),
            Piece::Marker(Marker::Slug) if slug.is_empty() => {
                if out.ends_with(['-', '_', '.', '/']) {
                    out.pop();
                }
            }
            Piece::Marker(Marker::Slug) => out.push_str(&slug),
        }
    }
    if !valid_branch_name(&out) {
        return Err(BranchPatternError::InvalidRef(out));
    }
    Ok(out)
}

/// Confere um padrão de branch no cadastro: só marcadores conhecidos, com
/// `{n}` ou `{key}`, e um nome de exemplo aceito pelo git.
pub fn validate_branch_pattern(pattern: &str) -> Result<(), BranchPatternError> {
    for (kind, labels) in [
        (WorkItemKind::Epic, vec![]),
        (WorkItemKind::Issue, vec!["bug".to_string()]),
    ] {
        branch_from_pattern(pattern, kind, 123, "PROJ-123", "Título de exemplo", &labels)?;
    }
    Ok(())
}

/// Nome aceito por `git check-ref-format --branch` (as regras de
/// `git-check-ref-format(1)`, sem rodar o git).
pub fn valid_branch_name(name: &str) -> bool {
    let bad_char = |c: char| c.is_ascii_control() || " ~^:?*[\\".contains(c);
    !name.is_empty()
        && name != "@"
        && !name.starts_with('-')
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("@{")
        && !name.chars().any(bad_char)
        && name
            .split('/')
            .all(|c| !c.starts_with('.') && !c.ends_with(".lock"))
}

/// Branch de trabalho de um work item do GitHub (spec §7.1): o padrão
/// [`GITHUB_BRANCH_PATTERN`].
pub fn branch_for(kind: WorkItemKind, number: u64, title: &str, labels: &[String]) -> String {
    let n = number as i64;
    branch_from_pattern(
        GITHUB_BRANCH_PATTERN,
        kind,
        n,
        &n.to_string(),
        title,
        labels,
    )
    .expect("o padrão do GitHub gera nome válido")
}

/// Número da issue numa branch do GitHub no padrão `<tipo>/<n>-<slug>` (ou
/// `<tipo>/<n>`), como o PR de uma sub-issue para a branch do épico (#454).
/// `None` se a branch não segue o padrão (`fix/teste-instavel`, `feat/12x`).
pub fn issue_number_from_branch(branch: &str) -> Option<i64> {
    let (prefix, rest) = branch.split_once('/')?;
    if prefix.is_empty() || rest.contains('/') {
        return None;
    }
    let (n, slug) = match rest.split_once('-') {
        Some((n, slug)) => (n, Some(slug)),
        None => (rest, None),
    };
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) || slug == Some("") {
        return None;
    }
    n.parse().ok().filter(|n| *n > 0)
}

/// Nome de diretório (um único componente de caminho) para o worktree de uma branch.
pub fn worktree_dir_name(branch: &str) -> String {
    branch.replace('/', "__")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkItemKind;

    fn labels(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn numero_da_issue_pela_branch() {
        assert_eq!(
            issue_number_from_branch("feat/424-ui-acoes-do-console"),
            Some(424)
        );
        assert_eq!(issue_number_from_branch("fix/5"), Some(5));
        assert_eq!(issue_number_from_branch("wip/429-console"), Some(429));
        for b in [
            "fix/teste-cleanup-auto-instavel",
            "feat/12x-slug",
            "feat/-1-x",
            "feat/0-x",
            "feat/12-",
            "424-sem-tipo",
            "/424-x",
            "feat/a/424-x",
            "develop",
        ] {
            assert_eq!(issue_number_from_branch(b), None, "{b}");
        }
    }

    #[test]
    fn slug_remove_acentos_e_pontuacao() {
        assert_eq!(
            slugify("Épico: Autenticação OAuth 2.0!", 60),
            "epico-autenticacao-oauth-2-0"
        );
    }

    #[test]
    fn slug_lida_com_emoji_e_espacos() {
        assert_eq!(slugify("  🚀 Lançar   versão  ", 60), "lancar-versao");
        assert_eq!(slugify("  --  ", 60), "");
    }

    #[test]
    fn slug_trunca_sem_hifen_no_fim() {
        let s = slugify("aaaa bbbb cccc", 6);
        assert_eq!(s, "aaaa-b");
        let s = slugify("aaaa bbbb", 5);
        assert_eq!(s, "aaaa");
    }

    #[test]
    fn slug_so_tem_caracteres_seguros() {
        let s = slugify("Ação/çõ \"aspas\" <tag> ~^:?*[]\\ ünïcödé ß", 80);
        assert!(
            s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{s}"
        );
        assert!(!s.starts_with('-') && !s.ends_with('-'));
        assert!(!s.contains("--"));
    }

    #[test]
    fn branch_de_epico_usa_wip() {
        assert_eq!(
            branch_for(WorkItemKind::Epic, 12, "Autenticação", &[]),
            "wip/12-autenticacao"
        );
    }

    #[test]
    fn branch_de_issue_por_label() {
        assert_eq!(
            branch_for(WorkItemKind::Issue, 7, "Corrige login", &labels(&["Bug"])),
            "fix/7-corrige-login"
        );
        assert_eq!(
            branch_for(WorkItemKind::Issue, 8, "Limpa deps", &labels(&["chore"])),
            "chore/8-limpa-deps"
        );
        assert_eq!(
            branch_for(
                WorkItemKind::Issue,
                9,
                "Export CSV",
                &labels(&["enhancement"])
            ),
            "feat/9-export-csv"
        );
    }

    #[test]
    fn branch_sem_slug_usa_so_numero() {
        assert_eq!(branch_for(WorkItemKind::Issue, 5, "🚀🚀", &[]), "feat/5");
    }

    #[test]
    fn padrao_do_jira() {
        let b = |p: &str, kind, labels: &[&str]| {
            branch_from_pattern(
                p,
                kind,
                5450,
                "PROJ-5450",
                "Portar o plugin",
                &labels.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            )
            .unwrap()
        };
        assert_eq!(
            b(JIRA_BRANCH_PATTERN, WorkItemKind::Issue, &[]),
            "feat/PROJ-5450-portar-o-plugin"
        );
        assert_eq!(
            b(JIRA_BRANCH_PATTERN, WorkItemKind::Epic, &[]),
            "wip/PROJ-5450-portar-o-plugin"
        );
        assert_eq!(b("{key}", WorkItemKind::Issue, &["bug"]), "PROJ-5450");
        assert_eq!(
            b("{prefix}/{n}_{slug}", WorkItemKind::Issue, &["chore"]),
            "chore/5450_portar-o-plugin"
        );
        assert_eq!(
            default_branch_pattern(TrackerKind::Github),
            GITHUB_BRANCH_PATTERN
        );
        assert_eq!(
            default_branch_pattern(TrackerKind::Jira),
            JIRA_BRANCH_PATTERN
        );
    }

    #[test]
    fn padrao_do_github_e_branch_for_iguais() {
        for (kind, n, title, l) in [
            (WorkItemKind::Epic, 12, "Autenticação", vec![]),
            (WorkItemKind::Issue, 7, "Corrige login", labels(&["Bug"])),
            (WorkItemKind::Issue, 5, "🚀🚀", vec![]),
        ] {
            assert_eq!(
                branch_from_pattern(GITHUB_BRANCH_PATTERN, kind, n, &n.to_string(), title, &l)
                    .unwrap(),
                branch_for(kind, n as u64, title, &l)
            );
        }
    }

    #[test]
    fn slug_vazio_derruba_o_separador() {
        assert_eq!(
            branch_from_pattern(JIRA_BRANCH_PATTERN, WorkItemKind::Issue, 5, "P-5", "", &[])
                .unwrap(),
            "feat/P-5"
        );
        assert_eq!(
            branch_from_pattern("{key}/{slug}", WorkItemKind::Issue, 5, "P-5", "🚀", &[]).unwrap(),
            "P-5"
        );
    }

    #[test]
    fn padroes_invalidos() {
        use BranchPatternError::*;
        assert!(validate_branch_pattern(GITHUB_BRANCH_PATTERN).is_ok());
        assert!(validate_branch_pattern(JIRA_BRANCH_PATTERN).is_ok());
        assert!(validate_branch_pattern("{key}").is_ok());
        assert_eq!(
            validate_branch_pattern("{prefix}/{numero}"),
            Err(UnknownMarker("numero".into()))
        );
        assert!(matches!(
            validate_branch_pattern("{prefix}/{slug}"),
            Err(NoNumber(_))
        ));
        assert!(matches!(
            validate_branch_pattern("{prefix/{n}"),
            Err(Unbalanced(_))
        ));
        assert!(matches!(
            validate_branch_pattern("{n}}"),
            Err(Unbalanced(_))
        ));
        for p in [
            "{n} x", "-{n}", "{n}..x", "{n}/", "{n}.lock", "a:{n}", "{n}~1", ".{n}",
        ] {
            assert!(
                matches!(validate_branch_pattern(p), Err(InvalidRef(_))),
                "{p}"
            );
        }
    }

    #[test]
    fn nomes_validos_para_o_git() {
        for n in ["feat/12-x", "PROJ-5450", "wip/PROJ-1-a.b"] {
            assert!(valid_branch_name(n), "{n}");
        }
        for n in ["", "@", "a//b", "a/.b", "a@{b", "a\\b", "a\tb", "/a", "a."] {
            assert!(!valid_branch_name(n), "{n:?}");
        }
    }

    #[test]
    fn diretorio_de_worktree_nao_tem_barra() {
        assert_eq!(worktree_dir_name("wip/12-auth"), "wip__12-auth");
    }

    #[test]
    fn slug_ignora_acentos_combinantes_nfd() {
        assert_eq!(slugify("Autenticac\u{0327}a\u{0303}o", 40), "autenticacao");
    }
}

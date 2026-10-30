//! Descrição da issue do Jira → Markdown (GFM), para o "Ver issue" (#434).
//! Só em memória: o hub converte o que o ambiente de sync leu e devolve à
//! UI, sem gravar.
//!
//! - [`description_markdown`]: a `fields.description` do `issue view --raw`,
//!   que é texto em wiki markup no Jira Server/Data Center (API v2) e um
//!   documento ADF no Jira Cloud (API v3);
//! - [`wiki_to_markdown`]: o subconjunto do wiki markup que aparece em
//!   descrição (títulos, listas, ênfase, código, citações, tabelas, links e
//!   imagens). O resto passa como texto;
//! - [`adf_to_markdown`]: os nós do ADF que aparecem em descrição; nó
//!   desconhecido vira o texto dos filhos.
//!
//! Texto do Jira não tem HTML: `<` vira `&lt;` fora do código, para o
//! Markdown não o interpretar (a UI ainda sanitiza o HTML gerado).

use serde_json::Value;

/// `fields.description` → Markdown. Ausente, nula ou vazia = `""`.
pub fn description_markdown(desc: Option<&Value>) -> String {
    match desc {
        Some(Value::String(s)) => wiki_to_markdown(s),
        Some(v @ Value::Object(_)) => adf_to_markdown(v),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------- wiki markup

/// Wiki markup do Jira → Markdown.
pub fn wiki_to_markdown(src: &str) -> String {
    let src = src.replace("\r\n", "\n").replace('\r', "\n");
    let mut out: Vec<String> = Vec::new();
    // Bloco de código aberto: a tag que o fecha (`{code}` ou `{noformat}`).
    let mut code: Option<&str> = None;
    let mut quote = false;
    // Dentro de uma tabela (a linha sem `||` do começo ganha cabeçalho vazio).
    let mut table = false;
    for line in src.split('\n') {
        if let Some(close) = code {
            match line.find(close) {
                Some(i) => {
                    if !line[..i].is_empty() {
                        out.push(line[..i].to_string());
                    }
                    out.push("```".into());
                    code = None;
                    let rest = line[i + close.len()..].trim();
                    if !rest.is_empty() {
                        out.push(inline(rest));
                    }
                }
                None => out.push(line.to_string()),
            }
            continue;
        }
        let t = line.trim();
        if let Some((close, lang, rest)) = code_open(t) {
            table = false;
            blank_before(&mut out);
            out.push(format!("```{lang}"));
            match rest.find(close) {
                // `{code}x{code}` na mesma linha.
                Some(i) => {
                    if !rest[..i].is_empty() {
                        out.push(rest[..i].to_string());
                    }
                    out.push("```".into());
                }
                None => {
                    if !rest.is_empty() {
                        out.push(rest.to_string());
                    }
                    code = Some(close);
                }
            }
            continue;
        }
        if t == "{quote}" {
            quote = !quote;
            if quote {
                blank_before(&mut out);
            } else {
                out.push(String::new());
            }
            continue;
        }
        // Painel: só o conteúdo (a linha que é só a tag some).
        let only_tag = t.ends_with('}') && t.matches('}').count() == 1;
        if only_tag && (t.starts_with("{panel") || t.starts_with("{color")) {
            continue;
        }
        let md = if t.starts_with('|') {
            let (row, header) = table_row(t);
            let n = row.len();
            let mut s = String::new();
            if !table {
                blank_before(&mut out);
                if !header {
                    s.push_str(&format!("|{}\n", " |".repeat(n)));
                    s.push_str(&format!("|{}\n", " --- |".repeat(n)));
                }
            }
            s.push_str(&format!("| {} |", row.join(" | ")));
            if header {
                s.push_str(&format!("\n|{}", " --- |".repeat(n)));
            }
            table = true;
            s
        } else {
            table = false;
            block_line(t)
        };
        out.push(if quote { quoted(&md) } else { md });
    }
    if code.is_some() {
        out.push("```".into());
    }
    out.join("\n").trim().to_string()
}

/// Linha em branco antes de um bloco (tabela, código, citação), para o
/// Markdown não o grudar no parágrafo anterior.
fn blank_before(out: &mut Vec<String>) {
    if out.last().is_some_and(|l| !l.is_empty()) {
        out.push(String::new());
    }
}

fn quoted(md: &str) -> String {
    md.split('\n')
        .map(|l| format!("> {l}").trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `{code}`, `{code:java}`, `{code:title=X|lang=sql}`, `{noformat}` no
/// começo da linha → (tag que fecha, linguagem, resto da linha).
fn code_open(t: &str) -> Option<(&'static str, String, &str)> {
    let (close, body) = match t.strip_prefix("{code") {
        Some(b) => ("{code}", b),
        None => ("{noformat}", t.strip_prefix("{noformat")?),
    };
    let end = body.find('}')?;
    let params = body[..end].strip_prefix(':').unwrap_or(&body[..end]);
    if !body[..end].is_empty() && !body[..end].starts_with(':') {
        return None; // `{codex}` não é tag.
    }
    let lang = params
        .split('|')
        .find_map(|p| match p.split_once('=') {
            Some(("lang" | "language", v)) => Some(v.trim()),
            Some(_) => None,
            None => Some(p.trim()),
        })
        .filter(|l| {
            l.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '#')
        })
        .unwrap_or("");
    let lang = if close == "{noformat}" { "" } else { lang };
    Some((close, lang.to_string(), &body[end + 1..]))
}

/// Uma linha fora de código e de tabela: título, lista, citação, régua ou
/// parágrafo.
fn block_line(t: &str) -> String {
    if t == "----" {
        return "---".into();
    }
    if let Some(rest) = t.strip_prefix("bq. ") {
        return format!("> {}", inline(rest));
    }
    let b = t.as_bytes();
    if b.len() >= 4 && b[0] == b'h' && (b'1'..=b'6').contains(&b[1]) && &t[2..4] == ". " {
        let level = (b[1] - b'0') as usize;
        return format!("{} {}", "#".repeat(level), inline(&t[4..]));
    }
    // Lista: `*`, `#` e `-` (um só) repetidos, depois espaço.
    let marks: String = t
        .chars()
        .take_while(|c| matches!(c, '*' | '#' | '-'))
        .collect();
    if !marks.is_empty()
        && t[marks.len()..].starts_with(' ')
        && (marks.chars().all(|c| c == '*' || c == '#') || marks == "-")
    {
        let mut indent = String::new();
        for c in marks.chars().take(marks.chars().count() - 1) {
            indent.push_str(if c == '#' { "   " } else { "  " });
        }
        let bullet = if marks.ends_with('#') { "1." } else { "-" };
        return format!("{indent}{bullet} {}", inline(t[marks.len()..].trim_start()));
    }
    inline(t)
}

/// `||a||b||` (cabeçalho) ou `|a|b|` → as células (já convertidas) e se é
/// cabeçalho. `|` dentro de `[link|url]` e `{{...}}` não separa célula.
fn table_row(t: &str) -> (Vec<String>, bool) {
    let header = t.starts_with("||");
    let mut cells = Vec::new();
    let mut cur = String::new();
    let (mut brackets, mut mono) = (0usize, false);
    let chars: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' && chars.get(i + 1) == Some(&'{') {
            mono = true;
        } else if c == '}' && chars.get(i + 1) == Some(&'}') {
            mono = false;
        } else if c == '[' {
            brackets += 1;
        } else if c == ']' {
            brackets = brackets.saturating_sub(1);
        }
        if c == '|' && brackets == 0 && !mono {
            cells.push(std::mem::take(&mut cur));
            if chars.get(i + 1) == Some(&'|') {
                i += 1;
            }
        } else {
            cur.push(c);
        }
        i += 1;
    }
    cells.push(cur);
    // O primeiro separador abre a linha e o último a fecha.
    if cells.first().is_some_and(|c| c.trim().is_empty()) {
        cells.remove(0);
    }
    if cells.len() > 1 && cells.last().is_some_and(|c| c.trim().is_empty()) {
        cells.pop();
    }
    let cells = cells
        .iter()
        .map(|c| inline(c.trim()).replace('|', "\\|"))
        .collect();
    (cells, header)
}

/// Ênfase, código, links, imagens e menções dentro de uma linha.
fn inline(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        // `{{mono}}`
        if c[i] == '{' && c.get(i + 1) == Some(&'{') {
            if let Some(end) = find(&c, i + 2, &['}', '}']) {
                let inner: String = c[i + 2..end].iter().collect();
                out.push_str(&code_span(&inner));
                i = end + 2;
                continue;
            }
        }
        // `{color:red}`, `{color}` e afins dentro da linha: somem.
        if c[i] == '{' {
            if let Some(end) = find(&c, i + 1, &['}']) {
                let tag: String = c[i + 1..end].iter().collect();
                if tag == "color" || tag.starts_with("color:") {
                    i = end + 1;
                    continue;
                }
            }
        }
        // `\\` = quebra de linha.
        if c[i] == '\\' && c.get(i + 1) == Some(&'\\') {
            out.push_str("<br>");
            i += 2;
            continue;
        }
        if c[i] == '[' {
            if let Some(end) = find(&c, i + 1, &[']']) {
                let inner: String = c[i + 1..end].iter().collect();
                if let Some(md) = link(&inner) {
                    out.push_str(&md);
                    i = end + 1;
                    continue;
                }
            }
        }
        if c[i] == '!' {
            if let Some(end) = find(&c, i + 1, &['!']) {
                let inner: String = c[i + 1..end].iter().collect();
                if let Some(md) = image(&inner) {
                    out.push_str(&md);
                    i = end + 1;
                    continue;
                }
            }
        }
        if let Some(md) = ['*', '_', '-', '+', '^', '~']
            .iter()
            .find(|m| c[i] == **m)
            .and_then(|m| emphasis(&c, i, *m))
        {
            out.push_str(&md.0);
            i = md.1;
            continue;
        }
        match c[i] {
            '<' => out.push_str("&lt;"),
            ch => out.push(ch),
        }
        i += 1;
    }
    out
}

/// Posição da próxima ocorrência de `pat` a partir de `from`.
fn find(c: &[char], from: usize, pat: &[char]) -> Option<usize> {
    (from..c.len().saturating_sub(pat.len() - 1)).find(|&j| c[j..j + pat.len()] == *pat)
}

fn code_span(inner: &str) -> String {
    let fence = if inner.contains('`') { "``" } else { "`" };
    format!("{fence}{inner}{fence}")
}

fn is_boundary(c: Option<&char>) -> bool {
    c.is_none_or(|c| !c.is_alphanumeric())
}

/// `*negrito*`, `_itálico_`, `-riscado-`, `+sublinhado+`, `^sobrescrito^` e
/// `~subscrito~`: o marcador abre depois de fronteira e antes de não-espaço,
/// e fecha depois de não-espaço e antes de fronteira, na mesma linha.
fn emphasis(c: &[char], i: usize, m: char) -> Option<(String, usize)> {
    if !is_boundary(i.checked_sub(1).map(|p| &c[p])) {
        return None;
    }
    let first = c.get(i + 1)?;
    if first.is_whitespace() || *first == m {
        return None;
    }
    let end = (i + 2..c.len())
        .find(|&j| c[j] == m && !c[j - 1].is_whitespace() && is_boundary(c.get(j + 1)))?;
    let inner = inline(&c[i + 1..end].iter().collect::<String>());
    let md = match m {
        '*' => format!("**{inner}**"),
        '_' => format!("*{inner}*"),
        '-' => format!("~~{inner}~~"),
        '+' => format!("<ins>{inner}</ins>"),
        '^' => format!("<sup>{inner}</sup>"),
        _ => format!("<sub>{inner}</sub>"),
    };
    Some((md, end + 1))
}

/// `[url]`, `[texto|url]`, `[~usuario]`, `[mailto:x]`; `None` = colchete
/// comum (fica como texto).
fn link(inner: &str) -> Option<String> {
    if let Some(user) = inner.strip_prefix('~') {
        return Some(format!("@{}", user.trim_start_matches("accountid:")));
    }
    let (text, url) = match inner.split_once('|') {
        Some((t, u)) => (Some(t.trim()), u.trim()),
        None => (None, inner.trim()),
    };
    // A dica do link (`[texto|url|dica]`) fica de fora.
    let url = url.split('|').next().unwrap_or(url);
    let ok = ["http://", "https://", "mailto:", "ftp://"]
        .iter()
        .any(|p| url.starts_with(p));
    if !ok || url.contains(char::is_whitespace) {
        return None;
    }
    Some(match text {
        Some(t) if !t.is_empty() => format!("[{}]({url})", inline(t)),
        _ => format!("<{url}>"),
    })
}

/// `!imagem.png!` e `!url|width=300!`. Anexo sem URL não tem como ser
/// mostrado fora do Jira: vira o nome entre colchetes.
fn image(inner: &str) -> Option<String> {
    let src = inner.split('|').next()?.trim();
    if src.is_empty() || src.contains(char::is_whitespace) {
        return None;
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        Some(format!("![]({src})"))
    } else if src.contains('.') {
        Some(format!("[anexo: {src}]"))
    } else {
        None
    }
}

// ------------------------------------------------------------------------ ADF

/// Documento ADF (Jira Cloud) → Markdown.
pub fn adf_to_markdown(doc: &Value) -> String {
    blocks(children(doc), "").trim().to_string()
}

fn children(v: &Value) -> &[Value] {
    v.get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn attr<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.get("attrs").and_then(|a| a.get(k))
}

/// Blocos separados por linha em branco; `prefix` vai em cada linha (a
/// indentação da lista ou o `> ` da citação).
fn blocks(nodes: &[Value], prefix: &str) -> String {
    nodes
        .iter()
        .map(|n| block(n, prefix))
        .filter(|b| !b.is_empty())
        .collect::<Vec<_>>()
        .join(&format!("\n{}\n", prefix.trim_end()))
}

fn prefixed(text: &str, prefix: &str) -> String {
    text.split('\n')
        .map(|l| format!("{prefix}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn block(n: &Value, prefix: &str) -> String {
    let kind = n.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "paragraph" => prefixed(&inlines(children(n)), prefix),
        "heading" => {
            let level = attr(n, "level")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6);
            format!(
                "{prefix}{} {}",
                "#".repeat(level as usize),
                inlines(children(n))
            )
        }
        "bulletList" | "orderedList" | "taskList" => list(n, kind, prefix),
        "codeBlock" => {
            let lang = attr(n, "language").and_then(Value::as_str).unwrap_or("");
            let text: String = children(n)
                .iter()
                .filter_map(|t| t.get("text").and_then(Value::as_str))
                .collect();
            prefixed(&format!("```{lang}\n{text}\n```"), prefix)
        }
        "blockquote" | "panel" => blocks(children(n), &format!("{prefix}> ")),
        "rule" => format!("{prefix}---"),
        "table" => prefixed(&table(n), prefix),
        "mediaSingle" | "mediaGroup" | "media" => format!("{prefix}[anexo]"),
        "expand" | "nestedExpand" => {
            let title = attr(n, "title").and_then(Value::as_str).unwrap_or("");
            let body = blocks(children(n), prefix);
            if title.is_empty() {
                body
            } else {
                format!(
                    "{prefix}**{}**\n{}\n{body}",
                    escape(title),
                    prefix.trim_end()
                )
            }
        }
        _ if n.get("content").is_some() => blocks(children(n), prefix),
        _ => prefixed(&inline_node(n), prefix),
    }
}

fn list(n: &Value, kind: &str, prefix: &str) -> String {
    let start = attr(n, "order").and_then(Value::as_u64).unwrap_or(1);
    children(n)
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let bullet = match kind {
                "orderedList" => format!("{}. ", start + i as u64),
                "taskList" => match attr(item, "state").and_then(Value::as_str) {
                    Some("DONE") => "- [x] ".to_string(),
                    _ => "- [ ] ".to_string(),
                },
                _ => "- ".to_string(),
            };
            let inner = format!("{prefix}{}", " ".repeat(bullet.len()));
            // `taskItem` traz os inlines direto; `listItem`, blocos.
            let body = if item.get("type").and_then(Value::as_str) == Some("taskItem") {
                inlines(children(item))
            } else {
                children(item)
                    .iter()
                    .map(|b| block(b, &inner))
                    .filter(|b| !b.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let body = body.strip_prefix(&inner).unwrap_or(&body).to_string();
            format!("{prefix}{bullet}{body}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn table(n: &Value) -> String {
    let mut lines = Vec::new();
    for (i, row) in children(n).iter().enumerate() {
        let cells: Vec<String> = children(row)
            .iter()
            .map(|c| {
                children(c)
                    .iter()
                    .map(|b| inlines(children(b)))
                    .collect::<Vec<_>>()
                    .join("<br>")
                    .replace('|', "\\|")
            })
            .collect();
        lines.push(format!("| {} |", cells.join(" | ")));
        if i == 0 {
            lines.push(format!("|{}", " --- |".repeat(cells.len())));
        }
    }
    lines.join("\n")
}

fn inlines(nodes: &[Value]) -> String {
    nodes.iter().map(inline_node).collect()
}

fn inline_node(n: &Value) -> String {
    match n.get("type").and_then(Value::as_str).unwrap_or("") {
        "text" => {
            let text = n.get("text").and_then(Value::as_str).unwrap_or("");
            let marks = n.get("marks").and_then(Value::as_array);
            let has = |m: &str| {
                marks.is_some_and(|ms| {
                    ms.iter()
                        .any(|x| x.get("type").and_then(Value::as_str) == Some(m))
                })
            };
            let mut s = if has("code") {
                code_span(text)
            } else {
                escape(text)
            };
            if has("strong") {
                s = format!("**{s}**");
            }
            if has("em") {
                s = format!("*{s}*");
            }
            if has("strike") {
                s = format!("~~{s}~~");
            }
            let href = marks.and_then(|ms| {
                ms.iter()
                    .find(|x| x.get("type").and_then(Value::as_str) == Some("link"))
                    .and_then(|l| attr(l, "href"))
                    .and_then(Value::as_str)
            });
            match href {
                Some(h) => format!("[{s}]({h})"),
                None => s,
            }
        }
        "hardBreak" => "<br>".into(),
        "mention" => {
            let t = attr(n, "text").and_then(Value::as_str).unwrap_or("@?");
            if t.starts_with('@') {
                t.to_string()
            } else {
                format!("@{t}")
            }
        }
        "emoji" => attr(n, "text")
            .or_else(|| attr(n, "shortName"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "inlineCard" | "blockCard" => attr(n, "url")
            .and_then(Value::as_str)
            .map(|u| format!("<{u}>"))
            .unwrap_or_default(),
        "status" | "date" => attr(n, "text")
            .or_else(|| attr(n, "timestamp"))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                v => v.to_string(),
            })
            .unwrap_or_default(),
        _ => inlines(children(n)),
    }
}

/// Texto puro do ADF: `<` não abre HTML.
fn escape(s: &str) -> String {
    s.replace('<', "&lt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn titulos_enfase_e_codigo_inline() {
        let md = wiki_to_markdown(
            "h1. Contexto\r\nTexto com *negrito*, _itálico_, -riscado- e {{cod_igo}}.\r\nh3. Fim",
        );
        assert_eq!(
            md,
            "# Contexto\nTexto com **negrito**, *itálico*, ~~riscado~~ e `cod_igo`.\n### Fim"
        );
    }

    #[test]
    fn hifen_e_asterisco_no_meio_da_palavra_ficam() {
        assert_eq!(
            wiki_to_markdown("pré-requisito a-b-c 2*3*4 snake_case_name"),
            "pré-requisito a-b-c 2*3*4 snake_case_name"
        );
        assert_eq!(wiki_to_markdown("a - b - c"), "a - b - c");
    }

    #[test]
    fn listas_aninhadas_e_numeradas() {
        let md = wiki_to_markdown("* um\n** um.um\n# primeiro\n## sub\n- traço");
        assert_eq!(md, "- um\n  - um.um\n1. primeiro\n   1. sub\n- traço");
    }

    #[test]
    fn negrito_no_comeco_da_linha_nao_e_lista() {
        assert_eq!(wiki_to_markdown("*Atenção* aqui"), "**Atenção** aqui");
    }

    #[test]
    fn blocos_de_codigo_e_noformat_ficam_crus() {
        let md = wiki_to_markdown(
            "antes\n{code:java}\nint *a* = 1; // <b>\n{code}\n{noformat}\n_x_\n{noformat}",
        );
        assert_eq!(
            md,
            "antes\n\n```java\nint *a* = 1; // <b>\n```\n\n```\n_x_\n```"
        );
        assert_eq!(
            wiki_to_markdown("{code:title=A.sql|lang=sql}select 1{code}"),
            "```sql\nselect 1\n```"
        );
        // Sem fechamento: fecha no fim.
        assert_eq!(wiki_to_markdown("{code}\nx"), "```\nx\n```");
    }

    #[test]
    fn citacoes() {
        assert_eq!(wiki_to_markdown("bq. citado"), "> citado");
        assert_eq!(
            wiki_to_markdown("a\n{quote}\nlinha *1*\nlinha 2\n{quote}\nb"),
            "a\n\n> linha **1**\n> linha 2\n\nb"
        );
    }

    #[test]
    fn tabelas_com_e_sem_cabecalho() {
        let md = wiki_to_markdown("texto\n||Campo||Valor||\n|a|[x|https://x.io]|\n|b|{{c|d}}|");
        assert_eq!(
            md,
            "texto\n\n| Campo | Valor |\n| --- | --- |\n| a | [x](https://x.io) |\n| b | `c\\|d` |"
        );
        let md = wiki_to_markdown("|a|b|");
        assert_eq!(md, "| | |\n| --- | --- |\n| a | b |");
    }

    #[test]
    fn links_mencoes_e_imagens() {
        let md = wiki_to_markdown(
            "[Doc|https://d.io/a] [https://e.io] [~fulano] [mailto:a@b.c] [nada] \
             !https://i.io/a.png|width=300! !print.png!",
        );
        assert_eq!(
            md,
            "[Doc](https://d.io/a) <https://e.io> @fulano <mailto:a@b.c> [nada] \
             ![](https://i.io/a.png) [anexo: print.png]"
        );
        // `javascript:` não vira link.
        assert_eq!(
            wiki_to_markdown("[x|javascript:alert(1)]"),
            "[x|javascript:alert(1)]"
        );
    }

    #[test]
    fn html_do_texto_nao_vira_html() {
        assert_eq!(
            wiki_to_markdown("<script>alert(1)</script>"),
            "&lt;script>alert(1)&lt;/script>"
        );
    }

    #[test]
    fn cor_painel_regua_e_quebra() {
        assert_eq!(
            wiki_to_markdown("{panel:title=X}\n{color:red}vermelho{color}\\\\linha\n----\n{panel}"),
            "vermelho<br>linha\n---"
        );
    }

    #[test]
    fn descricao_ausente_texto_ou_adf() {
        assert_eq!(description_markdown(None), "");
        assert_eq!(description_markdown(Some(&Value::Null)), "");
        assert_eq!(description_markdown(Some(&json!("h2. Oi"))), "## Oi");
        let adf = json!({ "type": "doc", "version": 1, "content": [
            { "type": "paragraph", "content": [{ "type": "text", "text": "oi" }] }
        ]});
        assert_eq!(description_markdown(Some(&adf)), "oi");
    }

    #[test]
    fn adf_completo() {
        let t = |s: &str| json!({ "type": "text", "text": s });
        let p = |c: Value| json!({ "type": "paragraph", "content": [c] });
        let adf = json!({ "type": "doc", "version": 1, "content": [
            { "type": "heading", "attrs": { "level": 2 }, "content": [t("Título")] },
            { "type": "paragraph", "content": [
                t("a "),
                { "type": "text", "text": "forte", "marks": [{ "type": "strong" }] },
                t(" "),
                { "type": "text", "text": "x()", "marks": [{ "type": "code" }] },
                t(" "),
                { "type": "text", "text": "site", "marks": [
                    { "type": "link", "attrs": { "href": "https://s.io" } }
                ]},
                { "type": "hardBreak" },
                { "type": "mention", "attrs": { "text": "@Fulano" } },
                t(" <b>")
            ]},
            { "type": "bulletList", "content": [
                { "type": "listItem", "content": [
                    p(t("um")),
                    { "type": "orderedList", "content": [
                        { "type": "listItem", "content": [p(t("sub"))] }
                    ]}
                ]},
                { "type": "listItem", "content": [p(t("dois"))] }
            ]},
            { "type": "taskList", "content": [
                { "type": "taskItem", "attrs": { "state": "DONE" }, "content": [t("feito")] },
                { "type": "taskItem", "attrs": { "state": "TODO" }, "content": [t("falta")] }
            ]},
            { "type": "codeBlock", "attrs": { "language": "rust" }, "content": [t("fn a() {}")] },
            { "type": "blockquote", "content": [p(t("citado"))] },
            { "type": "rule" },
            { "type": "table", "content": [
                { "type": "tableRow", "content": [
                    { "type": "tableHeader", "content": [p(t("H1"))] },
                    { "type": "tableHeader", "content": [p(t("H2"))] }
                ]},
                { "type": "tableRow", "content": [
                    { "type": "tableCell", "content": [p(t("a|b"))] },
                    { "type": "tableCell", "content": [p(t("c"))] }
                ]}
            ]},
            { "type": "mediaSingle", "content": [{ "type": "media", "attrs": {} }] }
        ]});
        assert_eq!(
            adf_to_markdown(&adf),
            "## Título\n\n\
             a **forte** `x()` [site](https://s.io)<br>@Fulano &lt;b>\n\n\
             - um\n  1. sub\n- dois\n\n\
             - [x] feito\n- [ ] falta\n\n\
             ```rust\nfn a() {}\n```\n\n\
             > citado\n\n\
             ---\n\n\
             | H1 | H2 |\n| --- | --- |\n| a\\|b | c |\n\n\
             [anexo]"
        );
    }
}

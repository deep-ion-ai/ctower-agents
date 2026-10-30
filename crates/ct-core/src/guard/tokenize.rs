//! Tokenização mínima de comandos shell: separa segmentos por `; && || | & ( ) \n`
//! e palavras por espaço, respeitando aspas simples, duplas e `\`.
//! Descarta comentários (`#` no início de palavra) e o corpo de heredocs
//! (`<<W`, `<<-W`, `<<'W'`, `<<"W"`). Não expande variáveis, globs nem
//! substituições de comando.

pub fn split_segments(input: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = input.chars().collect();
    let mut segments = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    // Heredocs pendentes: (delimitador, remove tabs à esquerda).
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '\'' => {
                in_word = true;
                while i < chars.len() {
                    let d = chars[i];
                    i += 1;
                    if d == '\'' {
                        break;
                    }
                    cur.push(d);
                }
            }
            '"' => {
                in_word = true;
                i = read_double_quoted(&chars, i, &mut cur);
            }
            '\\' => {
                in_word = true;
                if let Some(&n) = chars.get(i) {
                    i += 1;
                    if n != '\n' {
                        cur.push(n);
                    }
                }
            }
            '#' if !in_word => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '<' if chars.get(i) == Some(&'<') && chars.get(i + 1) == Some(&'<') => {
                // Here-string (`<<<`): não é heredoc.
                in_word = true;
                cur.push_str("<<<");
                i += 2;
            }
            '<' if chars.get(i) == Some(&'<') => {
                flush(&mut cur, &mut in_word, &mut words);
                i += 1;
                let strip_tabs = chars.get(i) == Some(&'-');
                if strip_tabs {
                    i += 1;
                }
                while i < chars.len() && matches!(chars[i], ' ' | '\t') {
                    i += 1;
                }
                let (delim, next) = read_delimiter(&chars, i);
                i = next;
                if !delim.is_empty() {
                    heredocs.push((delim, strip_tabs));
                }
            }
            ';' | '|' | '&' | '(' | ')' | '\n' => {
                flush(&mut cur, &mut in_word, &mut words);
                if (c == '&' || c == '|') && chars.get(i) == Some(&c) {
                    i += 1;
                }
                if !words.is_empty() {
                    segments.push(std::mem::take(&mut words));
                }
                if c == '\n' {
                    for (delim, strip_tabs) in heredocs.drain(..) {
                        i = skip_heredoc_body(&chars, i, &delim, strip_tabs);
                    }
                }
            }
            c if c.is_whitespace() => flush(&mut cur, &mut in_word, &mut words),
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    flush(&mut cur, &mut in_word, &mut words);
    if !words.is_empty() {
        segments.push(words);
    }
    segments
}

/// Lê o conteúdo de aspas duplas a partir de `i` (logo após a aspa de abertura);
/// devolve o índice após a aspa de fechamento.
fn read_double_quoted(chars: &[char], mut i: usize, cur: &mut String) -> usize {
    while i < chars.len() {
        let d = chars[i];
        i += 1;
        match d {
            '"' => break,
            '\\' => match chars.get(i) {
                Some(&n) if matches!(n, '"' | '\\' | '$' | '`') => {
                    cur.push(n);
                    i += 1;
                }
                _ => cur.push('\\'),
            },
            _ => cur.push(d),
        }
    }
    i
}

/// Lê a palavra delimitadora de um heredoc, removendo aspas e escapes.
fn read_delimiter(chars: &[char], mut i: usize) -> (String, usize) {
    let mut delim = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '<' | '>') {
            break;
        }
        i += 1;
        match c {
            '\'' => {
                while i < chars.len() {
                    let d = chars[i];
                    i += 1;
                    if d == '\'' {
                        break;
                    }
                    delim.push(d);
                }
            }
            '"' => i = read_double_quoted(chars, i, &mut delim),
            '\\' => {
                if let Some(&n) = chars.get(i) {
                    delim.push(n);
                    i += 1;
                }
            }
            _ => delim.push(c),
        }
    }
    (delim, i)
}

/// Pula as linhas do corpo do heredoc a partir de `i` (início de linha), até e
/// incluindo a linha igual ao delimitador; devolve o índice seguinte.
fn skip_heredoc_body(chars: &[char], mut i: usize, delim: &str, strip_tabs: bool) -> usize {
    while i < chars.len() {
        let start = i;
        while i < chars.len() && chars[i] != '\n' {
            i += 1;
        }
        let line: String = chars[start..i].iter().collect();
        if i < chars.len() {
            i += 1;
        }
        let line = if strip_tabs {
            line.trim_start_matches('\t')
        } else {
            line.as_str()
        };
        if line == delim {
            break;
        }
    }
    i
}

fn flush(cur: &mut String, in_word: &mut bool, words: &mut Vec<String>) {
    if *in_word {
        words.push(std::mem::take(cur));
        *in_word = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(v: &[&[&str]]) -> Vec<Vec<String>> {
        v.iter()
            .map(|s| s.iter().map(|w| w.to_string()).collect())
            .collect()
    }

    #[test]
    fn separa_por_operadores() {
        assert_eq!(
            split_segments("git add . && git commit -m x; git push | cat"),
            seg(&[
                &["git", "add", "."],
                &["git", "commit", "-m", "x"],
                &["git", "push"],
                &["cat"]
            ])
        );
        assert_eq!(
            split_segments("a || b\nc & d"),
            seg(&[&["a"], &["b"], &["c"], &["d"]])
        );
    }

    #[test]
    fn operadores_dentro_de_aspas_nao_separam() {
        assert_eq!(
            split_segments(r#"git commit -m "fix && git push origin develop""#),
            seg(&[&["git", "commit", "-m", "fix && git push origin develop"]])
        );
        assert_eq!(
            split_segments("echo 'a; b' | grep x"),
            seg(&[&["echo", "a; b"], &["grep", "x"]])
        );
    }

    #[test]
    fn escapes_e_aspas_concatenadas() {
        assert_eq!(
            split_segments(r#"echo a\ b "c\"d" e'f'"#),
            seg(&[&["echo", "a b", "c\"d", "ef"]])
        );
    }

    #[test]
    fn string_vazia_entre_aspas_vira_palavra() {
        assert_eq!(
            split_segments(r#"git commit -m """#),
            seg(&[&["git", "commit", "-m", ""]])
        );
    }

    #[test]
    fn heredoc_descarta_corpo() {
        assert_eq!(
            split_segments("cat <<EOF > README.md\ngit checkout main\nEOF\nls"),
            seg(&[&["cat", ">", "README.md"], &["ls"]])
        );
        assert_eq!(
            split_segments("cat <<'W' && echo a\nx; y\nW\nb"),
            seg(&[&["cat"], &["echo", "a"], &["b"]])
        );
        assert_eq!(
            split_segments("cat <<-\"W\"\n\tgit push\n\tW\nb"),
            seg(&[&["cat"], &["b"]])
        );
        assert_eq!(
            split_segments("cat <<A <<B\na\nA\nb\nB\nc"),
            seg(&[&["cat"], &["c"]])
        );
        // Sem terminador: descarta até o fim.
        assert_eq!(split_segments("cat <<EOF\ngit push"), seg(&[&["cat"]]));
        // Here-string não é heredoc.
        assert_eq!(
            split_segments("cat <<< x\ny"),
            seg(&[&["cat", "<<<", "x"], &["y"]])
        );
    }

    #[test]
    fn parenteses_separam() {
        assert_eq!(
            split_segments("(cd x && git push)"),
            seg(&[&["cd", "x"], &["git", "push"]])
        );
        assert_eq!(split_segments("echo '(a)'"), seg(&[&["echo", "(a)"]]));
    }

    #[test]
    fn comentario_ate_fim_da_linha() {
        assert_eq!(split_segments("a # b && c\nd"), seg(&[&["a"], &["d"]]));
        assert_eq!(
            split_segments("echo a#b '#c'"),
            seg(&[&["echo", "a#b", "#c"]])
        );
    }

    #[test]
    fn entrada_vazia() {
        assert!(split_segments("   ").is_empty());
        assert!(split_segments(";;").is_empty());
    }
}

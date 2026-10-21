//! Line lexer with OpenSSH semantics (`readconf.c` `process_config_line_depth` +
//! `misc.c` `argv_split(…, terminate_on_comment = 1)`):
//! - the keyword is case-insensitive and ends at whitespace or `=`; `Keyword=value`,
//!   `Keyword = value` and tabs are all accepted;
//! - arguments are split on spaces/tabs; `"…"` and `'…'` quote; `\'`, `\"`, `\\` and (outside
//!   quotes) `\ ` are escapes, any other backslash is kept literally (Windows paths survive);
//! - a `#` at the **start of a token** ends the line (full-line and trailing comments);
//!   a `#` inside a token is literal.

/// One meaningful config line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LexedLine {
    /// Lower-cased keyword.
    pub keyword: String,
    /// At least one argument.
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lexed {
    /// Blank line or comment.
    Empty,
    Line(LexedLine),
    /// Unterminated quote, missing argument, or a line starting with `=`.
    Bad,
}

fn is_ws(c: char) -> bool {
    c == ' ' || c == '\t'
}

pub(crate) fn lex_line(raw: &str) -> Lexed {
    let line = raw.trim_end_matches(['\r', '\n', ' ', '\t', '\x0c']);
    let s = line.trim_start_matches(is_ws);
    if s.is_empty() || s.starts_with('#') {
        return Lexed::Empty;
    }
    let kw_end = s.find(|c: char| is_ws(c) || c == '=').unwrap_or(s.len());
    let (keyword, rest) = s.split_at(kw_end);
    if keyword.is_empty() {
        return Lexed::Bad;
    }
    let rest = rest.trim_start_matches(is_ws);
    let rest = rest
        .strip_prefix('=')
        .unwrap_or(rest)
        .trim_start_matches(is_ws);
    match argv_split(rest) {
        Some(args) if !args.is_empty() => Lexed::Line(LexedLine {
            keyword: keyword.to_ascii_lowercase(),
            args,
        }),
        _ => Lexed::Bad,
    }
}

/// `argv_split(s, terminate_on_comment = 1)`. `None` on an unterminated quote.
pub(crate) fn argv_split(s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    loop {
        while it.next_if(|c| is_ws(*c)).is_some() {}
        match it.peek() {
            None | Some('#') => break,
            Some(_) => {}
        }
        let mut arg = String::new();
        let mut quote: Option<char> = None;
        loop {
            let Some(c) = it.next() else {
                if quote.is_some() {
                    return None;
                }
                break;
            };
            if c == '\\' {
                match it.peek().copied() {
                    Some(n)
                        if n == '\'' || n == '"' || n == '\\' || (quote.is_none() && n == ' ') =>
                    {
                        arg.push(n);
                        it.next();
                    }
                    _ => arg.push('\\'),
                }
            } else if quote.is_none() && is_ws(c) {
                break;
            } else if quote.is_none() && (c == '"' || c == '\'') {
                quote = Some(c);
            } else if quote == Some(c) {
                quote = None;
            } else {
                arg.push(c);
            }
        }
        out.push(arg);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn line(kw: &str, args: &[&str]) -> Lexed {
        Lexed::Line(LexedLine {
            keyword: kw.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        })
    }

    #[test]
    fn keyword_forms() {
        assert_eq!(
            lex_line("HostName example.com"),
            line("hostname", &["example.com"])
        );
        assert_eq!(
            lex_line("HostName=example.com"),
            line("hostname", &["example.com"])
        );
        assert_eq!(
            lex_line("  HostName = example.com  "),
            line("hostname", &["example.com"])
        );
        assert_eq!(lex_line("\tPORT\t2222\r"), line("port", &["2222"]));
        assert_eq!(lex_line("Host a b\tc"), line("host", &["a", "b", "c"]));
    }

    #[test]
    fn comments() {
        assert_eq!(lex_line(""), Lexed::Empty);
        assert_eq!(lex_line("   "), Lexed::Empty);
        assert_eq!(lex_line("# comment"), Lexed::Empty);
        assert_eq!(lex_line("   # indented comment"), Lexed::Empty);
        assert_eq!(lex_line("Port 22 # trailing"), line("port", &["22"]));
        assert_eq!(
            lex_line("Port 22#notacomment"),
            line("port", &["22#notacomment"])
        );
        assert_eq!(lex_line("Host a #b c"), line("host", &["a"]));
    }

    #[test]
    fn quotes_and_escapes() {
        assert_eq!(
            lex_line(r#"IdentityFile "~/.ssh/my key""#),
            line("identityfile", &["~/.ssh/my key"])
        );
        assert_eq!(
            lex_line("IdentityFile '~/a b'"),
            line("identityfile", &["~/a b"])
        );
        assert_eq!(lex_line(r#"X "a 'b' c""#), line("x", &["a 'b' c"]));
        assert_eq!(lex_line(r"X a\ b"), line("x", &["a b"]));
        assert_eq!(lex_line(r#"X a\"b"#), line("x", &["a\"b"]));
        assert_eq!(lex_line(r"X a\\b"), line("x", &[r"a\b"]));
        assert_eq!(
            lex_line(r"IdentityFile C:\Users\alice\.ssh\id"),
            line("identityfile", &[r"C:\Users\alice\.ssh\id"])
        );
        assert_eq!(
            lex_line(r##"X "# not a comment""##),
            line("x", &["# not a comment"])
        );
        assert_eq!(lex_line(r#"X ab"c d"e"#), line("x", &["abc de"]));
    }

    #[test]
    fn bad_lines() {
        assert_eq!(lex_line(r#"IdentityFile "unterminated"#), Lexed::Bad);
        assert_eq!(lex_line("Port"), Lexed::Bad);
        assert_eq!(lex_line("Port   # nothing"), Lexed::Bad);
        assert_eq!(lex_line("= value"), Lexed::Bad);
    }
}

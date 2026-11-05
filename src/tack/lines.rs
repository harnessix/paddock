//! Find the assignments of a variable in shell startup files, environment.d
//! files and .env files, and rewrite one so that it holds a reference.
//!
//! Only plain forms are rewritten: one assignment on the line, with a
//! literal value (unquoted, in single quotes, or in double quotes without
//! `$` or a backquote). A value that the shell computes, or a line with
//! more than one command, is reported and left as it is. The rewrite
//! changes only the value: the indentation, `export`, and a comment at the
//! end of the line stay.

use crate::credentials::secret::Secret;
use std::ops::Range;

/// The syntax of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// sh, bash, zsh startup files, environment.d and .env files:
    /// `[export ]NAME=VALUE`.
    Posix,
    /// fish: `set -gx NAME VALUE`.
    Fish,
}

impl Syntax {
    pub fn of(path: &std::path::Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("fish") => Syntax::Fish,
            _ => Syntax::Posix,
        }
    }
}

/// What one line says about one variable.
#[derive(Debug)]
pub enum Found {
    /// A literal value at `span` of the line.
    Value { span: Range<usize>, value: Secret },
    /// It holds a reference already.
    Reference,
    /// It sets the variable in a form that paddock does not rewrite.
    Unsupported(&'static str),
    /// It sets the variable to nothing. `find` never returns it.
    Empty,
}

/// What `line` does with the variable `name`, or None when it does not
/// set it.
pub fn find(line: &str, name: &str, syntax: Syntax) -> Option<Found> {
    let found = match syntax {
        Syntax::Posix => posix(line, name),
        Syntax::Fish => fish(line, name),
    };
    match found {
        Some(Found::Empty) => return None,
        Some(f) => return Some(f),
        None => {}
    }
    // The name followed by '=' somewhere else on a line that is not a
    // comment: `a=1; export NAME=x`, `export A=1 NAME=2`, `env NAME=x cmd`.
    let t = line.trim_start();
    if t.starts_with('#') {
        return None;
    }
    let mentions = line.match_indices(name).any(|(i, _)| {
        let before = line[..i].chars().next_back();
        let after = &line[i + name.len()..];
        before.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
            && (after.starts_with('=')
                || (syntax == Syntax::Fish && after.starts_with(' ') && t.starts_with("set ")))
    });
    mentions.then_some(Found::Unsupported(
        "the line does more than set the variable",
    ))
}

fn posix(line: &str, name: &str) -> Option<Found> {
    let start = line.len() - line.trim_start().len();
    let mut rest = &line[start..];
    let mut at = start;
    for kw in ["export ", "export\t"] {
        if let Some(r) = rest.strip_prefix(kw) {
            let r2 = r.trim_start();
            at += rest.len() - r2.len();
            rest = r2;
            break;
        }
    }
    let after = rest.strip_prefix(name)?.strip_prefix('=')?;
    let vstart = at + name.len() + 1;
    let (end, value) = match word(after, Syntax::Posix) {
        Ok(v) => v,
        Err(why) => return Some(Found::Unsupported(why)),
    };
    finish(line, vstart, vstart + end, value)
}

fn fish(line: &str, name: &str) -> Option<Found> {
    let start = line.len() - line.trim_start().len();
    let rest = line[start..].strip_prefix("set")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut pos = start + 3;
    let mut exported = false;
    // Flags, then the name.
    loop {
        let r = &line[pos..];
        let r2 = r.trim_start();
        pos += r.len() - r2.len();
        let tok_len = r2.find([' ', '\t']).unwrap_or(r2.len());
        let tok = &r2[..tok_len];
        if tok == "--erase"
            || tok == "--query"
            || (tok.starts_with('-') && !tok.starts_with("--") && tok.contains(['e', 'q']))
        {
            return None;
        }
        if tok.starts_with('-') {
            exported |= tok == "--export" || (!tok.starts_with("--") && tok.contains('x'));
            pos += tok_len;
            continue;
        }
        if tok != name {
            return None;
        }
        pos += tok_len;
        break;
    }
    if !exported {
        return Some(Found::Unsupported(
            "a fish variable without -x is not in the environment",
        ));
    }
    let r = &line[pos..];
    let r2 = r.trim_start();
    if r2.len() == r.len() {
        return Some(Found::Unsupported("the variable has no value"));
    }
    let vstart = pos + (r.len() - r2.len());
    let (end, value) = match word(r2, Syntax::Fish) {
        Ok(v) => v,
        Err(why) => return Some(Found::Unsupported(why)),
    };
    finish(line, vstart, vstart + end, value)
}

/// Check what follows the value, then decide.
fn finish(line: &str, vstart: usize, vend: usize, value: Secret) -> Option<Found> {
    let tail = &line[vend..];
    let t = tail.trim_start();
    if !t.is_empty() && !(t.starts_with('#') && t.len() < tail.len()) {
        return Some(Found::Unsupported(
            "the line does more than set the variable",
        ));
    }
    if value.is_empty() {
        return Some(Found::Empty);
    }
    if super::Reference::looks_like(value.as_str()) {
        return Some(Found::Reference);
    }
    Some(Found::Value {
        span: vstart..vend,
        value,
    })
}

/// Read one shell word with a literal value. Returns its length in `s` and
/// its value.
fn word(s: &str, syntax: Syntax) -> Result<(usize, Secret), &'static str> {
    const COMPUTED: &str = "the shell computes the value ($ or a backquote)";
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    let wipe = |mut v: Vec<u8>| {
        v.iter_mut().for_each(|c| *c = 0);
    };
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\r' | b'\n' => break,
            b'\'' => {
                let Some(end) = s[i + 1..].find('\'') else {
                    wipe(out);
                    return Err("a quote is not closed on this line");
                };
                // fish reads \' and \\ in single quotes.
                if syntax == Syntax::Fish && s[i + 1..i + 1 + end].contains('\\') {
                    wipe(out);
                    return Err("the value has a backslash in fish quotes");
                }
                out.extend_from_slice(&b[i + 1..i + 1 + end]);
                i += end + 2;
            }
            b'"' => {
                i += 1;
                loop {
                    match b.get(i) {
                        None => {
                            wipe(out);
                            return Err("a quote is not closed on this line");
                        }
                        Some(b'"') => {
                            i += 1;
                            break;
                        }
                        Some(b'$') | Some(b'`') => {
                            wipe(out);
                            return Err(COMPUTED);
                        }
                        Some(b'(') if syntax == Syntax::Fish => {
                            wipe(out);
                            return Err(COMPUTED);
                        }
                        Some(b'\\') if matches!(b.get(i + 1), Some(b'"' | b'\\' | b'$' | b'`')) => {
                            out.push(b[i + 1]);
                            i += 2;
                        }
                        Some(&c) => {
                            out.push(c);
                            i += 1;
                        }
                    }
                }
            }
            b'\\' if syntax == Syntax::Fish => {
                wipe(out);
                return Err("the value has a backslash escape of fish");
            }
            b'\\' => match b.get(i + 1) {
                Some(&c) if c != b'\n' => {
                    out.push(c);
                    i += 2;
                }
                _ => {
                    wipe(out);
                    return Err("the value goes on to the next line");
                }
            },
            b'$' | b'`' => {
                wipe(out);
                return Err(COMPUTED);
            }
            b'(' | b')' if syntax == Syntax::Fish => {
                wipe(out);
                return Err(COMPUTED);
            }
            b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')' => {
                wipe(out);
                return Err("the line does more than set the variable");
            }
            b'{' | b'*' | b'?' | b'[' | b'~'
                if syntax == Syntax::Fish || b[i] == b'~' && i == 0 =>
            {
                wipe(out);
                return Err(COMPUTED);
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok((i, Secret::new(out)))
}

/// The line with the value at `span` replaced by `text`.
pub fn rewrite(line: &str, span: &Range<usize>, text: &str) -> String {
    format!("{}{text}{}", &line[..span.start], &line[span.end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(line: &str, syntax: Syntax) -> (String, String) {
        match find(line, "GH_TOKEN", syntax) {
            Some(Found::Value { span, value }) => (
                value.as_str().to_string(),
                rewrite(line, &span, "tack://GH_TOKEN"),
            ),
            other => panic!("{line:?}: {other:?}"),
        }
    }

    #[test]
    fn plain_posix_forms_are_found_and_rewritten_in_place() {
        for (line, want, new) in [
            (
                "export GH_TOKEN=fake1",
                "fake1",
                "export GH_TOKEN=tack://GH_TOKEN",
            ),
            ("GH_TOKEN=fake1", "fake1", "GH_TOKEN=tack://GH_TOKEN"),
            (
                "  export   GH_TOKEN='fa ke'  # work",
                "fa ke",
                "  export   GH_TOKEN=tack://GH_TOKEN  # work",
            ),
            (
                "export GH_TOKEN=\"fa\\\"ke\\\\\"",
                "fa\"ke\\",
                "export GH_TOKEN=tack://GH_TOKEN",
            ),
            (
                "GH_TOKEN=fa'k e'1\r",
                "fak e1",
                "GH_TOKEN=tack://GH_TOKEN\r",
            ),
            (
                "export\tGH_TOKEN=a#b",
                "a#b",
                "export\tGH_TOKEN=tack://GH_TOKEN",
            ),
        ] {
            assert_eq!(
                value(line, Syntax::Posix),
                (want.into(), new.into()),
                "{line}"
            );
        }
    }

    #[test]
    fn fish_forms_are_found_and_rewritten() {
        for (line, want, new) in [
            (
                "set -gx GH_TOKEN fake1",
                "fake1",
                "set -gx GH_TOKEN tack://GH_TOKEN",
            ),
            (
                "  set --export GH_TOKEN 'fake 2' # x",
                "fake 2",
                "  set --export GH_TOKEN tack://GH_TOKEN # x",
            ),
            (
                "set -Ux GH_TOKEN fake3",
                "fake3",
                "set -Ux GH_TOKEN tack://GH_TOKEN",
            ),
        ] {
            assert_eq!(
                value(line, Syntax::Fish),
                (want.into(), new.into()),
                "{line}"
            );
        }
        assert!(matches!(
            find("set -g GH_TOKEN x", "GH_TOKEN", Syntax::Fish),
            Some(Found::Unsupported(_))
        ));
        for line in [
            "set -gx GH_TOKEN (cat f)",
            "set -gx GH_TOKEN 'a\\b'",
            "set -gx GH_TOKEN a\\nb",
        ] {
            assert!(
                matches!(
                    find(line, "GH_TOKEN", Syntax::Fish),
                    Some(Found::Unsupported(_))
                ),
                "{line}"
            );
        }
        assert!(find("set -e GH_TOKEN", "GH_TOKEN", Syntax::Fish).is_none());
    }

    #[test]
    fn computed_values_and_busy_lines_are_reported_and_not_rewritten() {
        for line in [
            "export GH_TOKEN=$(pass show gh)",
            "export GH_TOKEN=\"$OTHER\"",
            "export GH_TOKEN=`cat f`",
            "export GH_TOKEN=fake; echo hi",
            "export A=1 GH_TOKEN=fake",
            "a=1; export GH_TOKEN=fake",
            "GH_TOKEN=fake gh auth status",
            "export GH_TOKEN=\"fake",
            "export GH_TOKEN=fake\\",
            "export GH_TOKEN=fake&&x",
        ] {
            assert!(
                matches!(
                    find(line, "GH_TOKEN", Syntax::Posix),
                    Some(Found::Unsupported(_))
                ),
                "{line}"
            );
        }
    }

    #[test]
    fn other_lines_are_not_about_the_variable() {
        for line in [
            "# export GH_TOKEN=fake",
            "export MY_GH_TOKEN=fake",
            "export GH_TOKENS=fake",
            "export GH_TOKEN=",
            "echo $GH_TOKEN",
            "unset GH_TOKEN",
            "",
        ] {
            assert!(find(line, "GH_TOKEN", Syntax::Posix).is_none(), "{line}");
        }
    }

    #[test]
    fn a_reference_is_seen_as_done() {
        for (line, s) in [
            ("export GH_TOKEN=tack://GH_TOKEN", Syntax::Posix),
            ("GH_TOKEN='tack://GH_TOKEN'", Syntax::Posix),
            ("set -gx GH_TOKEN tack://GH_TOKEN", Syntax::Fish),
        ] {
            assert!(
                matches!(find(line, "GH_TOKEN", s), Some(Found::Reference)),
                "{line}"
            );
        }
    }
}

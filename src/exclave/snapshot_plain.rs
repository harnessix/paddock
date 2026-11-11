//! File selection for a project without a Git worktree.
use super::*;
use crate::safefs::Kind;
use std::ffi::CString;

struct Rule {
    base: String,
    pattern: String,
    negate: bool,
    directory: bool,
    anchored: bool,
}

fn rules(dir: &crate::safefs::Dir, file: &str, base: &str, out: &mut Vec<Rule>) -> Result<()> {
    if dir.kind(file.as_ref())? != Some(Kind::File) {
        return Ok(());
    }
    let data = dir
        .read(file.as_ref(), 1024 * 1024)?
        .context("ignore file disappeared")?;
    let text = std::str::from_utf8(&data).context("ignore file is not UTF-8")?;
    for line in text.lines() {
        let mut line = line.trim_end_matches('\r');
        // Unescaped trailing spaces are ignored; escaped spaces are significant.
        while line.ends_with(' ') && !line.ends_with("\\ ") {
            line = &line[..line.len() - 1];
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let negate = line.starts_with('!');
        if negate {
            line = &line[1..];
        }
        if line.is_empty() {
            continue;
        }
        let directory = line.ends_with('/');
        if directory {
            line = &line[..line.len() - 1];
        }
        let anchored = line.starts_with('/') || line.contains('/');
        line = line.strip_prefix('/').unwrap_or(line);
        if !line.is_empty() {
            out.push(Rule {
                base: base.into(),
                pattern: line.into(),
                negate,
                directory,
                anchored,
            });
        }
    }
    Ok(())
}

fn component(pattern: &str, text: &str) -> bool {
    let (Ok(pattern), Ok(text)) = (CString::new(pattern), CString::new(text)) else {
        return false;
    };
    // SAFETY: both arguments are valid, NUL-terminated strings.
    unsafe { libc::fnmatch(pattern.as_ptr(), text.as_ptr(), 0) == 0 }
}

fn matches_path(pattern: &[&str], text: &[&str]) -> bool {
    // Dynamic programming bounds the work even for many ** components.
    let mut prev = vec![false; text.len() + 1];
    prev[0] = true;
    for (index, part) in pattern.iter().enumerate() {
        let mut next = vec![false; text.len() + 1];
        if *part == "**" {
            // A trailing /** matches contents, not the directory itself.
            if index + 1 != pattern.len() {
                next[0] = prev[0];
            }
            for i in 1..=text.len() {
                next[i] = prev[i] && index + 1 != pattern.len() || prev[i - 1] || next[i - 1];
            }
        } else {
            for i in 1..=text.len() {
                next[i] = prev[i - 1] && component(part, text[i - 1]);
            }
        }
        prev = next;
    }
    prev[text.len()]
}

fn ignored(rules: &[Rule], rel: &str, is_dir: bool) -> bool {
    let mut excluded = false;
    for rule in rules {
        if rule.directory && !is_dir {
            continue;
        }
        let local = if rule.base.is_empty() {
            Some(rel)
        } else {
            rel.strip_prefix(&rule.base)
                .and_then(|s| s.strip_prefix('/'))
        };
        let Some(local) = local else {
            continue;
        };
        let matched = if rule.anchored {
            matches_path(
                &rule.pattern.split('/').collect::<Vec<_>>(),
                &local.split('/').collect::<Vec<_>>(),
            )
        } else {
            component(&rule.pattern, local.rsplit('/').next().unwrap_or(local))
        };
        if matched {
            excluded = !rule.negate;
        }
    }
    excluded
}

pub(super) fn capture(project: &Path, hidden: &[PathBuf]) -> Result<Snapshot> {
    let fs = crate::safefs::Root::open(project)?;
    let root = fs.dir(Path::new(""), false)?;
    let mut ruleset = Vec::new();
    rules(&root, ".paddockignore", "", &mut ruleset)?;
    let mut names = BTreeSet::new();
    walk(&fs, "", hidden, &mut ruleset, &mut names, 0)?;
    build(project, names, hidden, true)
}

fn walk(
    fs: &crate::safefs::Root,
    rel: &str,
    hidden: &[PathBuf],
    ruleset: &mut Vec<Rule>,
    names: &mut BTreeSet<String>,
    depth: usize,
) -> Result<()> {
    if depth > 256 {
        bail!("snapshot directory nesting exceeds limit");
    }
    let dir = fs.dir(Path::new(rel), false)?;
    let before = ruleset.len();
    rules(&dir, ".gitignore", rel, ruleset)?;
    for (name, kind) in dir.entries()? {
        let name = name
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF-8 snapshot path"))?;
        if name == ".git" || name == ".paddock" {
            continue;
        }
        let path = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if !permitted(&path, hidden)? || ignored(ruleset, &path, kind == Kind::Dir) {
            continue;
        }
        match kind {
            Kind::Dir => walk(fs, &path, hidden, ruleset, names, depth + 1)?,
            Kind::File | Kind::Symlink => {
                names.insert(path);
                if names.len() > MAX_FILES {
                    bail!("snapshot has too many files");
                }
            }
            _ => {}
        }
    }
    ruleset.truncate(before);
    Ok(())
}

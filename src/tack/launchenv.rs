//! The environment of a program that tackd starts: the caller's, without
//! the variables that load other code into the program, with the real
//! values of the allowed secrets.
//!
//! The caller's environment comes in the request, so it is hostile input.
//! A variable such as `LD_PRELOAD` would run any library inside the
//! program that holds the values; tackd drops it, and the ones like it
//! for the dynamic loader and the common interpreters. Variables that
//! name a command that the program runs as a child (`PAGER`, `EDITOR`,
//! `GIT_SSH_COMMAND`) stay: they are the user's settings, and a child of
//! the program gets the values in its environment as it would from a
//! shell.

use crate::credentials::secret::Secret;

/// Names that make a program load code, by exact name.
const DROPPED: &[&str] = &[
    "GCONV_PATH",
    "GLIBC_TUNABLES",
    "HOSTALIASES",
    "LOCPATH",
    "MALLOC_TRACE",
    "NLSPATH",
    "RESOLV_HOST_CONF",
    "NODE_OPTIONS",
    "NODE_PATH",
    "PYTHONPATH",
    "PYTHONHOME",
    "PYTHONSTARTUP",
    "PYTHONUSERBASE",
    "PYTHONINSPECT",
    "PYTHONBREAKPOINT",
    "PYTHONEXECUTABLE",
    "PERL5LIB",
    "PERLLIB",
    "PERL5OPT",
    "PERL5DB",
    "RUBYOPT",
    "RUBYLIB",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "LUA_INIT",
    "LUA_PATH",
    "LUA_CPATH",
    "TCLLIBPATH",
    "BASH_ENV",
    "ENV",
];

/// Name prefixes that make a program load code.
const DROPPED_PREFIX: &[&str] = &["LD_", "DYLD_", "LUA_INIT_", "BASH_FUNC_"];

/// Whether tackd drops the variable `name` from the caller's environment.
pub fn dropped(name: &str) -> bool {
    DROPPED.contains(&name) || DROPPED_PREFIX.iter().any(|p| name.starts_with(p))
}

/// The environment for the program, as `NAME=value` entries with a final
/// NUL, ready for execve, and the names that were dropped. A secret
/// replaces the caller's variable of the same name (the reference).
pub fn build(caller: &[String], secrets: &[(String, Secret)]) -> (Vec<Secret>, Vec<String>) {
    let mut out = Vec::new();
    let mut gone = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for e in caller {
        let Some((name, _)) = e.split_once('=') else {
            continue;
        };
        if name.is_empty() || e.contains('\0') {
            continue;
        }
        if secrets.iter().any(|(n, _)| n == name) {
            continue;
        }
        if dropped(name) {
            gone.push(name.to_string());
            continue;
        }
        // The first of a name wins, as getenv does.
        if !seen.insert(name.to_string()) {
            continue;
        }
        let mut b = e.as_bytes().to_vec();
        b.push(0);
        out.push(Secret::new(b));
    }
    for (name, value) in secrets {
        let mut b = Vec::with_capacity(name.len() + value.len() + 2);
        b.extend_from_slice(name.as_bytes());
        b.push(b'=');
        b.extend_from_slice(value.as_bytes());
        b.push(0);
        out.push(Secret::new(b));
    }
    gone.sort();
    gone.dedup();
    (out, gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &[Secret]) -> Vec<String> {
        v.iter()
            .map(|s| {
                String::from_utf8_lossy(s.as_bytes())
                    .trim_end_matches('\0')
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn loader_variables_go_secrets_come_in_and_the_rest_stays() {
        let caller: Vec<String> = [
            "PATH=/usr/bin",
            "GH_TOKEN=tack://GH_TOKEN",
            "LD_PRELOAD=/tmp/evil.so",
            "DYLD_INSERT_LIBRARIES=/tmp/x.dylib",
            "NODE_OPTIONS=--require /tmp/x.js",
            "BASH_FUNC_f%%=() { :; }",
            "PAGER=less",
            "PATH=/second",
            "NPM_TOKEN=tack://NPM_TOKEN",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (env, gone) = build(&caller, &[("GH_TOKEN".into(), Secret::from("fake-v"))]);
        assert_eq!(
            text(&env),
            [
                "PATH=/usr/bin",
                "PAGER=less",
                "NPM_TOKEN=tack://NPM_TOKEN",
                "GH_TOKEN=fake-v"
            ]
        );
        assert_eq!(
            gone,
            [
                "BASH_FUNC_f%%",
                "DYLD_INSERT_LIBRARIES",
                "LD_PRELOAD",
                "NODE_OPTIONS"
            ]
        );
        assert!(env.iter().all(|e| e.as_bytes().ends_with(&[0])));
    }

    #[test]
    fn a_secret_is_set_even_when_the_caller_has_no_reference() {
        let (env, _) = build(&[], &[("A".into(), Secret::from("v"))]);
        assert_eq!(text(&env), ["A=v"]);
        assert!(dropped("LD_LIBRARY_PATH") && dropped("PYTHONPATH"));
        assert!(!dropped("EDITOR") && !dropped("HOME"));
    }
}

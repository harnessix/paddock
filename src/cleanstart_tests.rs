use super::*;

fn os(v: &[&str]) -> Vec<OsString> {
    v.iter().map(OsString::from).collect()
}

#[test]
fn the_flag_is_read_from_the_status() {
    assert!(no_new_privs("Name:\tbash\nNoNewPrivs:\t1\nSeccomp:\t0\n"));
    assert!(!no_new_privs("NoNewPrivs:\t0\n"));
    assert!(!no_new_privs("Name:\tbash\n"));
}

#[test]
fn only_the_commands_that_start_a_sandbox_start_again() {
    assert!(starts_sandbox(&os(&["run", "claude", "."])));
    assert!(starts_sandbox(&os(&["-v", "run", "claude"])));
    assert!(starts_sandbox(&os(&["shell", "."])));
    assert!(starts_sandbox(&os(&["wrap", "code"])));
    assert!(starts_sandbox(&os(&["app", "run", "vscode"])));
    assert!(!starts_sandbox(&os(&["app", "list"])));
    assert!(!starts_sandbox(&os(&["forage"])));
    assert!(!starts_sandbox(&os(&["host", "run"])));
    assert!(!starts_sandbox(&os(&[])));
}

#[test]
fn the_unit_carries_the_arguments_the_environment_and_the_terminal() {
    let vars = vec![
        (OsString::from("HOME"), OsString::from("/home/demo")),
        (OsString::from("BAD"), OsString::from("a\nb")),
        (OsString::from("SENTRY-TRACE"), OsString::from("x")),
        (OsString::from("1X"), OsString::from("x")),
    ];
    let a = unit_args(
        Path::new("/bin/paddock"),
        &os(&["run", "claude", "."]),
        &vars,
        true,
    );
    let s: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
    assert_eq!(
        &s[..5],
        ["--user", "--quiet", "--collect", "--wait", "--same-dir"]
    );
    assert!(s.contains(&"--pty".to_string()));
    assert!(s.contains(&"--setenv=HOME=/home/demo".to_string()));
    assert!(!s.iter().any(|x| x.starts_with("--setenv=BAD")));
    assert!(!s
        .iter()
        .any(|x| x.starts_with("--setenv=SENTRY") || x.starts_with("--setenv=1X")));
    assert!(s.contains(&format!("--setenv={AGAIN_VAR}=1")));
    let dash = s.iter().position(|x| x == "--").unwrap();
    assert_eq!(&s[dash + 1..], ["/bin/paddock", "run", "claude", "."]);
    let p = unit_args(Path::new("/bin/paddock"), &os(&["run"]), &[], false);
    assert!(p.contains(&OsString::from("--pipe")));
}

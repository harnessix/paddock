use super::*;
use crate::tack::store::tests::Memory;
use std::rc::Rc;

/// A fake value. No real secret appears in these tests.
const V1: &str = "fake-value-one";
const V2: &str = "fake-value-two";

struct World {
    _t: tempfile::TempDir,
    env: HostEnv,
    store: Rc<Memory>,
}

fn world(vars: &[(&str, &str)]) -> World {
    let t = tempfile::tempdir().unwrap();
    let mut env = crate::privfs::tests::env_in(t.path());
    for (k, v) in vars {
        env.vars.insert(k.to_string(), v.to_string());
    }
    std::fs::create_dir_all(&env.home).unwrap();
    World {
        _t: t,
        env,
        store: Rc::new(Memory::default()),
    }
}

/// A Memory store behind a shared handle, so the test can look into it.
struct Shared(Rc<Memory>);
impl Store for Shared {
    fn describe(&self) -> String {
        self.0.describe()
    }
    fn writable(&self) -> bool {
        self.0.writable()
    }
    fn get(&self, n: &str) -> Result<Secret, StoreError> {
        self.0.get(n)
    }
    fn put(&self, n: &str, v: &Secret) -> Result<(), StoreError> {
        self.0.put(n, v)
    }
}

impl World {
    fn file(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.env.home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }
    fn plan(&self, names: &[&str], files: &[&Path], write: bool, replace: bool) -> Plan {
        let opts = Options {
            names: names.iter().map(|s| s.to_string()).collect(),
            files: files.iter().map(|p| p.to_path_buf()).collect(),
            write,
            replace,
        };
        let store = self.store.clone();
        let open = move |_: &str, _: &SecretCfg| -> Result<Box<dyn Store>, StoreError> {
            Ok(Box::new(Shared(store.clone())))
        };
        plan(&self.env, &Tack::default(), &opts, &open).unwrap()
    }
    fn stored(&self, name: &str) -> Option<String> {
        self.store
            .values
            .borrow()
            .get(name)
            .map(|v| String::from_utf8(v.clone()).unwrap())
    }
}

fn text(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

/// Every line of output, and the log, must be free of values.
fn assert_no_value(w: &World, lines: &[String]) {
    let all = lines.join("\n");
    let log = std::fs::read_to_string(crate::tack::log_file(&w.env)).unwrap_or_default();
    for v in [V1, V2] {
        assert!(!all.contains(v), "a value in the output:\n{all}");
        assert!(!log.contains(v), "a value in the log:\n{log}");
    }
}

#[test]
fn a_dry_run_changes_nothing_and_names_each_line() {
    let w = world(&[]);
    let rc = w.file(".bashrc", &format!("# mine\nexport GH_TOKEN={V1}\n"));
    let p = w.plan(&["GH_TOKEN"], &[], false, false);
    assert_eq!(p.names[0].action, Action::Store { replace: false });
    let lines = p.describe(&w.env);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("~/.bashrc:2  becomes  export GH_TOKEN=tack://GH_TOKEN")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("no [tack.secrets.GH_TOKEN] table")),
        "{lines:?}"
    );
    assert_no_value(&w, &lines);
    assert_eq!(text(&rc), format!("# mine\nexport GH_TOKEN={V1}\n"));
    assert_eq!(w.stored("GH_TOKEN"), None);
}

#[test]
fn import_stores_then_rewrites_keeps_mode_and_backs_up() {
    let w = world(&[]);
    let rc = w.file(".bashrc", &format!("a=1\nexport GH_TOKEN='{V1}' # gh\n"));
    std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o640)).unwrap();
    let env_d = w.file(
        ".config/environment.d/10-tokens.conf",
        &format!("GH_TOKEN={V1}\n"),
    );
    let dotenv = w.file("proj/.env", &format!("OTHER=1\nGH_TOKEN=\"{V1}\"\n"));
    let mut p = w.plan(&["GH_TOKEN"], &[&dotenv], true, false);
    let written = p.apply(&w.env);
    assert_eq!(written.len(), 3, "{written:?}");
    assert_eq!(w.stored("GH_TOKEN").as_deref(), Some(V1));
    assert_eq!(text(&rc), "a=1\nexport GH_TOKEN=tack://GH_TOKEN # gh\n");
    assert_eq!(text(&env_d), "GH_TOKEN=tack://GH_TOKEN\n");
    assert_eq!(text(&dotenv), "OTHER=1\nGH_TOKEN=tack://GH_TOKEN\n");
    assert_eq!(
        std::fs::metadata(&rc).unwrap().permissions().mode() & 0o777,
        0o640
    );
    for wr in &written {
        let b = wr.result.as_ref().unwrap();
        assert_eq!(crate::privfs::tests::mode(b), 0o600);
        assert!(text(b).contains(V1), "the backup holds the old file");
    }
    // No temporary file stays behind.
    let left: Vec<_> = std::fs::read_dir(&w.env.home)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains("paddock-tack"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    let events = crate::tack::events(&w.env);
    assert_eq!(events.iter().filter(|e| e["event"] == "import").count(), 3);
    assert_no_value(&w, &p.describe(&w.env));
    // A second run finds references and does nothing.
    let p2 = w.plan(&["GH_TOKEN"], &[&dotenv], true, false);
    assert_eq!(p2.names[0].action, Action::Done);
}

#[test]
fn a_store_that_fails_or_forgets_leaves_every_file_as_it_was() {
    for broken in [
        Memory {
            error: Some(StoreError::Locked("test".into())).into(),
            ..Default::default()
        },
        Memory {
            forget: true,
            ..Default::default()
        },
    ] {
        let mut w = world(&[]);
        w.store = Rc::new(broken);
        let rc = w.file(".zshrc", &format!("export GH_TOKEN={V1}\n"));
        let mut p = w.plan(&["GH_TOKEN"], &[], true, false);
        let written = p.apply(&w.env);
        assert!(written.is_empty(), "{written:?}");
        assert_eq!(text(&rc), format!("export GH_TOKEN={V1}\n"));
        assert!(!p.ok());
        assert!(
            matches!(&p.names[0].action, Action::Refused(why) if why.contains("locked") || why.contains("no file changed")),
            "{:?}",
            p.names[0].action
        );
    }
}

#[test]
fn a_crash_after_the_store_write_is_finished_by_the_next_run() {
    let w = world(&[]);
    let rc = w.file(".profile", &format!("export GH_TOKEN={V1}\n"));
    // The store holds the value, the file still has it: the state after a
    // crash between the two steps.
    w.store
        .values
        .borrow_mut()
        .insert("GH_TOKEN".into(), V1.into());
    let mut p = w.plan(&["GH_TOKEN"], &[], true, false);
    assert_eq!(p.names[0].action, Action::Rewrite);
    p.apply(&w.env);
    assert_eq!(text(&rc), "export GH_TOKEN=tack://GH_TOKEN\n");
}

#[test]
fn a_failed_rename_keeps_the_old_file_and_removes_the_temporary_one() {
    let w = world(&[]);
    let rc = w.file(".bashrc", &format!("export GH_TOKEN={V1}\n"));
    let mut p = w.plan(&["GH_TOKEN"], &[], true, false);
    FAIL_RENAME.with(|f| f.set(true));
    let written = p.apply(&w.env);
    FAIL_RENAME.with(|f| f.set(false));
    let e = written[0].result.as_ref().unwrap_err();
    assert!(e.contains("did not change") && e.contains("backup"), "{e}");
    assert_eq!(text(&rc), format!("export GH_TOKEN={V1}\n"));
    let names: Vec<String> = std::fs::read_dir(&w.env.home)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".bashrc"))
        .collect();
    assert_eq!(names, [".bashrc"]);
}

#[test]
fn a_file_that_changes_during_the_import_is_not_overwritten() {
    let w = world(&[]);
    let rc = w.file(".bashrc", &format!("export GH_TOKEN={V1}\n"));
    let mut p = w.plan(&["GH_TOKEN"], &[], true, false);
    std::fs::write(&rc, format!("export GH_TOKEN={V1}\nalias x=y\n")).unwrap();
    let written = p.apply(&w.env);
    assert!(written[0]
        .result
        .as_ref()
        .unwrap_err()
        .contains("changed after"));
    assert!(text(&rc).contains("alias x=y"));
}

#[test]
fn different_values_and_a_different_stored_value_are_refused() {
    let w = world(&[]);
    w.file(".bashrc", &format!("export GH_TOKEN={V1}\n"));
    w.file(".zshrc", &format!("export GH_TOKEN={V2}\n"));
    let p = w.plan(&["GH_TOKEN"], &[], true, false);
    let Action::Refused(why) = &p.names[0].action else {
        panic!("{:?}", p.names[0].action)
    };
    assert!(
        why.contains("2 different values") && why.contains("~/.zshrc:1"),
        "{why}"
    );
    assert_no_value(&w, &p.describe(&w.env));

    let w = world(&[]);
    let rc = w.file(".bashrc", &format!("export GH_TOKEN={V1}\n"));
    w.store
        .values
        .borrow_mut()
        .insert("GH_TOKEN".into(), V2.into());
    let mut p = w.plan(&["GH_TOKEN"], &[], true, false);
    assert!(matches!(&p.names[0].action, Action::Refused(w) if w.contains("--replace")));
    assert!(p.apply(&w.env).is_empty());
    assert!(text(&rc).contains(V1));
    let mut p = w.plan(&["GH_TOKEN"], &[], true, true);
    assert_eq!(p.names[0].action, Action::Store { replace: true });
    p.apply(&w.env);
    assert_eq!(w.stored("GH_TOKEN").as_deref(), Some(V1));
}

#[test]
fn a_read_only_store_only_confirms_the_value() {
    let mut w = world(&[]);
    w.store = Rc::new(Memory {
        read_only: true,
        ..Default::default()
    });
    let rc = w.file(".bashrc", &format!("export GH_TOKEN={V1}\n"));
    let p = w.plan(&["GH_TOKEN"], &[], true, false);
    assert!(matches!(&p.names[0].action, Action::Refused(w) if w.contains("password manager")));
    w.store
        .values
        .borrow_mut()
        .insert("GH_TOKEN".into(), V1.into());
    let mut p = w.plan(&["GH_TOKEN"], &[], true, false);
    assert_eq!(p.names[0].action, Action::Rewrite);
    p.apply(&w.env);
    assert_eq!(text(&rc), "export GH_TOKEN=tack://GH_TOKEN\n");
}

#[test]
fn a_linked_dotfile_is_written_through_its_link() {
    let w = world(&[]);
    let target = w.file("dotfiles/bashrc", &format!("export GH_TOKEN={V1}\n"));
    std::os::unix::fs::symlink(&target, w.env.home.join(".bashrc")).unwrap();
    let mut p = w.plan(&["GH_TOKEN"], &[], true, false);
    p.apply(&w.env);
    assert!(std::fs::symlink_metadata(w.env.home.join(".bashrc"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(text(&target), "export GH_TOKEN=tack://GH_TOKEN\n");
}

#[test]
fn notes_name_forms_it_skips_and_values_only_in_the_environment() {
    let w = world(&[("GH_TOKEN", V1), ("OTHER", "tack://OTHER")]);
    w.file(".bashrc", "export GH_TOKEN=$(pass show gh)\n");
    let p = w.plan(&["GH_TOKEN", "OTHER", "GH_TOKEN"], &[], false, false);
    assert_eq!(p.names.len(), 2);
    assert_eq!(p.names[0].action, Action::NotFound);
    let lines = p.describe(&w.env);
    let all = lines.join("\n");
    assert!(
        all.contains("~/.bashrc:1  not changed: the shell computes the value"),
        "{all}"
    );
    assert!(all.contains("plain value in this environment"), "{all}");
    assert!(!all.contains("OTHER has a plain value"), "{all}");
    assert_no_value(&w, &lines);
}

#[test]
fn bad_names_and_missing_files_are_refused_before_anything_runs() {
    let w = world(&[]);
    let open =
        |_: &str, _: &SecretCfg| -> Result<Box<dyn Store>, StoreError> { panic!("no store") };
    let o = |names: &[&str], files: Vec<PathBuf>| Options {
        names: names.iter().map(|s| s.to_string()).collect(),
        files,
        write: false,
        replace: false,
    };
    let e = plan(&w.env, &Tack::default(), &o(&[], vec![]), &open)
        .err()
        .unwrap();
    assert!(e.to_string().contains("name at least one"));
    let e = plan(&w.env, &Tack::default(), &o(&["A-B"], vec![]), &open)
        .err()
        .unwrap();
    assert!(e.to_string().contains("not an environment variable name"));
    let e = plan(
        &w.env,
        &Tack::default(),
        &o(&["A"], vec![w.env.home.join("nope")]),
        &open,
    )
    .err()
    .unwrap();
    assert!(e.to_string().contains("is not a file"));
}

#[test]
fn files_that_cannot_be_read_as_text_are_skipped_and_named() {
    let w = world(&[]);
    std::fs::write(w.env.home.join(".bashrc"), [0xff, 0xfe, b'\n']).unwrap();
    let p = w.plan(&["GH_TOKEN"], &[], false, false);
    assert!(p.describe(&w.env)[0].contains("skipped  ~/.bashrc: it is not UTF-8 text"));
}

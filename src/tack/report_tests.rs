use super::*;
use crate::credentials::secret::Secret;
use crate::tack::store::tests::Memory;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const V1: &str = "fake-value-one";

fn world(cfg: &str, vars: &[(&str, &str)]) -> (tempfile::TempDir, HostEnv) {
    let t = tempfile::tempdir().unwrap();
    let mut env = crate::privfs::tests::env_in(t.path());
    for (k, v) in vars {
        env.vars.insert(k.to_string(), v.to_string());
    }
    std::fs::create_dir_all(&env.home).unwrap();
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(config::file(&env), cfg).unwrap();
    (t, env)
}

fn tool(dir: &std::path::Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, "#!/bin/true\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn doctor(env: &HostEnv, store: Memory) -> Vec<String> {
    let store = std::rc::Rc::new(store);
    struct S(std::rc::Rc<Memory>);
    impl Store for S {
        fn describe(&self) -> String {
            "memory".into()
        }
        fn writable(&self) -> bool {
            true
        }
        fn get(&self, n: &str) -> Result<Secret, StoreError> {
            self.0.get(n)
        }
        fn put(&self, n: &str, v: &Secret) -> Result<(), StoreError> {
            self.0.put(n, v)
        }
    }
    let open = move |_: &str, _: &SecretCfg| -> Result<Box<dyn Store>, StoreError> {
        Ok(Box::new(S(store.clone())))
    };
    doctor_with(env, &open)
}

#[test]
fn doctor_is_one_line_without_secrets() {
    let (_t, env) = world("", &[]);
    assert_eq!(
        doctor(&env, Memory::default()),
        ["  off      no secrets are set up (`paddock help environment`)"]
    );
    std::fs::write(config::file(&env), "[tack.secrets.A]\nbackend = 2\n").unwrap();
    let l = doctor(&env, Memory::default());
    assert!(l[0].starts_with("  error    [tack] in"), "{l:?}");
}

#[test]
fn doctor_names_store_plain_values_and_changed_programs() {
    let t = tempfile::tempdir().unwrap();
    let gh = tool(t.path(), "gh");
    let hash = program::sha256_file(&gh).unwrap();
    let git = tool(t.path(), "git");
    let cfg = format!(
        "[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"{}\"\nsha256 = \"{hash}\"\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"{}\"\nsha256 = \"{hash}\"\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"/no/such/tool\"\n[tack.secrets.NPM_TOKEN]\n",
        gh.display(),
        git.display()
    );
    std::fs::write(&git, "#!/bin/false\n").unwrap();
    let (_w, env) = world(&cfg, &[("GH_TOKEN", V1), ("NPM_TOKEN", "tack://NPM_TOKEN")]);
    std::fs::write(
        env.home.join(".bashrc"),
        format!("export GH_TOKEN={V1}\nexport NPM_TOKEN=$(cat f)\n"),
    )
    .unwrap();
    let store = Memory::default();
    store
        .values
        .borrow_mut()
        .insert("GH_TOKEN".into(), V1.into());
    let l = doctor(&env, store);
    let all = l.join("\n");
    for want in [
        "  ok       GH_TOKEN  in the memory",
        "  changed  GH_TOKEN  ",
        "  missing  GH_TOKEN  allowlisted program /no/such/tool is not there",
        "  plain    GH_TOKEN  has a plain value in ~/.bashrc:1: paddock tack import GH_TOKEN",
        "  plain    GH_TOKEN  has a plain value in this environment",
        "  missing  NPM_TOKEN  is not in the memory: paddock tack import NPM_TOKEN",
        "  note     NPM_TOKEN has no allowlist",
        "  plain?   NPM_TOKEN  is set in ~/.bashrc:2 in a form",
        PHASE_NOTE,
    ] {
        assert!(all.contains(want), "missing {want:?} in:\n{all}");
    }
    assert!(
        !all.contains(&format!("{}  changed", gh.display())),
        "{all}"
    );
    assert!(
        !all.contains("NPM_TOKEN  has a plain value in this environment"),
        "{all}"
    );
    assert!(!all.contains(V1), "{all}");
}

#[test]
fn doctor_names_a_locked_store_and_old_backups() {
    let (_t, env) = world("[tack.secrets.GH_TOKEN]\n", &[]);
    crate::privfs::write(&crate::tack::backup_dir(&env).join("1-2/x"), "old").unwrap();
    let l = doctor(
        &env,
        Memory {
            error: Some(StoreError::Locked("test".into())).into(),
            ..Default::default()
        },
    );
    let all = l.join("\n");
    assert!(
        all.contains("  error    GH_TOKEN  the keyring is locked (test): unlock it"),
        "{all}"
    );
    assert!(all.contains("1 backups in"), "{all}");
}

#[test]
fn list_has_configured_and_imported_names_and_no_value() {
    let (_t, env) = world(
        "[tack.secrets.B]\nbackend = \"command\"\ncommand = [\"op\", \"read\", \"x\"]\nhosts = [\"api.example.com\"]\n",
        &[],
    );
    crate::tack::log_event(&env, serde_json::json!({"event": "import", "name": "A"}));
    let r = rows(&env, &config::load(&env).unwrap());
    assert_eq!(r.len(), 2);
    assert_eq!((r[0].name.as_str(), r[0].configured), ("A", false));
    assert!(r[0].last_import.is_some());
    assert_eq!(r[1].reference, "tack://B");
    assert_eq!(r[1].store, "command op");
    assert_eq!(r[1].hosts, ["api.example.com"]);
}

#[test]
fn explain_says_what_the_sandbox_sees() {
    let (_t, env) = world("[tack.secrets.GH_TOKEN]\n[tack.secrets.NPM_TOKEN]\n", &[]);
    let var = |v: &str| crate::spec::EnvVar {
        value: v.into(),
        origin: "host env".into(),
        secret: true,
    };
    let mut vars = BTreeMap::new();
    vars.insert("GH_TOKEN".to_string(), var("tack://GH_TOKEN"));
    vars.insert("OTHER".to_string(), var("tack://OTHER"));
    vars.insert("NPM_TOKEN".to_string(), var(V1));
    let l = explain(&env, &vars).join("\n");
    assert!(
        l.contains("  GH_TOKEN  the sandbox sees tack://GH_TOKEN"),
        "{l}"
    );
    assert!(
        l.contains(
            "  NPM_TOKEN  enters the sandbox as a plain value: run paddock tack import NPM_TOKEN"
        ),
        "{l}"
    );
    assert!(l.contains("  OTHER  the sandbox sees tack://OTHER"), "{l}");
    assert!(l.contains(PHASE_NOTE));
    assert!(!l.contains(V1), "{l}");
    let (_t, env) = world("", &[]);
    assert!(explain(&env, &BTreeMap::new()).is_empty());
}

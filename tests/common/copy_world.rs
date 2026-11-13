//! The world of the copy tests (tests/copy_cli.rs and
//! tests/copy_pointer.rs): a temp home, a project with three files, and
//! the built paddock with its folders there.

use paddock::copy::run::Run;
use paddock::copy::{start, Config};
use paddock::env::HostEnv;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub struct World {
    _tmp: super::TestDir,
    pub root: PathBuf,
    pub project: PathBuf,
}

const VARS: [(&str, &str); 5] = [
    ("XDG_CONFIG_HOME", "cfg"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_RUNTIME_DIR", "run"),
];

pub fn put(dir: &Path, rel: &str, text: &str) {
    let f = dir.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, text).unwrap();
}

pub fn get(dir: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(rel)).ok()
}

pub fn world() -> World {
    let tmp = super::tempdir();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    for d in ["home", "cfg/paddock/harness.d", "run"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(
        root.join("cfg/paddock/harness.d/fake.toml"),
        "name = \"fake\"\n[locate]\ncommands = [\"sh\"]\n",
    )
    .unwrap();
    let project = root.join("webshop");
    put(&project, "README.md", "the shop\n");
    put(&project, "src/cart.ts", "one\ntwo\nthree\n");
    put(&project, "src/price.ts", "price\n");
    World {
        _tmp: tmp,
        root,
        project,
    }
}

impl World {
    /// The environment that the built paddock gets, as the library sees it.
    pub fn env(&self) -> HostEnv {
        let vars = VARS
            .iter()
            .map(|(k, d)| (k.to_string(), self.root.join(d).display().to_string()))
            .collect();
        HostEnv::from_parts(self.root.join("home"), "demo".into(), 1000, 1000, vars)
    }

    pub fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.env("HOME", self.root.join("home"))
            .env_remove("TMUX")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .current_dir(&self.project)
            .stdin(Stdio::null());
        for (k, d) in VARS {
            c.env(k, self.root.join(d));
        }
        c
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().unwrap()
    }

    /// Run a command that must work, and parse its JSON.
    pub fn json(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        let (out, err) = (text(&o.stdout), text(&o.stderr));
        assert!(o.status.success(), "{args:?}: {out}{err}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    /// Start a run on a copy, as `paddock run --copy` does.
    pub fn start(&self, id: &str) -> Run {
        start::create(
            &self.env(),
            &Config::default(),
            &self.project,
            (id, "fake"),
            true,
            &mut |_, q| panic!("unexpected question: {q}"),
        )
        .unwrap()
        .run
    }
}

pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

pub fn strs(v: &Value) -> Vec<&str> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect()
}

/// The id of the run in most tests.
pub const ID: &str = "fake-webshop-1a2b3c";

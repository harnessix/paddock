use super::*;
use std::cell::RefCell;
use std::rc::Rc;

#[path = "setup_password_tests.rs"]
mod password;
#[path = "setup_route_tests.rs"]
mod route;

/// A backend of files: each state writes what the real one writes, and a
/// script says how each call ends.
struct Fake {
    /// The manifest state as seen at each call, for the "never sealed
    /// early" claim.
    seen: Rc<RefCell<Vec<(String, State)>>>,
    golden_manifest: PathBuf,
    guided: Vec<Result<End, String>>,
    configure: Vec<Result<End, String>>,
    /// The answers of the panel to `ask_password`, in order.
    asked: Vec<Result<Option<String>, String>>,
    /// The password of each configure call.
    configured_with: Rc<RefCell<Vec<String>>>,
    install_fails: bool,
    reject: Vec<PathBuf>,
    latest_size: Option<u64>,
    short_download: usize,
    calls: Rc<RefCell<Vec<String>>>,
    /// The host version, and the version and build of the image.
    host_version: String,
    image: (String, String),
    support: std::result::Result<(), String>,
    /// The route of each guided call, by its word and its reason.
    routes: Rc<RefCell<Vec<String>>>,
    /// The disk tool of the offline seed and trust; `None`: no tool.
    disk: Option<Rc<RefCell<crate::macvm::offline_fake::FakeDisk>>>,
    /// The answers of the trust check after the offline patch.
    trusted: Vec<Result<Option<bool>, String>>,
}

impl Fake {
    fn new(golden: &Golden) -> Self {
        Self {
            seen: Rc::new(RefCell::new(Vec::new())),
            golden_manifest: golden.manifest.clone(),
            guided: vec![Ok(End::Done)],
            configure: vec![Ok(End::Done)],
            asked: Vec::new(),
            configured_with: Rc::new(RefCell::new(Vec::new())),
            install_fails: false,
            reject: Vec::new(),
            latest_size: Some(10),
            short_download: 0,
            calls: Rc::new(RefCell::new(Vec::new())),
            host_version: "15.2".into(),
            image: ("15.2".into(), "24C101".into()),
            support: Err("VZMacGuestProvisioningOptions".into()),
            routes: Rc::new(RefCell::new(Vec::new())),
            disk: None,
            trusted: vec![Ok(Some(true))],
        }
    }

    fn note(&self, what: &str) {
        let state = std::fs::read_to_string(&self.golden_manifest)
            .ok()
            .and_then(|t| Manifest::parse(&t, &self.golden_manifest).ok())
            .map(|m| m.state)
            .unwrap_or(State::Fetch);
        self.seen.borrow_mut().push((what.to_string(), state));
        self.calls.borrow_mut().push(what.to_string());
    }
}

impl Backend for Fake {
    fn host(&self) -> HostInfo {
        HostInfo {
            version: self.host_version.clone(),
            build: "24C101".into(),
        }
    }
    fn latest(&mut self) -> Result<RestoreInfo> {
        self.note("latest");
        Ok(RestoreInfo {
            url: "https://updates.example/15.2.ipsw".into(),
            version: "15.2".into(),
            build: "24C101".into(),
            size: self.latest_size,
        })
    }
    fn download(&mut self, _url: &str, dest: &Path, _size: Option<u64>) -> Result<()> {
        self.note("download");
        assert!(layout::is_partial(dest));
        let bytes = if self.short_download > 0 {
            self.short_download -= 1;
            b"short".to_vec()
        } else {
            b"ipsw-bytes".to_vec()
        };
        std::fs::write(dest, bytes)?;
        Ok(())
    }
    fn check_image(&mut self, path: &Path) -> Result<ImageInfo> {
        self.note("check_image");
        if self.reject.iter().any(|r| r == path) {
            bail!("mostFeaturefulSupportedConfiguration is nil");
        }
        Ok(ImageInfo {
            version: self.image.0.clone(),
            build: self.image.1.clone(),
            hardware_model_b64: "aGFyZHdhcmU=".into(),
        })
    }
    fn install(&mut self, golden: &Golden, ipsw: &Path) -> Result<()> {
        self.note("install");
        assert!(ipsw.is_file());
        for f in [
            &golden.hardware_model,
            &golden.machine_id,
            &golden.aux,
            &golden.disk,
        ] {
            std::fs::write(layout::partial(f), "installed")?;
        }
        if let Some(d) = &self.disk {
            d.borrow().store(&layout::partial(&golden.disk));
        }
        if self.install_fails {
            bail!("the installer failed");
        }
        Ok(())
    }
    fn provisioning(&self) -> std::result::Result<(), String> {
        self.support.clone()
    }
    fn guided(&mut self, golden: &Golden, password: &str, route: &Route) -> Result<End> {
        self.note("guided");
        let why = route.why().map(|w| format!(" {w:?}")).unwrap_or_default();
        self.routes
            .borrow_mut()
            .push(format!("{}{why}", route.word()));
        // The boot shares the seed folder: it must be complete.
        assert!(super::seed::ready(&golden.seed), "no seed folder at guided");
        assert!(golden.disk.is_file() && !layout::partial(&golden.disk).exists());
        assert!(golden::is_password(password));
        match self.guided.remove(0) {
            Ok(e) => {
                // The real window ends without the offline trust only when
                // the agent reports the permission, and marks it.
                if e == End::Done && !route.trust_offline {
                    setup_events::record_person_once(&golden.events, setup_events::TRUSTED, "fake");
                }
                Ok(e)
            }
            Err(t) => bail!("{t}"),
        }
    }
    fn configure(&mut self, _golden: &Golden, password: &str) -> Result<End> {
        self.note("configure");
        assert!(golden::is_keyring_password(password));
        self.configured_with.borrow_mut().push(password.to_string());
        match self.configure.remove(0) {
            Ok(e) => Ok(e),
            Err(t) => bail!("{t}"),
        }
    }
    fn ask_password(&mut self, _golden: &Golden) -> Result<Option<String>> {
        self.note("ask_password");
        match self.asked.remove(0) {
            Ok(p) => Ok(p),
            Err(t) => bail!("{t}"),
        }
    }
    fn shutdown(&mut self, _golden: &Golden, password: &str) -> Result<()> {
        self.note("shutdown");
        assert!(golden::is_keyring_password(password));
        Ok(())
    }
    fn disk_tool(&mut self) -> Option<Box<dyn crate::macvm::diskimage::DiskTool>> {
        let d = self.disk.as_ref()?;
        let image = self.golden_manifest.with_file_name("disk.img");
        Some(Box::new(crate::macvm::offline_fake::Shared {
            d: d.clone(),
            image,
        }))
    }
    fn trusted(&mut self, _golden: &Golden, password: &str) -> Result<Option<bool>> {
        self.note("trusted");
        assert!(golden::is_keyring_password(password));
        match self.trusted.remove(0) {
            Ok(e) => Ok(e),
            Err(t) => bail!("{t}"),
        }
    }
}

struct World {
    _t: tempfile::TempDir,
    env: HostEnv,
    layout: Layout,
}

fn world() -> World {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let layout = Layout::new(&env);
    World { _t: t, env, layout }
}

fn run_with(w: &World, fake: &mut Fake, opts: &Opts, answer: bool) -> (i32, String) {
    let mut out = Vec::new();
    let mut ask = |_q: &str| Ok(answer);
    let mut ui = Ui {
        out: &mut out,
        ask: &mut ask,
    };
    let code = run(&w.env, &w.layout, &w.layout.golden, fake, opts, &mut ui).unwrap();
    (code, String::from_utf8(out).unwrap())
}

/// The options of a test: a fake free space, because the temporary
/// folder of the test host can be small.
fn roomy() -> Opts {
    Opts {
        fake_free: Some("1T".into()),
        ..Default::default()
    }
}

fn state_of(w: &World) -> State {
    w.layout.golden.read_manifest().unwrap().unwrap().state
}

#[test]
fn a_full_setup_seals_the_golden_and_never_says_sealed_early() {
    let w = world();
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(state_of(&w), State::Sealed);
    assert!(out.contains("golden ready (macOS 15.2 (24C101)"), "{out}");
    assert!(w.layout.golden.partials().is_empty());
    assert!(w.layout.ipsw_file("15.2").is_file());
    use std::os::unix::fs::PermissionsExt;
    for f in w.layout.golden.sealed_files() {
        assert_eq!(
            std::fs::metadata(f).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }
    // The manifest named each state before it ran, and said sealed only
    // after the shutdown.
    let seen = fake.seen.borrow();
    let at = |what: &str| {
        seen.iter()
            .find(|(w, _)| w == what)
            .map(|(_, s)| *s)
            .unwrap()
    };
    assert_eq!(at("latest"), State::Fetch);
    assert_eq!(at("install"), State::Install);
    assert_eq!(at("guided"), State::Guided);
    assert_eq!(at("configure"), State::Configure);
    assert_eq!(at("shutdown"), State::Seal);
    assert!(seen.iter().all(|(_, s)| *s != State::Sealed));
    assert!(
        !w.layout.golden.seed.exists(),
        "the seal removes the seed folder"
    );
    assert!(out.contains("the seed folder"), "{out}");
    assert!(out.contains(&super::seed::command()), "{out}");
    let states: Vec<String> = super::setup_events::read(&w.layout.golden.events)
        .into_iter()
        .filter(|l| l.kind == "state" && !l.person)
        .map(|l| l.text)
        .collect();
    assert_eq!(
        states,
        [
            "fetch",
            "fetch done",
            "install",
            "install done",
            "seed",
            "seed done",
            "guided",
            "guided done",
            "trust",
            "trust done",
            "configure",
            "configure done",
            "seal",
            "seal done"
        ]
    );
    let m = w.layout.golden.read_manifest().unwrap().unwrap();
    assert!(m.sealed.is_some());
    assert_eq!(m.hardware_model_b64, "aGFyZHdhcmU=");
    assert_eq!(m.ipsw_size, Some(10));
    // A second run finds the sealed golden and does nothing.
    let (code, out) = run_with(&w, &mut Fake::new(&w.layout.golden), &roomy(), true);
    assert_eq!(code, 0);
    assert!(out.contains("sealed already"), "{out}");
}

#[test]
fn the_password_never_reaches_the_terminal() {
    let w = world();
    let pw = Keyring::open(&w.env).unwrap().password().unwrap();
    let mut fake = Fake::new(&w.layout.golden);
    let (_, out) = run_with(&w, &mut fake, &roomy(), true);
    assert!(!out.contains(&pw));
    let manifest = std::fs::read_to_string(&w.layout.golden.manifest).unwrap();
    assert!(!manifest.contains(&pw));
}

#[test]
fn setup_with_missing_ipsw_fails_before_any_folder_is_made() {
    let w = world();
    let opts = Opts {
        ipsw: Some(w._t.path().join("none.ipsw")),
        ..Default::default()
    };
    let e = preflight(&w.env, &opts).unwrap_err().to_string();
    assert!(e.contains("none.ipsw") && e.contains("--ipsw"), "{e}");
    assert!(!w.layout.root.exists(), "no macvm folder");
    assert!(
        !w.env.paddock_state().join("keyring").exists(),
        "no keyring yet"
    );
    let mut fake = Fake::new(&w.layout.golden);
    let mut out = Vec::new();
    let mut ask = |_q: &str| Ok(true);
    let mut ui = Ui {
        out: &mut out,
        ask: &mut ask,
    };
    assert!(run(
        &w.env,
        &w.layout,
        &w.layout.golden,
        &mut fake,
        &opts,
        &mut ui
    )
    .is_err());
    assert!(!w.layout.root.exists());
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn setup_with_an_ipsw_file_skips_the_download_and_the_question() {
    let w = world();
    let ipsw = w._t.path().join("mine.ipsw");
    std::fs::write(&ipsw, "local").unwrap();
    let opts = Opts {
        ipsw: Some(ipsw.clone()),
        ..roomy()
    };
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &opts, false);
    assert_eq!(code, 0, "{out}");
    let calls = fake.calls.borrow();
    assert!(!calls.iter().any(|c| c == "latest" || c == "download"));
    let m = w.layout.golden.read_manifest().unwrap().unwrap();
    assert_eq!(m.ipsw.as_deref(), Some(ipsw.as_path()));
    assert_eq!(m.ipsw_size, Some(5));
}

#[test]
fn a_refused_question_stops_in_fetch() {
    let w = world();
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &roomy(), false);
    assert_eq!(code, EXIT_AGAIN);
    assert_eq!(state_of(&w), State::Fetch);
    assert!(out.contains("stopped in step 1 (fetch)"), "{out}");
    assert!(!fake.calls.borrow().iter().any(|c| c == "download"));
}

#[test]
fn no_space_refuses_before_the_download_with_both_sizes() {
    let w = world();
    let mut fake = Fake::new(&w.layout.golden);
    fake.latest_size = Some(15_000_000_000);
    let opts = Opts {
        fake_free: Some("1G".into()),
        ..Default::default()
    };
    let (code, out) = run_with(&w, &mut fake, &opts, true);
    assert_eq!(code, EXIT_AGAIN);
    assert!(out.contains("47 GB") && out.contains("1 GB"), "{out}");
    assert!(!fake.calls.borrow().iter().any(|c| c == "download"));
    assert_eq!(state_of(&w), State::Fetch);
}

#[test]
fn no_space_refuses_before_the_install() {
    let w = world();
    let opts = Opts {
        fake_free: Some("20G".into()),
        ..Default::default()
    };
    // Enough for the download alone, too little with the golden after it:
    // the fetch refuses before the download.
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &opts, true);
    assert_eq!(code, EXIT_AGAIN);
    assert!(
        out.contains("the download and the golden after it needs 32 GB") && out.contains("21 GB"),
        "{out}"
    );
    assert!(!fake.calls.borrow().iter().any(|c| c == "download"));
    assert_eq!(state_of(&w), State::Fetch);
    // An image that is there already: Install checks again.
    let mut fake = Fake::new(&w.layout.golden);
    fake.install_fails = true;
    assert_eq!(run_with(&w, &mut fake, &roomy(), true).0, EXIT_AGAIN);
    assert_eq!(state_of(&w), State::Install);
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &opts, true);
    assert_eq!(code, EXIT_AGAIN);
    assert!(
        out.contains("the install needs 32 GB") && out.contains("21 GB"),
        "{out}"
    );
    assert!(!fake.calls.borrow().iter().any(|c| c == "install"));
    assert_eq!(state_of(&w), State::Install);
    // With room, the next run continues at Install without a download.
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &roomy(), false);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("continues there"), "{out}");
    assert!(!fake
        .calls
        .borrow()
        .iter()
        .any(|c| c == "download" || c == "latest"));
}

#[test]
fn a_torn_download_is_fetched_again_not_installed() {
    let w = world();
    let mut fake = Fake::new(&w.layout.golden);
    fake.short_download = 1;
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("downloads it again"), "{out}");
    assert_eq!(
        fake.calls
            .borrow()
            .iter()
            .filter(|c| *c == "download")
            .count(),
        2
    );
    // Two torn downloads in a row fail the fetch.
    let w = world();
    let mut fake = Fake::new(&w.layout.golden);
    fake.short_download = 2;
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN);
    assert!(out.contains("twice"), "{out}");
    assert!(
        w.layout.ipsw.read_dir().unwrap().next().is_none(),
        "no partial stays"
    );
}

#[test]
fn an_image_that_fails_at_install_goes_back_to_fetch() {
    let w = world();
    // Run to Install, then damage the file: the size differs.
    let mut fake = Fake::new(&w.layout.golden);
    fake.install_fails = true;
    assert_eq!(run_with(&w, &mut fake, &roomy(), true).0, EXIT_AGAIN);
    assert_eq!(state_of(&w), State::Install);
    std::fs::write(w.layout.ipsw_file("15.2"), "torn").unwrap();
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("has 4 bytes, the download said 10"), "{out}");
    let calls = fake.calls.borrow();
    assert_eq!(calls.iter().filter(|c| *c == "download").count(), 1);
    assert_eq!(calls.iter().filter(|c| *c == "install").count(), 1);
    // A rejected image twice in a row stops in Fetch.
    let w = world();
    let mut fake = Fake::new(&w.layout.golden);
    fake.reject = vec![w.layout.ipsw_file("15.2")];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN, "{out}");
    assert!(out.contains("failed its check"), "{out}");
}

#[test]
fn a_failed_install_stays_in_install_and_leaves_no_partial_next_time() {
    let w = world();
    let mut fake = Fake::new(&w.layout.golden);
    fake.install_fails = true;
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN);
    assert!(
        out.contains("step 2 (install) failed: the installer failed"),
        "{out}"
    );
    assert_eq!(state_of(&w), State::Install);
    assert_eq!(
        w.layout.golden.partials().len(),
        4,
        "the installer's partials stay for the crash check"
    );
    // The next run sees them as a crash of Install, removes them, runs again.
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("crashed in step 2 (install)") && out.contains("disk.img.partial"),
        "{out}"
    );
    assert!(!fake.calls.borrow().iter().any(|c| c == "download"));
}

#[test]
fn crashed_seed_reruns_seed_only() {
    let w = world();
    // A manifest in Seed with the installed files and a partial of the seed.
    let g = &w.layout.golden;
    std::fs::create_dir_all(&g.dir).unwrap();
    for f in [&g.hardware_model, &g.machine_id, &g.aux, &g.disk] {
        std::fs::write(f, "installed").unwrap();
    }
    std::fs::write(g.dir.join("seed.partial"), "").unwrap();
    let mut m = Manifest::new("15.2", "24C101", "0.2.1");
    m.version = "15.2".into();
    m.ipsw = Some(w._t.path().join("x.ipsw"));
    m.at(State::Seed);
    g.write_manifest(&m).unwrap();
    let mut fake = Fake::new(g);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("crashed in step 3 (seed) and left seed.partial"),
        "{out}"
    );
    let calls = fake.calls.borrow();
    assert_eq!(calls[0], "guided", "{calls:?}");
    assert!(!calls.iter().any(|c| c == "install" || c == "download"));
    assert!(!g.dir.join("seed.partial").exists());
}

/// A golden of the given state with the installed files, as an older
/// build left it.
fn installed_at(w: &World, state: State) {
    let g = &w.layout.golden;
    std::fs::create_dir_all(&g.dir).unwrap();
    for f in [&g.hardware_model, &g.machine_id, &g.aux, &g.disk] {
        std::fs::write(f, "installed").unwrap();
    }
    let mut m = Manifest::new("15.2", "24C101", "0.2.1");
    m.ipsw = Some(w._t.path().join("x.ipsw"));
    m.at(state);
    g.write_manifest(&m).unwrap();
}

#[test]
fn an_older_seed_manifest_runs_the_new_seed_without_root() {
    let w = world();
    installed_at(&w, State::Seed);
    let agent = w._t.path().join("paddock-guest");
    std::fs::write(&agent, "agent").unwrap();
    let opts = Opts {
        agent: Some(agent),
        ..roomy()
    };
    let mut fake = Fake::new(&w.layout.golden);
    fake.guided = vec![Ok(End::Stopped)];
    let (code, out) = run_with(&w, &mut fake, &opts, true);
    assert_eq!(code, EXIT_AGAIN, "{out}");
    assert!(out.contains("step 3 of 7, seed"), "{out}");
    assert!(out.contains("holds the agent"), "{out}");
    assert_eq!(state_of(&w), State::Guided);
    let seed = &w.layout.golden.seed;
    assert!(super::seed::ready(seed));
    assert_eq!(
        std::fs::read_to_string(seed.join("agent/current/paddock-guest")).unwrap(),
        "agent"
    );
    assert_eq!(fake.calls.borrow().as_slice(), ["guided"]);
}

#[test]
fn an_older_guided_manifest_gets_the_seed_folder_before_the_boot() {
    let w = world();
    installed_at(&w, State::Guided);
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("the seed folder"), "{out}");
    assert_eq!(fake.calls.borrow()[0], "guided");
}

#[test]
fn a_seed_folder_of_another_version_is_made_again_at_guided() {
    let w = world();
    installed_at(&w, State::Guided);
    let seed = &w.layout.golden.seed;
    super::seed::prepare(seed, None).unwrap();
    std::fs::write(seed.join("install.sh"), "#!/bin/sh\necho old\n").unwrap();
    let mut fake = Fake::new(&w.layout.golden);
    fake.guided = vec![Ok(End::Stopped)];
    let (_, out) = run_with(&w, &mut fake, &roomy(), true);
    assert!(out.contains("the seed folder"), "{out}");
    assert_eq!(
        std::fs::read_to_string(seed.join("install.sh")).unwrap(),
        super::seed::INSTALL
    );
}

#[test]
fn a_seed_that_cannot_copy_the_agent_fails_and_stays_in_seed() {
    let w = world();
    installed_at(&w, State::Seed);
    let opts = Opts {
        agent: Some(w._t.path().join("no-such-agent")),
        ..roomy()
    };
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &opts, true);
    assert_eq!(code, EXIT_AGAIN);
    assert!(out.contains("step 3 (seed) failed"), "{out}");
    assert!(out.contains("no-such-agent"), "{out}");
    assert_eq!(state_of(&w), State::Seed);
    assert!(fake.calls.borrow().is_empty());
    let last = super::setup_events::read(&w.layout.golden.events)
        .pop()
        .unwrap();
    assert!(last.text.starts_with("seed failed: "), "{last:?}");
    // The next setup finds the partial, removes it and seeds again.
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("crashed in step 3 (seed)"), "{out}");
}

#[test]
fn a_stopped_guided_stays_guided_and_boots_again() {
    let w = world();
    let mut fake = Fake::new(&w.layout.golden);
    fake.guided = vec![Ok(End::Stopped)];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN);
    assert!(out.contains("stopped in step 4 (guided)"), "{out}");
    assert_eq!(state_of(&w), State::Guided);
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        fake.calls.borrow().first().map(String::as_str),
        Some("guided")
    );
}

#[test]
fn a_failed_configure_stays_and_skip_configure_passes_it() {
    let w = world();
    let mut fake = Fake::new(&w.layout.golden);
    fake.configure = vec![Err("the guest agent did not answer".into())];
    let (code, out) = run_with(&w, &mut fake, &roomy(), true);
    assert_eq!(code, EXIT_AGAIN);
    assert!(
        out.contains("step 6 (configure) failed: the guest agent did not answer"),
        "{out}"
    );
    assert_eq!(state_of(&w), State::Configure);
    let opts = Opts {
        skip_configure: true,
        ..roomy()
    };
    let mut fake = Fake::new(&w.layout.golden);
    let (code, out) = run_with(&w, &mut fake, &opts, true);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("configure skipped"), "{out}");
    assert!(!fake.calls.borrow().iter().any(|c| c == "configure"));
}

#[test]
fn an_unknown_manifest_stops_the_setup_with_the_remove_line() {
    let w = world();
    let g = &w.layout.golden;
    std::fs::create_dir_all(&g.dir).unwrap();
    std::fs::write(&g.manifest, "schema = 9\nstate = \"fetch\"\n").unwrap();
    let mut fake = Fake::new(g);
    let mut out = Vec::new();
    let mut ask = |_q: &str| Ok(true);
    let mut ui = Ui {
        out: &mut out,
        ask: &mut ask,
    };
    let e = run(&w.env, &w.layout, g, &mut fake, &roomy(), &mut ui)
        .unwrap_err()
        .to_string();
    assert!(e.contains("paddock macvm remove"), "{e}");
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn two_setups_do_not_run_at_once() {
    let w = world();
    let _held = lock(&w.layout).unwrap();
    let mut fake = Fake::new(&w.layout.golden);
    let mut out = Vec::new();
    let mut ask = |_q: &str| Ok(true);
    let mut ui = Ui {
        out: &mut out,
        ask: &mut ask,
    };
    let e = run(
        &w.env,
        &w.layout,
        &w.layout.golden,
        &mut fake,
        &roomy(),
        &mut ui,
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("another `paddock macvm setup` runs"), "{e}");
}

#[test]
fn setup_refuses_inside_a_sandbox() {
    let t = tempfile::tempdir().unwrap();
    let mut env = crate::privfs::tests::env_in(t.path());
    env.vars.insert("PADDOCK".into(), "1".into());
    let e = preflight(&env, &roomy()).unwrap_err().to_string();
    assert!(e.contains("host only"), "{e}");
}

#[test]
fn agent_binary_prefers_the_given_path() {
    let w = world();
    let opts = Opts {
        agent: Some(PathBuf::from("/x/paddock-guest")),
        ..roomy()
    };
    assert_eq!(
        agent_binary(&w.layout, &opts),
        Some(PathBuf::from("/x/paddock-guest"))
    );
    let own = w.layout.agent_file(env!("CARGO_PKG_VERSION"));
    std::fs::create_dir_all(own.parent().unwrap()).unwrap();
    std::fs::write(&own, "").unwrap();
    assert_eq!(agent_binary(&w.layout, &roomy()), Some(own));
}

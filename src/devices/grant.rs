//! Who sees a device. A sandbox sees `/dev/paddock/NAME` when the table
//! says `enabled = true` and one of these holds, in this order:
//!
//! 1. `paddock run HARNESS --device NAME`, or `paddock-host spawn ...
//!    --device NAME` from a parent that has it.
//! 2. `[pastures.P] devices = ["NAME"]` and the sandbox is in pasture P.
//! 3. `grant` of the table has `project:PATH` for the project, `harness:H`
//!    for the harness, or `all`. A project `paddock.toml` that says
//!    `[devices] use = ["NAME"]`, approved once, counts as `project:` of
//!    that project.
//! 4. The table has no `grant` key: every sandbox, as in 0.2.1.
//!
//! A spawned child never gets more than its parent (`ceiling`): a device
//! outside the ceiling is off for it, whatever the grants say.
//!
//! A request of another paddock (`paddock device serve-remote`,
//! forward.rs) is a remote caller: its identity is the transport, named
//! in `GrantCtx::remote` (the ssh peer address, or `local` for a socket).
//! Only `remote:HOST` and `remote:any` grant to it: a host name in the
//! grant matches the peer when it resolves to the peer's address. Nothing
//! else grants to a remote caller, not even a table without a `grant`
//! key, and a `remote:` grant never grants to a sandbox of this host.

use super::config::{expand_home, DeviceDef, ParseEnv};
use anyhow::{bail, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One entry of `grant = [...]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "grant", content = "to")]
pub enum Grant {
    /// `pasture:NAME`: the members of that pasture.
    Pasture(String),
    /// `project:PATH`: sandboxes of that project.
    Project(PathBuf),
    /// `harness:NAME`: sandboxes of that harness.
    Harness(String),
    /// `all`: every sandbox, written out.
    All,
    /// `remote:HOST`: the requests of the paddock on HOST (its address as
    /// the transport gives it, or a name that resolves to it); `any` for
    /// every remote paddock.
    Remote(String),
}

const SHAPES: &str = "pasture:NAME, project:PATH, harness:NAME, remote:HOST, all";

/// A host for `remote:HOST`: a name, an address, or `any`.
fn remote_host_ok(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
}

impl Grant {
    /// Parse one entry. `~` in a `project:` path stays as written.
    pub fn parse(s: &str) -> Result<Self> {
        Self::parse_with(s, ParseEnv::default())
    }

    pub fn parse_with(s: &str, env: ParseEnv<'_>) -> Result<Self> {
        if s == "all" {
            return Ok(Grant::All);
        }
        let Some((kind, rest)) = s.split_once(':') else {
            bail!("grant {s:?} is not a grant. The shapes: {SHAPES}");
        };
        match kind {
            "pasture" => {
                crate::pasture::check_name(rest)
                    .map_err(|e| anyhow::anyhow!("grant {s:?}: {e:#}"))?;
                Ok(Grant::Pasture(rest.to_string()))
            }
            "harness" => {
                let ok = !rest.is_empty()
                    && rest.len() <= 64
                    && rest
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                if !ok {
                    bail!("grant {s:?}: {rest:?} is not a harness name");
                }
                Ok(Grant::Harness(rest.to_string()))
            }
            "project" => {
                if rest.is_empty() {
                    bail!("grant {s:?}: project: needs a folder, for example project:~/src/app");
                }
                let p = match env.home {
                    Some(h) => expand_home(rest, h),
                    None => PathBuf::from(rest),
                };
                if !p.is_absolute() && !rest.starts_with('~') {
                    bail!(
                        "grant {s:?}: project: needs an absolute path or one that starts with ~/"
                    );
                }
                Ok(Grant::Project(p))
            }
            "remote" => {
                if !remote_host_ok(rest) {
                    bail!(
                        "grant {s:?}: remote: needs a host name or address, or any; for example remote:linuxbox or remote:any"
                    );
                }
                Ok(Grant::Remote(rest.to_string()))
            }
            _ => bail!("grant {s:?} is not a grant. The shapes: {SHAPES}"),
        }
    }

    /// The entry as the table writes it.
    pub fn render(&self) -> String {
        match self {
            Grant::Pasture(p) => format!("pasture:{p}"),
            Grant::Project(p) => format!("project:{}", p.display()),
            Grant::Harness(h) => format!("harness:{h}"),
            Grant::All => "all".into(),
            Grant::Remote(h) => format!("remote:{h}"),
        }
    }

    /// True when this `remote:` grant covers the peer `peer`: `any`, the
    /// same text, or a name that resolves to the peer's address.
    pub fn covers_remote(&self, peer: &str) -> bool {
        let Grant::Remote(h) = self else {
            return false;
        };
        if h == "any" || h.eq_ignore_ascii_case(peer) {
            return true;
        }
        let Ok(addr) = peer.parse::<std::net::IpAddr>() else {
            return false;
        };
        resolves_to(h, addr)
    }
}

/// True when the host name `h` resolves to `addr` (one lookup, no cache:
/// a request is seconds, a lookup milliseconds).
fn resolves_to(h: &str, addr: std::net::IpAddr) -> bool {
    use std::net::ToSocketAddrs;
    if h.parse::<std::net::IpAddr>().is_ok() {
        // Two spellings of one address.
        return h.parse::<std::net::IpAddr>() == Ok(addr);
    }
    (h, 0u16)
        .to_socket_addrs()
        .map(|mut it| it.any(|a| a.ip() == addr))
        .unwrap_or(false)
}

/// What one sandbox is, for the rule above.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantCtx {
    /// The project of the sandbox.
    pub project: PathBuf,
    /// The repository of a worktree: a `project:` grant for it matches too.
    pub repo: Option<PathBuf>,
    pub harness: String,
    /// The home, to print a `project:` path with `~`.
    pub home: PathBuf,
    /// The pasture that the sandbox joined.
    pub pasture: Option<String>,
    /// `[pastures.P] devices` of that pasture.
    pub pasture_devices: Vec<String>,
    /// `--device` of the run or the spawn.
    pub devices: Vec<String>,
    /// `[devices] use` of the project file, after the one-time approval.
    pub project_use: Vec<String>,
    /// For a spawned sandbox: the devices of its parent. It gets no other.
    pub ceiling: Option<Vec<String>>,
    /// A remote caller (forward.rs): the peer of the transport, for
    /// example `192.168.1.5` or `local`. Only `remote:` grants apply.
    pub remote: Option<String>,
}

impl GrantCtx {
    /// A sandbox without a project, a harness or a pasture: it sees the
    /// devices that every sandbox sees.
    pub fn none() -> Self {
        Self::default()
    }

    /// The context of one run, from what `build` made of it: the project,
    /// the repository of a worktree, the harness, the pasture and its
    /// `devices`, the `--device` flags, the `[devices] use` of the project
    /// file, and the ceiling of a spawned sandbox.
    pub fn for_run(env: &crate::env::HostEnv, built: &crate::build::Built) -> Self {
        let pasture_devices = built
            .pasture
            .as_ref()
            .and_then(|p| {
                let all = crate::pasture::load(env).ok()?;
                all.get(p).map(|x| x.devices.clone())
            })
            .unwrap_or_default();
        Self {
            project: built.spec.project.clone(),
            repo: Some(built.home_key.clone()),
            harness: built.spec.harness.clone(),
            home: env.home.clone(),
            pasture: built.pasture.clone(),
            pasture_devices,
            devices: built.devices.clone(),
            project_use: built.manifest.devices.r#use.clone(),
            ceiling: built.device_ceiling.clone(),
            remote: None,
        }
    }

    /// Only a ceiling, for a child without grants of its own.
    pub fn with_ceiling(ceiling: Option<&[String]>) -> Self {
        Self {
            ceiling: ceiling.map(|c| c.to_vec()),
            ..Default::default()
        }
    }

    /// The requests of the paddock behind `peer` (forward.rs).
    pub fn for_remote(peer: &str) -> Self {
        Self {
            remote: Some(peer.to_string()),
            ..Default::default()
        }
    }
}

/// How a sandbox got a device. The catalog shows it as `grant`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantHit {
    /// `--device NAME`.
    Run,
    Pasture(String),
    /// The `project:` path, with `~` for the home.
    Project(String),
    Harness(String),
    /// `all` in the table.
    All,
    /// No `grant` key in the table.
    NoKey,
    /// `remote:HOST` or `remote:any`, for a remote caller: the peer.
    Remote(String),
}

impl std::fmt::Display for GrantHit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrantHit::Run => write!(f, "--device"),
            GrantHit::Pasture(p) => write!(f, "pasture {p}"),
            GrantHit::Project(p) => write!(f, "project {p}"),
            GrantHit::Harness(h) => write!(f, "harness {h}"),
            GrantHit::All => write!(f, "all"),
            GrantHit::NoKey => write!(f, "all (no grant key)"),
            GrantHit::Remote(p) => write!(f, "remote {p}"),
        }
    }
}

/// Why a sandbox does not get a device that the configuration defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// `enabled = false`, or no `enabled` key.
    Off,
    /// No grant covers this sandbox.
    NoGrant,
    /// A spawned child: its parent does not have the device.
    AboveCeiling,
    /// A remote caller, and no `remote:` grant covers it.
    NotRemote,
    /// A remote caller, and the table mounts the caller's project
    /// (`caller = ["project:ro"]`), which is on another machine.
    NoCallerFiles,
}

/// `path` with `~` for `home`.
pub fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if home.as_os_str().is_empty() => rest.display().to_string(),
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The rule of the module doc: how `ctx` sees `def`, or why not. The
/// `enabled` key and the ceiling come first, then the grants in order.
pub fn status(def: &DeviceDef, ctx: &GrantCtx) -> Result<GrantHit, Why> {
    if !def.enabled {
        return Err(Why::Off);
    }
    if ctx.ceiling.as_ref().is_some_and(|c| !c.contains(&def.name)) {
        return Err(Why::AboveCeiling);
    }
    if let Some(peer) = &ctx.remote {
        let covered = def.grant.iter().flatten().any(|g| g.covers_remote(peer));
        if !covered {
            return Err(Why::NotRemote);
        }
        // The caller's files are on another machine.
        if !def.profile().caller.is_empty() {
            return Err(Why::NoCallerFiles);
        }
        return Ok(GrantHit::Remote(peer.clone()));
    }
    if ctx.devices.contains(&def.name) {
        return Ok(GrantHit::Run);
    }
    if let Some(p) = &ctx.pasture {
        if ctx.pasture_devices.contains(&def.name) {
            return Ok(GrantHit::Pasture(p.clone()));
        }
    }
    let project_match = |p: &Path| {
        same_folder(p, &ctx.project) || ctx.repo.as_deref().is_some_and(|r| same_folder(p, r))
    };
    if let Some(grants) = &def.grant {
        for g in grants {
            match g {
                Grant::Project(p) if project_match(p) => {
                    return Ok(GrantHit::Project(tilde(p, &ctx.home)));
                }
                Grant::Harness(h) if *h == ctx.harness => {
                    return Ok(GrantHit::Harness(h.clone()));
                }
                Grant::All => return Ok(GrantHit::All),
                _ => {}
            }
        }
        if ctx.project_use.contains(&def.name) {
            return Ok(GrantHit::Project(tilde(&ctx.project, &ctx.home)));
        }
        return Err(Why::NoGrant);
    }
    Ok(GrantHit::NoKey)
}

/// `status` as an option: the hit, or None when the sandbox does not get
/// the device.
pub fn visible(def: &DeviceDef, ctx: &GrantCtx) -> Option<GrantHit> {
    status(def, ctx).ok()
}

/// The text of the catalog and the off file for a device that this
/// sandbox does not get.
pub fn how_to_enable(name: &str, why: Why) -> String {
    match why {
        Why::Off => format!(
            "the user sets enabled = true in [devices.{name}] of ~/.config/paddock/config.toml on the host"
        ),
        Why::NoGrant => format!(
            "the user grants it with --device {name}, [pastures.NAME] devices, or grant in [devices.{name}]"
        ),
        Why::AboveCeiling => format!(
            "the sandbox that started this one does not have the device {name}; a child gets at most the devices of its parent"
        ),
        Why::NotRemote => format!(
            "the user on this machine grants it to remote callers with grant = [\"remote:HOST\"] (the address or name of the calling machine) or grant = [\"remote:any\"] in [devices.{name}]"
        ),
        Why::NoCallerFiles => format!(
            "[devices.{name}] mounts the caller's project (caller in its sandbox table), and the files of a remote caller are on another machine; the user removes caller from the table, or the device program copies the files itself"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::DevicesCfg;

    fn def(extra: &str) -> DeviceDef {
        let enabled = if extra.contains("enabled") {
            ""
        } else {
            "enabled = true\n"
        };
        DevicesCfg::parse(&format!(
            "[devices.mac]\nshape = \"exec\"\n{enabled}command = [\"true\"]\n{extra}"
        ))
        .unwrap()
        .user
        .remove(0)
    }

    fn ctx() -> GrantCtx {
        GrantCtx {
            project: PathBuf::from("/home/demo/src/x"),
            repo: None,
            harness: "claude".into(),
            home: PathBuf::from("/home/demo"),
            ..Default::default()
        }
    }

    #[test]
    fn grant_entries_parse_or_name_the_four_shapes() {
        assert_eq!(
            Grant::parse("pasture:workers").unwrap(),
            Grant::Pasture("workers".into())
        );
        assert_eq!(
            Grant::parse("harness:claude").unwrap(),
            Grant::Harness("claude".into())
        );
        assert_eq!(
            Grant::parse("project:/src/x").unwrap(),
            Grant::Project("/src/x".into())
        );
        assert_eq!(Grant::parse("all").unwrap(), Grant::All);
        let home = Path::new("/home/demo");
        assert_eq!(
            Grant::parse_with(
                "project:~/src/x",
                ParseEnv {
                    home: Some(home),
                    ..Default::default()
                }
            )
            .unwrap(),
            Grant::Project("/home/demo/src/x".into())
        );
        for bad in [
            "workers",
            "pasture:",
            "pasture:Bad",
            "user:me",
            "project:src/x",
            "harness:a b",
        ] {
            let e = format!("{:#}", Grant::parse(bad).unwrap_err());
            assert!(e.contains(&format!("grant {bad:?}")), "{bad}: {e}");
        }
        let e = format!("{:#}", Grant::parse("user:me").unwrap_err());
        assert_eq!(
            e,
            "grant \"user:me\" is not a grant. The shapes: pasture:NAME, project:PATH, harness:NAME, remote:HOST, all"
        );
        for g in [
            "pasture:workers",
            "project:/src/x",
            "harness:claude",
            "all",
            "remote:any",
            "remote:mac.local",
        ] {
            assert_eq!(Grant::parse(g).unwrap().render(), g);
        }
        for bad in ["remote:", "remote:a b", "remote:x/y"] {
            let e = format!("{:#}", Grant::parse(bad).unwrap_err());
            assert!(e.contains("remote: needs a host name"), "{bad}: {e}");
        }
    }

    #[test]
    fn a_remote_caller_gets_only_remote_grants_and_a_sandbox_never_does() {
        // A remote caller: nothing but `remote:` covers it, not even a
        // table without a grant key, `all`, or `--device`.
        let any = def("grant = [\"remote:any\"]\n");
        let mut c = GrantCtx::for_remote("192.168.1.5");
        assert_eq!(status(&any, &c), Ok(GrantHit::Remote("192.168.1.5".into())));
        assert_eq!(
            GrantHit::Remote("192.168.1.5".into()).to_string(),
            "remote 192.168.1.5"
        );
        for d in [
            def(""),
            def("grant = [\"all\"]\n"),
            def("grant = [\"harness:claude\"]\n"),
            def("grant = [\"remote:10.0.0.9\"]\n"),
        ] {
            assert_eq!(status(&d, &c), Err(Why::NotRemote), "{:?}", d.grant);
        }
        c.devices = vec!["mac".into()];
        assert_eq!(status(&def(""), &c), Err(Why::NotRemote));
        // The peer by its address, or by a name that resolves to it.
        let same = def("grant = [\"remote:192.168.1.5\"]\n");
        assert!(status(&same, &c).is_ok());
        let v6 = def("grant = [\"remote:::1\"]\n");
        assert!(status(&v6, &GrantCtx::for_remote("::1")).is_ok());
        let lo = def("grant = [\"remote:localhost\"]\n");
        assert!(
            status(&lo, &GrantCtx::for_remote("127.0.0.1")).is_ok(),
            "localhost resolves to 127.0.0.1"
        );
        assert_eq!(
            status(&lo, &GrantCtx::for_remote("local")),
            Err(Why::NotRemote),
            "a socket peer is `local`, not a name"
        );
        let local = def("grant = [\"remote:local\"]\n");
        assert!(status(&local, &GrantCtx::for_remote("local")).is_ok());
        // A sandbox of this host never gets a device through a remote grant.
        assert_eq!(status(&any, &ctx()), Err(Why::NoGrant));
        let off = def("enabled = false\ngrant = [\"remote:any\"]\n");
        assert_eq!(status(&off, &GrantCtx::for_remote("local")), Err(Why::Off));
        assert!(how_to_enable("mac", Why::NotRemote).contains("remote:HOST"));
        assert!(how_to_enable("mac", Why::NotRemote).contains("remote:any"));
        // The caller's files are on another machine: a table that mounts
        // them is off for a remote caller, with the reason.
        let files =
            def("grant = [\"remote:any\"]\n[devices.mac.sandbox]\ncaller = [\"project:ro\"]\n");
        assert_eq!(
            status(&files, &GrantCtx::for_remote("local")),
            Err(Why::NoCallerFiles)
        );
        assert_eq!(status(&files, &ctx()), Err(Why::NoGrant));
        assert!(how_to_enable("mac", Why::NoCallerFiles).contains("on another machine"));
    }

    #[test]
    fn a_pasture_grant_needs_the_pasture_and_the_run_flag_wins() {
        let d = def("grant = [\"pasture:workers\"]\n");
        let mut c = ctx();
        assert_eq!(status(&d, &c), Err(Why::NoGrant));
        c.pasture = Some("workers".into());
        assert_eq!(
            status(&d, &c),
            Err(Why::NoGrant),
            "the pasture must list it"
        );
        c.pasture_devices = vec!["mac".into()];
        assert_eq!(status(&d, &c), Ok(GrantHit::Pasture("workers".into())));
        c.devices = vec!["mac".into()];
        assert_eq!(status(&d, &c), Ok(GrantHit::Run));
        assert_eq!(GrantHit::Run.to_string(), "--device");
        assert_eq!(
            GrantHit::Pasture("workers".into()).to_string(),
            "pasture workers"
        );
    }

    #[test]
    fn a_project_grant_matches_the_canonical_path_and_the_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let d = def(&format!("grant = [\"project:{}\"]\n", link.display()));
        let mut c = ctx();
        c.project = real.clone();
        assert!(matches!(status(&d, &c), Ok(GrantHit::Project(_))));
        c.project = tmp.path().join("other");
        assert_eq!(status(&d, &c), Err(Why::NoGrant));
        c.repo = Some(real.clone());
        assert!(matches!(status(&d, &c), Ok(GrantHit::Project(_))));
        let d = def("grant = [\"project:/home/demo/src/x\"]\n");
        assert_eq!(status(&d, &ctx()), Ok(GrantHit::Project("~/src/x".into())));
        assert_eq!(
            GrantHit::Project("~/src/x".into()).to_string(),
            "project ~/src/x"
        );
    }

    #[test]
    fn harness_all_and_no_key() {
        let d = def("grant = [\"harness:claude\"]\n");
        assert_eq!(status(&d, &ctx()), Ok(GrantHit::Harness("claude".into())));
        let mut c = ctx();
        c.harness = "codex".into();
        assert_eq!(status(&d, &c), Err(Why::NoGrant));
        let d = def("grant = [\"all\"]\n");
        assert_eq!(status(&d, &c), Ok(GrantHit::All));
        let d = def("");
        assert_eq!(status(&d, &c), Ok(GrantHit::NoKey));
        assert_eq!(GrantHit::NoKey.to_string(), "all (no grant key)");
        assert_eq!(status(&d, &GrantCtx::none()), Ok(GrantHit::NoKey));
        let off = def("enabled = false\n");
        assert_eq!(status(&off, &c), Err(Why::Off));
    }

    #[test]
    fn the_project_file_counts_as_a_project_grant_only_for_that_project() {
        let d = def("grant = [\"pasture:workers\"]\n");
        let mut c = ctx();
        c.project_use = vec!["mac".into()];
        assert_eq!(status(&d, &c), Ok(GrantHit::Project("~/src/x".into())));
        // It never turns a device on.
        let off = def("enabled = false\ngrant = [\"pasture:workers\"]\n");
        assert_eq!(status(&off, &c), Err(Why::Off));
    }

    #[test]
    fn the_ceiling_of_a_child_comes_before_every_grant() {
        let d = def("");
        let mut c = ctx();
        c.ceiling = Some(vec![]);
        c.devices = vec!["mac".into()];
        assert_eq!(status(&d, &c), Err(Why::AboveCeiling));
        c.ceiling = Some(vec!["mac".into()]);
        assert_eq!(status(&d, &c), Ok(GrantHit::Run));
        assert!(
            how_to_enable("mac", Why::AboveCeiling).contains("at most the devices of its parent")
        );
        assert_eq!(
            how_to_enable("mac", Why::NoGrant),
            "the user grants it with --device mac, [pastures.NAME] devices, or grant in [devices.mac]"
        );
    }
}

//! The plain logic of the `paddock-vm` helper: the arguments, the state
//! folder, the machine identifier file and the checks before a boot. It
//! runs on every system, so the tests run everywhere. The Virtualization
//! framework code is in `vz.rs`.

use super::{assets, ports};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The size of the writable disk of the guest, created sparse.
pub const STATE_DISK_BYTES: u64 = 2 << 30;
/// The virtiofs tag of the project folder.
pub const PROJECT_TAG: &str = "project";
/// The longest machine identifier file that the helper reads.
const MACHINE_ID_MAX: u64 = 4096;

#[derive(Debug, Parser)]
#[command(
    name = "paddock-vm",
    version,
    about = "Run the Linux VM of a paddock sandbox (macOS)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Boot the VM and run until the guest stops or the helper gets SIGTERM.
    Start(StartArgs),
    /// Build the golden macOS guest of app mode. `paddock macvm setup`
    /// starts this: the Virtualization framework needs the entitlement
    /// of this binary.
    #[command(name = "macvm-setup")]
    MacvmSetup(MacvmSetupArgs),
    /// Run one app session in the macOS guest: the profile, its clone of
    /// the golden, cold or warm start, pause, save and stop.
    App(AppArgs),
    /// What a launcher bundle of `paddock app install` does when Finder
    /// starts it: run one app profile, or, as the drag target, make a
    /// profile of each bundle dropped on it.
    Install(InstallArgs),
}

/// The arguments of `paddock-vm install`. The bundle of
/// `paddock app install` carries them in its target file, and
/// `shim::argv_from_bundle` turns them into this argv.
#[derive(Debug, Clone, clap::Args)]
pub struct InstallArgs {
    /// The app profile to run.
    #[arg(long)]
    pub app: Option<String>,
    /// The drag target: each path becomes an app profile and runs. With
    /// no path it waits for the bundles that Finder sends.
    #[arg(long)]
    pub drop_target: bool,
    /// App bundles to add, for a drop that arrives as an argument.
    pub paths: Vec<PathBuf>,
    /// The instance of the app profile, 1 by default.
    #[arg(long, requires = "app")]
    pub instance: Option<u32>,
    /// A suspended session resumes after a click on its Dock icon: the
    /// event log names that reason.
    #[arg(long, requires = "app", hide = true)]
    pub resume_from_dock: bool,
}

#[derive(Debug, Clone, clap::Args)]
pub struct MacvmSetupArgs {
    /// The `macvm` folder of paddock's data.
    #[arg(long)]
    pub dir: PathBuf,
    /// A restore image file instead of the download.
    #[arg(long, value_name = "PATH")]
    pub ipsw: Option<PathBuf>,
    /// No question before the download.
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// The paddock-guest binary that the seed copies into the golden.
    #[arg(long, value_name = "PATH")]
    pub agent: Option<PathBuf>,
    /// Build into golden.new (`paddock macvm update`).
    #[arg(long)]
    pub new: bool,
    /// With --new: build even when the image is the version of the
    /// current golden.
    #[arg(long)]
    pub force: bool,
}

/// A host folder shared into the macOS guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareArg {
    /// The name under the automount folder of the guest.
    pub name: String,
    pub path: PathBuf,
    pub read_only: bool,
}

/// `NAME=PATH` or `NAME=PATH:ro`.
pub fn parse_share(text: &str) -> Result<ShareArg, String> {
    let (name, rest) = text
        .split_once('=')
        .ok_or_else(|| format!("{text:?} is not NAME=PATH[:ro]"))?;
    let (path, read_only) = match rest.strip_suffix(":ro") {
        Some(p) => (p, true),
        None => (rest.strip_suffix(":rw").unwrap_or(rest), false),
    };
    let ok_name = !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    if !ok_name {
        return Err(format!(
            "the share name {name:?} must be letters, digits, dots, dashes or underscores, at most 64"
        ));
    }
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(format!(
            "the share {} is not an absolute path",
            path.display()
        ));
    }
    Ok(ShareArg {
        name: name.to_string(),
        path,
        read_only,
    })
}

/// `--forward` of an app session: only the guest proxy port.
fn parse_app_forward(text: &str) -> Result<(u32, PathBuf), String> {
    let (port, path) = parse_forward(text)?;
    if port != ports::GUEST_PROXY {
        return Err(format!(
            "an app session forwards only the guest-proxy port ({}); the other ports stay inside the helper",
            ports::GUEST_PROXY
        ));
    }
    Ok((port, path))
}

/// The arguments of `paddock-vm app`. `paddock app run` makes them.
#[derive(Debug, Clone, clap::Args)]
pub struct AppArgs {
    /// The profile folder: the clone of the golden, the data disk, the
    /// saved state and the stamp.
    #[arg(long)]
    pub profile: PathBuf,
    /// The sealed golden folder.
    #[arg(long)]
    pub golden: PathBuf,
    /// The name of the app, for the lines and the slot text.
    #[arg(long)]
    pub app: String,
    /// The file descriptor with the session token on its first line, and
    /// the guest password on the second, when there is one.
    #[arg(long, value_name = "FD")]
    pub token_fd: i32,
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u32).range(1..=64))]
    pub cpus: u32,
    /// The memory of the VM, in MiB.
    #[arg(long, default_value_t = 4096, value_parser = clap::value_parser!(u64).range(1024..=262144))]
    pub memory: u64,
    /// The display of the guest: pixels and scale, `WxH@S`.
    #[arg(long, value_name = "WxH@S")]
    pub display: crate::macvm::session::Display,
    /// A host folder for the guest: `NAME=PATH` read-write, `NAME=PATH:ro`
    /// read-only. It appears under the automount folder of the guest.
    #[arg(long = "share", value_name = "NAME=PATH[:ro]", value_parser = parse_share)]
    pub shares: Vec<ShareArg>,
    /// Send guest connections to the guest-proxy port to this unix
    /// socket instead of the proxy inside the helper.
    #[arg(long = "forward", value_name = "PORT=SOCKET", value_parser = parse_app_forward)]
    pub forwards: Vec<(u32, PathBuf)>,
    /// The control socket for the parent paddock. A unix socket path on
    /// macOS has at most 103 bytes.
    #[arg(long, value_name = "PATH")]
    pub control: Option<PathBuf>,
    /// Do not save the machine state at quit: the next start is a cold
    /// boot.
    #[arg(long = "no-save", action = clap::ArgAction::SetFalse)]
    pub save: bool,
    /// Seconds between the stop request to the guest and the hard stop.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=600))]
    pub stop_timeout: u64,
    /// The host input source at the start, restored at the end.
    #[arg(long, default_value = "")]
    pub input_source: String,
    /// The session record that the helper removes when the session ends.
    #[arg(long, value_name = "PATH")]
    pub record: Option<PathBuf>,
    /// The folder with the guest agent of this paddock, shared read-only
    /// as `agent`.
    #[arg(long, value_name = "DIR")]
    pub agent: Option<PathBuf>,
    /// The clone of the host app bundle in the profile, which the session
    /// copies into the guest once per version. Without
    /// it the session starts no app.
    #[arg(long, value_name = "PATH")]
    pub app_bundle: Option<PathBuf>,
    /// The guest may remove `com.apple.quarantine` from the copy: the
    /// host bundle carried the user's approval.
    #[arg(long)]
    pub app_unquarantine: bool,
    /// An argument for the app, for example a workspace folder of an
    /// editor (repeatable). It is the guest path.
    #[arg(long = "app-arg", value_name = "VALUE")]
    pub app_args: Vec<String>,
    /// The network mode of the session, for the lines and the firewall.
    #[arg(long, default_value = "ask")]
    pub network: String,
    /// A host of the list of network mode api or allowlist (repeatable):
    /// the endpoints of the profile and `[network] allow` of the user.
    #[arg(long = "allow", value_name = "HOST")]
    pub allow: Vec<String>,
    /// The guest honors the bench ops of `paddock macvm bench`.
    #[arg(long)]
    pub bench: bool,
}

/// Everything an app session needs, checked.
#[derive(Debug, Clone)]
pub struct AppPlan {
    pub profile: crate::macvm::session::Profile,
    pub golden: crate::macvm::layout::Golden,
    pub app: String,
    pub token_fd: i32,
    pub cpus: u32,
    pub memory_bytes: u64,
    pub display: crate::macvm::session::Display,
    pub shares: Vec<ShareArg>,
    pub proxy_socket: Option<PathBuf>,
    pub control: PathBuf,
    pub save: bool,
    pub bench: bool,
    pub stop_timeout: Duration,
    pub input_source: String,
    pub record: Option<PathBuf>,
    /// The clone of the host app bundle in the profile, when the session
    /// runs an app.
    pub app_bundle: Option<PathBuf>,
    pub app_unquarantine: bool,
    pub app_args: Vec<String>,
    pub network: String,
    /// The host list of mode api or allowlist.
    pub allow: Vec<String>,
}

/// Check the arguments of an app session. It touches no folder.
pub fn plan_app(args: &AppArgs) -> Result<AppPlan> {
    use crate::macvm::layout::Golden;
    use crate::macvm::session::Profile;
    if args.app.is_empty()
        || args.app.len() > 64
        || !args
            .app
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        bail!(
            "the app name {:?} must be letters, digits, dots, dashes or underscores",
            args.app
        );
    }
    let profile = Profile::new(absolute(&args.profile)?);
    let golden_dir = absolute(&args.golden)?;
    if !golden_dir.is_dir() {
        bail!("the golden folder {} is missing", golden_dir.display());
    }
    if profile.dir.starts_with(&golden_dir) || golden_dir.starts_with(&profile.dir) {
        bail!(
            "the profile {} and the golden {} must not contain each other",
            profile.dir.display(),
            golden_dir.display()
        );
    }
    if args.token_fd < 0 {
        bail!("--token-fd {} is not a file descriptor", args.token_fd);
    }
    let control = match &args.control {
        Some(c) => {
            if !c.is_absolute() {
                bail!("the control socket {} is not an absolute path", c.display());
            }
            c.clone()
        }
        None => profile.dir.join("control.sock"),
    };
    crate::sockpath::check(&control)?;
    let mut shares = args.shares.clone();
    if let Some(agent) = &args.agent {
        shares.push(ShareArg {
            name: "agent".into(),
            path: absolute(agent)?,
            read_only: true,
        });
    }
    let mut names = std::collections::BTreeSet::new();
    for s in &shares {
        if !names.insert(s.name.clone()) {
            bail!("two shares are named {}", s.name);
        }
        // The profile's own shares (macvm/shares.rs): drop, downloads and
        // the read-only app copies. Any other folder of the profile is
        // private to the host.
        if s.path.starts_with(&profile.dir)
            && s.path != profile.downloads()
            && s.path != profile.drop_dir()
            && s.path != profile.apps()
        {
            bail!(
                "the share {} is inside the profile folder",
                s.path.display()
            );
        }
    }
    let mut proxy_socket = None;
    for (port, path) in &args.forwards {
        if *port == ports::GUEST_PROXY && proxy_socket.replace(path.clone()).is_some() {
            bail!("port {port} has two --forward sockets");
        }
    }
    Ok(AppPlan {
        profile,
        golden: Golden::new(golden_dir),
        app: args.app.clone(),
        token_fd: args.token_fd,
        cpus: args.cpus,
        memory_bytes: args.memory << 20,
        display: args.display,
        shares,
        proxy_socket,
        control,
        save: args.save,
        bench: args.bench,
        stop_timeout: Duration::from_secs(args.stop_timeout),
        input_source: args.input_source.clone(),
        record: args.record.clone(),
        app_bundle: match &args.app_bundle {
            Some(b) => Some(absolute(b)?),
            None => None,
        },
        app_unquarantine: args.app_unquarantine,
        app_args: args.app_args.clone(),
        network: args.network.clone(),
        allow: args.allow.clone(),
    })
}

#[derive(Debug, Clone, clap::Args)]
pub struct StartArgs {
    /// The folder with the guest files and assets.toml.
    #[arg(long)]
    pub assets: PathBuf,
    /// The project folder. It is the only host folder in the VM.
    #[arg(long)]
    pub project: PathBuf,
    /// The folder for the machine identifier, the guest disk and the
    /// console log.
    #[arg(long)]
    pub state: PathBuf,
    /// The control socket for the parent paddock. The default is
    /// `control.sock` in the state folder. paddock passes a short path:
    /// a unix socket path on macOS has at most 103 bytes.
    #[arg(long, value_name = "PATH")]
    pub control: Option<PathBuf>,
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..=64))]
    pub cpus: u32,
    /// The memory of the VM, in MiB.
    #[arg(long, default_value_t = 2048, value_parser = clap::value_parser!(u64).range(256..=262144))]
    pub memory: u64,
    /// Send guest connections to a host vsock port to a unix socket:
    /// `PORT=SOCKET`, for example `1025=/tmp/broker.sock` or `broker=...`.
    #[arg(long = "forward", value_name = "PORT=SOCKET", value_parser = parse_forward)]
    pub forwards: Vec<(u32, PathBuf)>,
    /// Seconds between the stop request to the guest and the hard stop.
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=600))]
    pub stop_timeout: u64,
    /// One more argument for the kernel command line.
    #[arg(long, value_name = "ARG", allow_hyphen_values = true)]
    pub append: Vec<String>,
}

fn parse_forward(text: &str) -> Result<(u32, PathBuf), String> {
    let (port, path) = text
        .split_once('=')
        .ok_or_else(|| format!("{text:?} is not PORT=SOCKET"))?;
    let port = ports::parse(port).ok_or_else(|| {
        let known: Vec<String> = ports::ALL
            .iter()
            .map(|(p, n)| format!("{p} ({n})"))
            .collect();
        format!("{port:?} is not a paddock VM port: {}", known.join(", "))
    })?;
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(format!(
            "the socket {} is not an absolute path",
            path.display()
        ));
    }
    crate::sockpath::check(&path).map_err(|e| e.to_string())?;
    Ok((port, path))
}

/// The files of the state folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateLayout {
    pub dir: PathBuf,
    /// The `VZGenericMachineIdentifier` of the VM, as its data.
    pub machine_id: PathBuf,
    /// The writable disk of the guest, raw and sparse.
    pub disk: PathBuf,
    /// The serial console of the guest.
    pub console: PathBuf,
    /// The control socket for the parent paddock.
    pub control: PathBuf,
}

impl StateLayout {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            machine_id: dir.join("machine-id"),
            disk: dir.join("disk.img"),
            console: dir.join("console.log"),
            control: dir.join("control.sock"),
        }
    }

    /// Create the folder with mode 0700, or check an existing one.
    pub fn prepare(&self) -> Result<()> {
        match std::fs::symlink_metadata(&self.dir) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => bail!("the state folder {} is not a folder", self.dir.display()),
            Err(_) => std::fs::create_dir_all(&self.dir)
                .with_context(|| format!("cannot create {}", self.dir.display()))?,
        }
        std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("cannot set the mode of {}", self.dir.display()))?;
        crate::sockpath::check(&self.control)
    }

    /// Create the guest disk on the first start, sparse. True when it is new.
    pub fn ensure_disk(&self, bytes: u64) -> Result<bool> {
        if let Ok(m) = std::fs::symlink_metadata(&self.disk) {
            if !m.is_file() {
                bail!(
                    "the guest disk {} is not a regular file",
                    self.disk.display()
                );
            }
            return Ok(false);
        }
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.disk)
            .with_context(|| format!("cannot create {}", self.disk.display()))?;
        f.set_len(bytes)
            .with_context(|| format!("cannot size {}", self.disk.display()))?;
        Ok(true)
    }

    /// The saved machine identifier, or `None` before the first start.
    pub fn read_machine_id(&self) -> Result<Option<Vec<u8>>> {
        let path = &self.machine_id;
        let m = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        if !m.is_file() || m.len() == 0 || m.len() > MACHINE_ID_MAX {
            bail!(
                "the machine identifier {} is damaged: remove the file to make a new one",
                path.display()
            );
        }
        let data =
            std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        Ok(Some(data))
    }

    /// Save a new machine identifier. An existing file stays as it is.
    pub fn write_machine_id(&self, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() as u64 > MACHINE_ID_MAX {
            bail!("a machine identifier of {} bytes", data.len());
        }
        let path = &self.machine_id;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("cannot create {}", path.display()))?;
        f.write_all(data)
            .and_then(|_| f.sync_all())
            .with_context(|| format!("cannot write {}", path.display()))
    }
}

/// The project folder as an absolute, real path. Refuse the home folder and
/// any folder that contains it: the home must never be in the VM.
pub fn check_project(project: &Path, home: &Path) -> Result<PathBuf> {
    let real = project
        .canonicalize()
        .with_context(|| format!("the project folder {} is missing", project.display()))?;
    if !real.is_dir() {
        bail!("the project {} is not a folder", real.display());
    }
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    if home.starts_with(&real) {
        bail!(
            "the project folder {} is the home folder or contains it: the VM never gets the home",
            real.display()
        );
    }
    Ok(real)
}

/// The init of the guest image: `packaging/vm/agent-init`. It starts the
/// guest agent.
pub const GUEST_INIT: &str = "/sbin/paddock-init";

/// The kernel command line: the console, the read-only root disk, the init
/// of the guest image, and the extra arguments. A later `init=` in the
/// extra arguments wins over the first one.
pub fn kernel_cmdline(extra: &[String]) -> Result<String> {
    let mut args = vec![
        "console=hvc0".to_string(),
        "root=/dev/vda".into(),
        "rootfstype=ext4".into(),
        "ro".into(),
        format!("init={GUEST_INIT}"),
    ];
    for a in extra {
        if a.is_empty() || a.chars().any(|c| c.is_whitespace() || c.is_control()) {
            bail!("the kernel argument {a:?} is empty or has a space or a control character");
        }
        args.push(a.clone());
    }
    Ok(args.join(" "))
}

/// Everything a boot needs, checked.
#[derive(Debug, Clone)]
pub struct Plan {
    pub assets: assets::Assets,
    pub project: PathBuf,
    pub state: StateLayout,
    pub cpus: u32,
    pub memory_bytes: u64,
    pub forwards: BTreeMap<u32, PathBuf>,
    pub cmdline: String,
    pub stop_timeout: Duration,
}

/// Check the arguments and the files before a boot. It does not touch the
/// state folder: `StateLayout::prepare` does that.
pub fn plan(args: &StartArgs, home: &Path) -> Result<Plan> {
    let project = check_project(&args.project, home)?;
    let assets = assets::check(&args.assets)?;
    let state_dir = absolute(&args.state)?;
    // The guest writes the project. It must not reach the files that the
    // helper trusts.
    for (what, path) in [("state", &state_dir), ("assets", &absolute(&args.assets)?)] {
        if path.starts_with(&project) || project.starts_with(path) {
            bail!(
                "the {what} folder {} and the project {} must not contain each other",
                path.display(),
                project.display()
            );
        }
    }
    let mut state = StateLayout::new(&state_dir);
    if let Some(c) = &args.control {
        if !c.is_absolute() {
            bail!("the control socket {} is not an absolute path", c.display());
        }
        crate::sockpath::check(c)?;
        state.control = c.clone();
    }
    let mut forwards = BTreeMap::new();
    for (port, path) in &args.forwards {
        if forwards.insert(*port, path.clone()).is_some() {
            bail!("port {port} has two --forward sockets");
        }
    }
    Ok(Plan {
        assets,
        project,
        state,
        cpus: args.cpus,
        memory_bytes: args.memory << 20,
        forwards,
        cmdline: kernel_cmdline(&args.append)?,
        stop_timeout: Duration::from_secs(args.stop_timeout),
    })
}

/// An absolute path, with the real path of its parent when it exists.
fn absolute(path: &Path) -> Result<PathBuf> {
    let abs =
        std::path::absolute(path).with_context(|| format!("cannot resolve {}", path.display()))?;
    if let Ok(real) = abs.canonicalize() {
        return Ok(real);
    }
    match (abs.parent(), abs.file_name()) {
        (Some(parent), Some(name)) => Ok(parent
            .canonicalize()
            .map(|p| p.join(name))
            .unwrap_or(abs.clone())),
        _ => Ok(abs),
    }
}

#[cfg(test)]
#[path = "helper_tests.rs"]
mod tests;

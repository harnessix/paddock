//! The guest disk as a plain user sees it on the host, platform-neutral:
//! attach the raw image with `hdiutil`, mount its volumes without owners,
//! find the Data volume, detach, and check the container with
//! `fsck_apfs -n`. The offline seed and the offline trust of `paddock
//! macvm setup` work through the [`DiskTool`] trait, so the tests run
//! them on Linux with a folder that stands in for the mount.
//!
//! [`Hdiutil`] is the tool of a Mac: it runs the commands and reads their
//! property-list output with the small reader of this file. The argv
//! builders and the readers have Linux tests with recorded output.

use super::text;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// An attached and mounted guest disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attached {
    /// The whole disk, for the detach: `/dev/disk4`.
    pub disk: String,
    /// The mount point of the Data volume, writable, without owners.
    pub data: PathBuf,
}

/// What `fsck_apfs -n` said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fsck {
    Clean,
    /// The check could not run as a plain user: the reason.
    Skipped(String),
}

/// The disk operations of the offline seed and trust.
pub trait DiskTool {
    /// Attach `image` and mount its volumes without owners under `root`.
    fn attach(&mut self, image: &Path, root: &Path) -> Result<Attached>;
    /// Detach a disk of [`attach`](DiskTool::attach).
    fn detach(&mut self, a: &Attached) -> Result<()>;
    /// Attach without a mount, check the APFS container read-only, detach.
    /// `Err` when the check found damage or the attach failed.
    fn fsck(&mut self, image: &Path) -> Result<Fsck>;
    /// A copy of `from` at `to` that costs no space: an APFS clone. `Err`
    /// when the volume cannot clone.
    fn clone_file(&mut self, from: &Path, to: &Path) -> Result<()>;
    /// Detach whatever a crashed run left mounted under `root`.
    fn detach_stale(&mut self, _root: &Path) {}
}

/// Remove the mount folder `root` and its empty mount points, never
/// recursively: a volume that is still mounted there (a failed detach)
/// keeps every file, and its folder stays.
pub fn remove_mount_root(root: &Path) {
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let _ = std::fs::remove_dir(e.path());
        }
    }
    let _ = std::fs::remove_dir(root);
}

/// The folders under `root` that are mount points: another device than
/// `root` itself.
pub fn mounts_under(root: &Path) -> Vec<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let Ok(dev) = std::fs::metadata(root).map(|m| m.dev()) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_dir() && m.dev() != dev))
        .collect()
}

/// A value of an XML property list.
#[derive(Debug, Clone, PartialEq)]
pub enum Plist {
    Dict(Vec<(String, Plist)>),
    Array(Vec<Plist>),
    String(String),
    Integer(i64),
    Bool(bool),
    /// `<data>`, `<date>`, `<real>`: kept as their text.
    Other(String),
}

impl Plist {
    pub fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Plist::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Plist::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn array(&self) -> &[Plist] {
        match self {
            Plist::Array(a) => a,
            _ => &[],
        }
    }
}

/// Read an XML property list: the text from `<plist>` on. Errors name
/// what was wrong; the output of a tool that printed something else is
/// an error, never a panic.
pub fn parse_plist(xml: &str) -> Result<Plist> {
    let start = xml
        .find("<plist")
        .and_then(|i| xml[i..].find('>').map(|j| i + j + 1))
        .ok_or_else(|| anyhow!(text::plist_unreadable("no <plist>")))?;
    let mut p = Reader {
        s: &xml[start..],
        depth: 0,
    };
    p.value()
}

struct Reader<'a> {
    s: &'a str,
    depth: usize,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

impl Reader<'_> {
    /// The next tag, after any text and comments: its name with a leading
    /// `/` for an end tag and a trailing `/` for an empty one.
    fn tag(&mut self) -> Result<String> {
        loop {
            let i = self
                .s
                .find('<')
                .ok_or_else(|| anyhow!(text::plist_unreadable("the input ended early")))?;
            let rest = &self.s[i..];
            if let Some(after) = rest.strip_prefix("<!--") {
                let end = after
                    .find("-->")
                    .ok_or_else(|| anyhow!(text::plist_unreadable("an open comment")))?;
                self.s = &after[end + 3..];
                continue;
            }
            let j = rest
                .find('>')
                .ok_or_else(|| anyhow!(text::plist_unreadable("an open tag")))?;
            let name = rest[1..j].trim().to_string();
            self.s = &rest[j + 1..];
            if name.starts_with('?') || name.starts_with('!') {
                continue;
            }
            return Ok(name);
        }
    }

    /// The text up to the end tag `</name>`.
    fn text(&mut self, name: &str) -> Result<String> {
        let end = format!("</{name}>"); // not user text
        let i = self
            .s
            .find(&end)
            .ok_or_else(|| anyhow!(text::plist_unreadable(name)))?;
        let t = unescape(&self.s[..i]);
        self.s = &self.s[i + end.len()..];
        Ok(t)
    }

    fn value(&mut self) -> Result<Plist> {
        let tag = self.tag()?;
        self.value_of(&tag)
    }

    fn value_of(&mut self, tag: &str) -> Result<Plist> {
        self.depth += 1;
        if self.depth > 64 {
            return Err(anyhow!(text::plist_unreadable("nested too deep")));
        }
        let v = match tag {
            "dict" => {
                let mut d = Vec::new();
                loop {
                    let t = self.tag()?;
                    if t == "/dict" {
                        break;
                    }
                    if t != "key" {
                        return Err(anyhow!(text::plist_unreadable(
                            "a dict entry without a key"
                        )));
                    }
                    let k = self.text("key")?;
                    let v = self.value()?;
                    d.push((k, v));
                }
                Plist::Dict(d)
            }
            "dict/" => Plist::Dict(Vec::new()),
            "array" => {
                let mut a = Vec::new();
                loop {
                    let t = self.tag()?;
                    if t == "/array" {
                        break;
                    }
                    a.push(self.value_of(&t)?);
                }
                Plist::Array(a)
            }
            "array/" => Plist::Array(Vec::new()),
            "string" => Plist::String(self.text("string")?),
            "string/" => Plist::String(String::new()),
            "integer" => {
                let t = self.text("integer")?;
                Plist::Integer(
                    t.trim()
                        .parse()
                        .map_err(|_| anyhow!(text::plist_unreadable("an integer")))?,
                )
            }
            "true/" => Plist::Bool(true),
            "false/" => Plist::Bool(false),
            "data" | "date" | "real" => Plist::Other(self.text(tag)?),
            other => return Err(anyhow!(text::plist_unreadable(other))),
        };
        self.depth -= 1;
        Ok(v)
    }
}

/// The devices of `hdiutil attach -plist`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entities {
    /// The whole disk (`GUID_partition_scheme`).
    pub disk: Option<String>,
    /// The APFS physical store (`Apple_APFS`), for `fsck_apfs`.
    pub store: Option<String>,
    /// Each volume: its device and its mount point, when mounted.
    pub volumes: Vec<(String, Option<PathBuf>)>,
}

/// The type GUID of an APFS volume as `hdiutil` names it.
const APFS_VOLUME_HINT: &str = "41504653-0000-11AA-AA11-00306543ECAC";

/// Read the output of `hdiutil attach -plist`.
pub fn entities(xml: &str) -> Result<Entities> {
    let p = parse_plist(xml)?;
    let list = p
        .get("system-entities")
        .ok_or_else(|| anyhow!(text::plist_unreadable("no system-entities")))?;
    let mut e = Entities::default();
    for ent in list.array() {
        let Some(dev) = ent.get("dev-entry").and_then(Plist::str) else {
            continue;
        };
        let hint = ent.get("content-hint").and_then(Plist::str).unwrap_or("");
        let mount = ent
            .get("mount-point")
            .and_then(Plist::str)
            .map(PathBuf::from);
        match hint {
            "GUID_partition_scheme" => e.disk = Some(dev.to_string()),
            "Apple_APFS" => e.store = Some(dev.to_string()),
            _ => {}
        }
        let apfs_volume = hint == APFS_VOLUME_HINT
            || hint == "Apple_APFS_Volume"
            || ent.get("volume-kind").and_then(Plist::str) == Some("apfs");
        if apfs_volume {
            e.volumes.push((dev.to_string(), mount));
        }
    }
    Ok(e)
}

/// The devices whose role is Data, from `diskutil apfs list -plist`.
pub fn data_devices(xml: &str) -> Result<Vec<String>> {
    let p = parse_plist(xml)?;
    let mut out = Vec::new();
    for c in p.get("Containers").map(Plist::array).unwrap_or(&[]) {
        for v in c.get("Volumes").map(Plist::array).unwrap_or(&[]) {
            let data = v
                .get("Roles")
                .map(Plist::array)
                .unwrap_or(&[])
                .iter()
                .any(|r| r.str() == Some("Data"));
            if let (true, Some(dev)) = (data, v.get("DeviceIdentifier").and_then(Plist::str)) {
                out.push(dev.to_string());
            }
        }
    }
    Ok(out)
}

/// The device name without `/dev/`.
fn bare(dev: &str) -> &str {
    dev.strip_prefix("/dev/").unwrap_or(dev)
}

/// The mount point of the Data volume: the volume that `diskutil` names
/// with the Data role, else the one named `Data` or `... - Data` (the
/// names of the macOS installer).
pub fn data_mount(e: &Entities, data_roles: &[String]) -> Option<PathBuf> {
    let by_role = e.volumes.iter().find_map(|(dev, m)| {
        data_roles
            .iter()
            .any(|d| bare(d) == bare(dev))
            .then(|| m.clone())
            .flatten()
    });
    by_role.or_else(|| {
        e.volumes.iter().find_map(|(_, m)| {
            let m = m.as_ref()?;
            let name = m.file_name()?.to_string_lossy();
            (name == "Data" || name.ends_with(" - Data")).then(|| m.clone())
        })
    })
}

/// The argv of the attach with the mount: the raw image class, no Finder
/// window, no owners, the mount points under `root`.
pub fn attach_argv(image: &Path, root: &Path) -> Vec<String> {
    vec![
        "/usr/bin/hdiutil".into(),
        "attach".into(),
        "-imagekey".into(),
        "diskimage-class=CRawDiskImage".into(),
        "-nobrowse".into(),
        "-owners".into(),
        "off".into(),
        "-mountRoot".into(),
        root.display().to_string(),
        "-plist".into(),
        image.display().to_string(),
    ]
}

/// The argv of the attach without a mount, for `fsck_apfs`.
pub fn attach_nomount_argv(image: &Path) -> Vec<String> {
    vec![
        "/usr/bin/hdiutil".into(),
        "attach".into(),
        "-imagekey".into(),
        "diskimage-class=CRawDiskImage".into(),
        "-nomount".into(),
        "-plist".into(),
        image.display().to_string(),
    ]
}

/// The argv of a detach; `force` after the plain one failed (Spotlight or
/// the fsck can hold the disk for a moment).
pub fn detach_argv(disk: &str, force: bool) -> Vec<String> {
    let mut v = vec!["/usr/bin/hdiutil".into(), "detach".into(), disk.to_string()];
    if force {
        v.push("-force".into());
    }
    v
}

/// The argv of the check: read-only, on the physical store.
pub fn fsck_argv(store: &str) -> Vec<String> {
    vec!["/sbin/fsck_apfs".into(), "-n".into(), dev_path(store)]
}

fn dev_path(dev: &str) -> String {
    format!("/dev/{}", bare(dev)) // not user text
}

/// Whether a failed `fsck_apfs` failed for want of rights: the check is
/// then skipped, not failed.
pub fn fsck_denied(output: &str) -> bool {
    [
        "Permission denied",
        "Operation not permitted",
        "must be run as root",
        "must be root",
    ]
    .iter()
    .any(|m| output.contains(m))
}

/// The output of `hdiutil` that says the disk is busy for a moment.
/// How long an attach waits for a VM that just ended to let go of the
/// image.
pub const LOCK_WAIT: Duration = Duration::from_secs(90);

/// Wait until no process holds the `flock` of `image`, at most `max`.
/// Virtualization keeps the disk of a VM locked for a while after the VM
/// ends, and an attach in that time fails with EAGAIN, whose text depends
/// on the language of the Mac. A probe lock taken and released at once
/// tells when the image is free, whatever the language. After `max` the
/// attach goes on and names its own error.
pub fn wait_unlocked(image: &Path, max: Duration) {
    use std::os::fd::AsRawFd;
    let deadline = std::time::Instant::now() + max;
    loop {
        let Ok(f) = std::fs::OpenOptions::new().read(true).open(image) else {
            return;
        };
        // SAFETY: flock on an fd that `f` owns.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            // SAFETY: as above.
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
            return;
        }
        if std::time::Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn busy(output: &str) -> bool {
    output.contains("Resource temporarily unavailable") || output.contains("Resource busy")
}

/// How long a disk command may run.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(600);

/// The tool of a Mac: `hdiutil`, `diskutil` and `fsck_apfs`.
#[derive(Debug, Default)]
pub struct Hdiutil;

/// Run an argv, wait at most `COMMAND_TIMEOUT`, and give the exit success
/// and both streams.
pub fn run(argv: &[String]) -> Result<(bool, String, String)> {
    use std::process::{Command, Stdio};
    let (prog, args) = argv
        .split_first()
        .ok_or_else(|| anyhow!(text::plist_unreadable("an empty command")))?;
    let child = Command::new(prog)
        .args(args)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!(text::cannot_run_command(&argv.join(" "), &e.to_string())))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(COMMAND_TIMEOUT) {
        Ok(Ok(o)) => Ok((
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )),
        Ok(Err(e)) => Err(anyhow!(text::cannot_run_command(
            &argv.join(" "),
            &e.to_string()
        ))),
        Err(_) => {
            // SAFETY: a plain kill of the child that this call started.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            Err(anyhow!(text::disk_command_timeout(
                &argv.join(" "),
                COMMAND_TIMEOUT
            )))
        }
    }
}

fn failed(argv: &[String], out: &str, err: &str) -> anyhow::Error {
    anyhow!(text::disk_command_failed(
        &argv.join(" "),
        out.trim(),
        err.trim()
    ))
}

impl Hdiutil {
    /// Run an attach, again after a busy answer, at most three times.
    fn attach_with(&self, argv: &[String]) -> Result<Entities> {
        let mut last = None;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(3));
            }
            let (ok, out, err) = run(argv)?;
            if ok {
                return entities(&out);
            }
            let busy_now = busy(&err) || busy(&out);
            last = Some(failed(argv, &out, &err));
            if !busy_now {
                break;
            }
        }
        Err(last.expect("one try ran"))
    }

    fn detach_disk(&self, disk: &str) -> Result<()> {
        for attempt in 0..4 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(2));
            }
            let argv = detach_argv(disk, attempt == 3);
            let (ok, out, err) = run(&argv)?;
            if ok {
                return Ok(());
            }
            if attempt == 3 {
                return Err(failed(&argv, &out, &err));
            }
        }
        unreachable!()
    }
}

impl DiskTool for Hdiutil {
    fn attach(&mut self, image: &Path, root: &Path) -> Result<Attached> {
        crate::privfs::dir(root)?;
        wait_unlocked(image, LOCK_WAIT);
        let e = self.attach_with(&attach_argv(image, root))?;
        let disk = e
            .disk
            .clone()
            .ok_or_else(|| anyhow!(text::attach_no_disk(image)))?;
        let argv: Vec<String> = ["/usr/sbin/diskutil", "apfs", "list", "-plist"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let roles = match run(&argv) {
            Ok((true, out, _)) => data_devices(&out).unwrap_or_default(),
            _ => Vec::new(),
        };
        match data_mount(&e, &roles) {
            Some(data) => Ok(Attached { disk, data }),
            None => {
                let _ = self.detach_disk(&disk);
                Err(anyhow!(text::attach_no_data_volume(image)))
            }
        }
    }

    fn detach(&mut self, a: &Attached) -> Result<()> {
        self.detach_disk(&a.disk)
    }

    fn fsck(&mut self, image: &Path) -> Result<Fsck> {
        let e = self.attach_with(&attach_nomount_argv(image))?;
        let disk = e
            .disk
            .clone()
            .ok_or_else(|| anyhow!(text::attach_no_disk(image)))?;
        let result = match &e.store {
            None => Err(anyhow!(text::attach_no_store(image))),
            Some(store) => {
                let argv = fsck_argv(store);
                match run(&argv) {
                    Ok((true, _, _)) => Ok(Fsck::Clean),
                    Ok((false, out, err)) if fsck_denied(&out) || fsck_denied(&err) => {
                        Ok(Fsck::Skipped(err.trim().to_string()))
                    }
                    Ok((false, out, err)) => Err(failed(&argv, &out, &err)),
                    Err(e) => Err(e),
                }
            }
        };
        let detached = self.detach_disk(&disk);
        let r = result?;
        detached?;
        Ok(r)
    }

    fn clone_file(&mut self, from: &Path, to: &Path) -> Result<()> {
        clone_file(from, to)
    }

    fn detach_stale(&mut self, root: &Path) {
        for m in mounts_under(root) {
            let argv = detach_argv(&m.display().to_string(), true);
            if let Err(e) = run(&argv) {
                log::warn!("macvm: {e:#}");
            }
        }
    }
}

/// An APFS clone of a file: `clonefile(2)` on macOS. Elsewhere a plain
/// copy, for the tests.
pub fn clone_file(from: &Path, to: &Path) -> Result<()> {
    let _ = std::fs::remove_file(to);
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes());
        let (Ok(a), Ok(b)) = (c(from), c(to)) else {
            return Err(anyhow!(text::not_utf8(from)));
        };
        // SAFETY: two NUL-terminated paths; flags 0.
        if unsafe { libc::clonefile(a.as_ptr(), b.as_ptr(), 0) } != 0 {
            let e = std::io::Error::last_os_error();
            return Err(anyhow!(text::cannot_clone_disk(from, &e.to_string())));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::copy(from, to)
            .map_err(|e| anyhow!(text::cannot_clone_disk(from, &e.to_string())))?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "diskimage_tests.rs"]
mod tests;

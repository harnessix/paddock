//! The caller chain: the process that asks tackd, its parent, and so on up
//! to the first process, read from the kernel.
//!
//! Each process is classified:
//!
//! - sandbox: a paddock sandbox (or another container) holds it. On Linux:
//!   its pid namespace or its root folder differs from the host's (those
//!   of process 1), or it is bwrap. On macOS: the kernel says it runs
//!   under a Seatbelt profile, or it is sandbox-exec.
//! - agent: a coding agent that paddock knows, by the name of its program
//!   or of the script that its interpreter runs (`node .../codex.js`).
//!
//! What a process can change about itself (its name, its arguments) only
//! adds marks: a process cannot remove a namespace it is in. A process
//! that detaches from its parent (a daemon) leaves the chain of its
//! children: the agent mark is best effort, the sandbox mark is not.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The longest chain that tackd walks.
pub const MAX_DEPTH: usize = 128;

/// One process, as the kernel reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// The real user id.
    pub uid: u32,
    /// The program file, when the kernel tells it.
    pub exe: Option<PathBuf>,
    /// The first two words of the command line (program and script).
    pub argv: Vec<String>,
    /// Outside the host's namespaces, or sandboxed by the kernel.
    pub contained: bool,
}

/// A source of processes: the kernel, or a fake table in tests.
pub trait Table {
    fn get(&self, pid: u32) -> Option<Proc>;
}

/// A fixed table for tests.
#[derive(Debug, Default, Clone)]
pub struct Fake(pub BTreeMap<u32, Proc>);

impl Fake {
    pub fn add(mut self, pid: u32, ppid: u32, exe: &str, argv: &[&str]) -> Self {
        self.0.insert(
            pid,
            Proc {
                pid,
                ppid,
                uid: 1000,
                exe: Some(PathBuf::from(exe)),
                argv: argv.iter().map(|s| s.to_string()).collect(),
                contained: false,
            },
        );
        self
    }
    pub fn contain(mut self, pid: u32) -> Self {
        if let Some(p) = self.0.get_mut(&pid) {
            p.contained = true;
        }
        self
    }
}

impl Table for Fake {
    fn get(&self, pid: u32) -> Option<Proc> {
        self.0.get(&pid).cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub proc: Proc,
    pub sandbox: bool,
    /// The agent's name, when it is one.
    pub agent: Option<String>,
}

impl Link {
    pub fn name(&self) -> String {
        self.proc
            .exe
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .or_else(|| self.proc.argv.first().map(|a| base(a).to_string()))
            .unwrap_or_else(|| "?".into())
    }

    /// `name[pid]`, with `(sandbox)` or `(agent NAME)`.
    pub fn summary(&self) -> String {
        let mut s = format!("{}[{}]", self.name(), self.proc.pid);
        if self.sandbox {
            s.push_str(" (sandbox)");
        }
        if let Some(a) = &self.agent {
            s.push_str(&format!(" (agent {a})"));
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Chain {
    /// The asking process first.
    pub links: Vec<Link>,
    /// The walk stopped early: too deep, or a loop.
    pub cut: Option<String>,
}

impl Chain {
    pub fn sandbox(&self) -> bool {
        self.links.iter().any(|l| l.sandbox)
    }
    pub fn agent(&self) -> Option<&str> {
        self.links.iter().find_map(|l| l.agent.as_deref())
    }
    pub fn summary(&self) -> Vec<String> {
        let mut out: Vec<String> = self.links.iter().map(Link::summary).collect();
        if let Some(c) = &self.cut {
            out.push(format!("({c})"));
        }
        out
    }
    /// Whether a process with this program file is among the callers
    /// (the asking process too).
    pub fn has_program(&self, p: &std::path::Path) -> bool {
        let want = super::program::real_path(p);
        self.links
            .iter()
            .filter_map(|l| l.proc.exe.as_ref())
            .any(|e| e == p || *e == want)
    }
}

fn base(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// Names that never count as sandbox programs or agents: the plain text
/// of a word, without a script suffix.
fn stem(s: &str) -> &str {
    let b = base(s);
    for ext in [".js", ".mjs", ".cjs", ".py", ".exe"] {
        if let Some(x) = b.strip_suffix(ext) {
            return x;
        }
    }
    b
}

/// The programs that start a sandbox around their child.
const SANDBOXERS: &[&str] = &["bwrap", "sandbox-exec"];

/// The command names of the coding agents that paddock has profiles for.
/// Desktop editors (VS Code) are not agents here.
pub fn agent_names() -> Vec<String> {
    let mut out = Vec::new();
    for (name, _) in crate::profile::BUILTIN {
        if *name == "vscode" {
            continue;
        }
        out.push(name.to_string());
        if let Some(p) = crate::profile::builtin(name) {
            out.extend(p.locate.commands);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Classify one process.
pub fn classify(p: Proc, agents: &[String]) -> Link {
    let mut words: Vec<&str> = Vec::new();
    if let Some(e) = p.exe.as_ref().and_then(|e| e.to_str()) {
        words.push(e);
    }
    words.extend(p.argv.iter().take(2).map(String::as_str));
    let sandbox = p.contained || words.iter().any(|w| SANDBOXERS.contains(&base(w)));
    let agent = words
        .iter()
        .map(|w| stem(w))
        .find(|w| agents.iter().any(|a| a == w))
        .map(str::to_string);
    Link {
        proc: p,
        sandbox,
        agent,
    }
}

/// Walk from `pid` up to the first process. The first link is `pid`
/// itself; None when it is not there (it ended).
pub fn chain(table: &dyn Table, pid: u32, agents: &[String]) -> Option<Chain> {
    let first = table.get(pid)?;
    let mut out = Chain::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut next = Some(first);
    while let Some(p) = next.take() {
        if !seen.insert(p.pid) {
            out.cut = Some("a loop in the process table".into());
            break;
        }
        if out.links.len() >= MAX_DEPTH {
            out.cut = Some(format!("more than {MAX_DEPTH} callers"));
            break;
        }
        let ppid = p.ppid;
        let pid = p.pid;
        out.links.push(classify(p, agents));
        if ppid == 0 || ppid == pid {
            break;
        }
        // A parent that ended between the reads: the chain ends there.
        next = table.get(ppid);
    }
    Some(out)
}

/// The kernel's process table.
pub struct Kernel {
    #[cfg(target_os = "linux")]
    host: linux::Host,
}

impl Kernel {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            host: linux::Host::read(),
        }
    }
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

impl Table for Kernel {
    #[cfg(target_os = "linux")]
    fn get(&self, pid: u32) -> Option<Proc> {
        linux::get(&self.host, std::path::Path::new("/proc"), pid)
    }
    #[cfg(target_os = "macos")]
    fn get(&self, pid: u32) -> Option<Proc> {
        macos::get(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn get(&self, _pid: u32) -> Option<Proc> {
        None
    }
}

/// The parts of `/proc/PID/stat` after the command name: (state, ppid).
/// The name is in parentheses and can hold spaces and `)`, so the text is
/// cut at the last `)`.
pub fn parse_stat(stat: &str) -> Option<(char, u32)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut it = rest.split_whitespace();
    let state = it.next()?.chars().next()?;
    let ppid = it.next()?.parse().ok()?;
    Some((state, ppid))
}

/// The real uid from `/proc/PID/status`.
pub fn parse_status_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|r| r.split_whitespace().next())
        .and_then(|u| u.parse().ok())
}

/// The first two words of `/proc/PID/cmdline` (NUL separated).
pub fn parse_cmdline(raw: &[u8]) -> Vec<String> {
    raw.split(|&b| b == 0)
        .filter(|w| !w.is_empty())
        .take(2)
        .map(|w| String::from_utf8_lossy(&w[..w.len().min(4096)]).to_string())
        .collect()
}

#[cfg(target_os = "linux")]
pub mod linux {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    /// The namespaces and root of the host, from process 1.
    #[derive(Debug, Clone, Default)]
    pub struct Host {
        pub pid_ns: Option<(u64, u64)>,
        pub root: Option<(u64, u64)>,
    }

    fn id(p: &Path) -> Option<(u64, u64)> {
        std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
    }

    impl Host {
        pub fn read() -> Self {
            Self::at(Path::new("/proc"), 1)
        }
        pub fn at(proc: &Path, pid: u32) -> Self {
            let d = proc.join(pid.to_string());
            Host {
                pid_ns: id(&d.join("ns/pid")),
                root: id(&d.join("root")),
            }
        }
    }

    /// One process from a proc folder. Reading the program and the
    /// namespaces of another user's process needs CAP_SYS_PTRACE; without
    /// it they are unknown, and an unknown namespace counts as contained
    /// when the host's is known.
    pub fn get(host: &Host, proc: &Path, pid: u32) -> Option<Proc> {
        let d = proc.join(pid.to_string());
        let stat = std::fs::read_to_string(d.join("stat")).ok()?;
        let (state, ppid) = parse_stat(&stat)?;
        if state == 'Z' || state == 'X' {
            return None;
        }
        let uid = parse_status_uid(&std::fs::read_to_string(d.join("status")).ok()?)?;
        let exe = std::fs::read_link(d.join("exe")).ok().map(|p| {
            // A replaced program shows as "PATH (deleted)".
            let s = p.to_string_lossy();
            match s.strip_suffix(" (deleted)") {
                Some(x) => PathBuf::from(x),
                None => p,
            }
        });
        let argv = std::fs::read(d.join("cmdline"))
            .map(|b| parse_cmdline(&b))
            .unwrap_or_default();
        let differs = |own: Option<(u64, u64)>, host: Option<(u64, u64)>| match (own, host) {
            (Some(a), Some(b)) => a != b,
            (None, Some(_)) => true,
            _ => false,
        };
        let contained =
            differs(id(&d.join("ns/pid")), host.pid_ns) || differs(id(&d.join("root")), host.root);
        Some(Proc {
            pid,
            ppid,
            uid,
            exe,
            argv,
            contained,
        })
    }
}

#[cfg(target_os = "macos")]
pub mod macos {
    use super::*;

    extern "C" {
        // libsystem_sandbox: nonzero when the process runs under a
        // Seatbelt profile. Called with no operation and no filter.
        fn sandbox_check(
            pid: libc::pid_t,
            operation: *const libc::c_char,
            ty: libc::c_int,
            ...
        ) -> libc::c_int;
    }

    pub fn get(pid: u32) -> Option<Proc> {
        let pid_c = libc::pid_t::try_from(pid).ok()?;
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: info is a proc_bsdinfo of the size we pass.
        let n = unsafe {
            libc::proc_pidinfo(
                pid_c,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if n != size {
            return None;
        }
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: buf has the size we pass.
        let len = unsafe {
            libc::proc_pidpath(
                pid_c,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len() as u32,
            )
        };
        let exe = (len > 0).then(|| {
            use std::os::unix::ffi::OsStrExt;
            PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len as usize]))
        });
        // SAFETY: a null operation and type 0 (SANDBOX_FILTER_NONE).
        let sandboxed = unsafe { sandbox_check(pid_c, std::ptr::null(), 0) } > 0;
        Some(Proc {
            pid,
            ppid: info.pbi_ppid,
            uid: info.pbi_ruid,
            exe,
            // The command line of another process needs sysctl(KERN_PROCARGS2);
            // the program path is enough on macOS, where agents are binaries
            // or node scripts with their own name in the path.
            argv: Vec::new(),
            contained: sandboxed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agents() -> Vec<String> {
        agent_names()
    }

    #[test]
    fn the_known_agents_are_named() {
        let a = agents();
        for n in [
            "claude",
            "codex",
            "gemini",
            "opencode",
            "copilot",
            "cursor-agent",
        ] {
            assert!(a.contains(&n.to_string()), "{n}");
        }
        assert!(!a.contains(&"code".to_string()));
    }

    #[test]
    fn a_plain_terminal_chain_has_no_marks() {
        let t = Fake::default()
            .add(1, 0, "/usr/lib/systemd/systemd", &["/sbin/init"])
            .add(500, 1, "/usr/bin/foot", &["foot"])
            .add(600, 500, "/usr/bin/bash", &["-bash"])
            .add(700, 600, "/usr/local/bin/paddock", &["paddock", "tack"]);
        let c = chain(&t, 700, &agents()).unwrap();
        assert_eq!(c.links.len(), 4);
        assert!(!c.sandbox() && c.agent().is_none(), "{:?}", c.summary());
        assert_eq!(c.summary()[0], "paddock[700]");
        assert!(c.has_program(std::path::Path::new("/usr/bin/foot")));
    }

    #[test]
    fn an_agent_in_the_chain_is_found_by_program_or_script() {
        let t = Fake::default()
            .add(1, 0, "/sbin/init", &[])
            .add(10, 1, "/usr/bin/node", &["node", "/opt/codex/bin/codex.js"])
            .add(11, 10, "/bin/bash", &["bash", "-c"])
            .add(
                20,
                1,
                "/home/demo/.local/share/claude/local/claude",
                &["claude"],
            )
            .add(21, 20, "/bin/zsh", &["zsh"]);
        let c = chain(&t, 11, &agents()).unwrap();
        assert_eq!(c.agent(), Some("codex"));
        assert!(
            c.summary()[1].contains("(agent codex)"),
            "{:?}",
            c.summary()
        );
        assert_eq!(chain(&t, 21, &agents()).unwrap().agent(), Some("claude"));
    }

    #[test]
    fn a_sandbox_is_found_by_namespace_or_by_its_starter() {
        let t = Fake::default()
            .add(1, 0, "/sbin/init", &[])
            .add(5, 1, "/usr/bin/bwrap", &["bwrap"])
            .add(6, 5, "/usr/bin/gh", &["gh"])
            .add(7, 1, "/usr/bin/foo", &["foo"])
            .contain(7);
        assert!(chain(&t, 6, &agents()).unwrap().sandbox());
        let c = chain(&t, 7, &agents()).unwrap();
        assert!(c.sandbox());
        assert_eq!(c.summary()[0], "foo[7] (sandbox)");
    }

    #[test]
    fn renaming_a_process_cannot_remove_the_sandbox_mark() {
        // A contained process that calls itself something harmless.
        let t = Fake::default()
            .add(1, 0, "/sbin/init", &[])
            .add(9, 1, "/tmp/innocent", &["innocent"])
            .contain(9);
        assert!(chain(&t, 9, &agents()).unwrap().sandbox());
    }

    #[test]
    fn loops_depth_and_missing_processes_end_the_walk() {
        let t = Fake::default().add(2, 3, "/a", &[]).add(3, 2, "/b", &[]);
        let c = chain(&t, 2, &agents()).unwrap();
        assert_eq!(c.links.len(), 2);
        assert!(c.cut.as_deref().unwrap().contains("loop"));
        let mut deep = Fake::default();
        for i in 1..=300u32 {
            deep = deep.add(i, i - 1, "/x", &[]);
        }
        let c = chain(&deep, 300, &agents()).unwrap();
        assert_eq!(c.links.len(), MAX_DEPTH);
        assert!(c.cut.is_some());
        assert!(chain(&t, 99, &agents()).is_none());
        // A parent that ended: the chain stops there.
        let t = Fake::default().add(4, 77, "/c", &[]);
        assert_eq!(chain(&t, 4, &agents()).unwrap().links.len(), 1);
    }

    #[test]
    fn stat_status_and_cmdline_parse_hostile_names() {
        assert_eq!(parse_stat("42 (a) b) c) S 7 42 42"), Some(('S', 7)));
        assert_eq!(parse_stat("42 (x"), None);
        assert_eq!(parse_stat("1 (init) S"), None);
        assert_eq!(
            parse_status_uid("Name:\tx\nUid:\t1000\t1000\t1000\t1000\n"),
            Some(1000)
        );
        assert_eq!(parse_status_uid("Name: x\n"), None);
        assert_eq!(
            parse_cmdline(b"node\0/a/codex.js\0--x\0"),
            ["node", "/a/codex.js"]
        );
        assert!(parse_cmdline(b"").is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_reads_from_proc() {
        let host = linux::Host::read();
        let me = std::process::id();
        let p = linux::get(&host, std::path::Path::new("/proc"), me).unwrap();
        assert_eq!(p.pid, me);
        assert_eq!(p.uid, unsafe { libc::getuid() });
        assert!(p.exe.is_some());
        let c = chain(&Kernel::new(), me, &agents()).unwrap();
        assert_eq!(c.links[0].proc.pid, me);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_fake_proc_folder_with_another_namespace_is_contained() {
        let t = tempfile::tempdir().unwrap();
        let mk = |pid: u32, ns: &str| {
            let d = t.path().join(pid.to_string());
            std::fs::create_dir_all(d.join("ns")).unwrap();
            std::fs::write(d.join("stat"), format!("{pid} (x) S 1 0")).unwrap();
            std::fs::write(d.join("status"), "Uid:\t1000\t1000\n").unwrap();
            std::fs::write(d.join("cmdline"), b"x\0").unwrap();
            std::fs::write(t.path().join(ns), "").unwrap();
            std::os::unix::fs::symlink(t.path().join(ns), d.join("ns/pid")).unwrap();
            std::os::unix::fs::symlink("/", d.join("root")).unwrap();
        };
        mk(1, "host-ns");
        mk(2, "host-ns");
        mk(3, "other-ns");
        let host = linux::Host::at(t.path(), 1);
        assert!(!linux::get(&host, t.path(), 2).unwrap().contained);
        assert!(linux::get(&host, t.path(), 3).unwrap().contained);
        // A zombie is not there.
        std::fs::write(t.path().join("2/stat"), "2 (x) Z 1 0").unwrap();
        assert!(linux::get(&host, t.path(), 2).is_none());
    }
}

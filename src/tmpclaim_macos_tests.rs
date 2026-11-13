//! TMP1 on a real Mac: Seatbelt with the rules of the claims, in the real
//! `/private/tmp`. Each test uses names of its own and removes them.

use super::*;
use crate::seatbelt::{Access, Net, Policy};
use std::process::Command;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

fn uid() -> u32 {
    // SAFETY: geteuid has no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

fn unique(tag: &str) -> String {
    let n: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    format!("paddock-tmp1-{tag}-{}-{n:x}", std::process::id())
}

struct Run {
    records: tempfile::TempDir,
    work: tempfile::TempDir,
}

impl Run {
    fn new() -> Self {
        Run {
            records: tempfile::tempdir().unwrap(),
            work: tempfile::tempdir().unwrap(),
        }
    }

    fn claim(&self, run: &str, pid: u32, wants: &[Resolved]) -> Claim {
        let live = |p: u32| crate::state::alive(p);
        let place = Place {
            root: Path::new(ROOT),
            records: self.records.path(),
            uid: uid(),
            live: &live,
            procs: &HostProcs,
        };
        claim(&place, run, pid, wants).unwrap()
    }

    fn release(&self, run: &str) -> Vec<String> {
        let live = |p: u32| crate::state::alive(p) && p != std::process::id();
        let place = Place {
            root: Path::new(ROOT),
            records: self.records.path(),
            uid: uid(),
            live: &live,
            procs: &HostProcs,
        };
        release(&place, run)
    }

    /// The profile of a terminal run with the rules of `rec` and a work
    /// folder of its own.
    fn profile(&self, rec: &Record) -> String {
        let mut p = Policy::terminal(Net::None, vec![]);
        p.add(
            crate::backend::seatbelt::real(self.work.path()),
            Access::ReadWrite,
            "work",
        );
        host::policy(&mut p, rec);
        crate::seatbelt::render(&p).unwrap()
    }

    /// Run a shell script in the sandbox. Exit code and output.
    fn sh(&self, profile: &str, script: &str) -> (i32, String) {
        let f = self.work.path().join("p.sb");
        std::fs::write(&f, profile).unwrap();
        let out = Command::new(SANDBOX_EXEC)
            .arg("-f")
            .arg(&f)
            .args(["/bin/sh", "-c", script])
            .current_dir(self.work.path())
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code().unwrap_or(-1), text)
    }
}

fn shared(name: &str, entry: &str) -> Resolved {
    Resolved {
        name: name.into(),
        kind: WantKind::Shared,
        entries: vec![entry.into(), format!("{entry}.lock")],
        replaces_args: vec![],
        stop: vec![],
        why: String::new(),
        home_bound: false,
    }
}

fn own(name: &str, kind: WantKind) -> Resolved {
    Resolved {
        name: name.into(),
        kind,
        entries: vec![],
        replaces_args: vec![],
        stop: vec![],
        why: String::new(),
        home_bound: false,
    }
}

#[test]
fn a_run_creates_reads_and_binds_its_own_names_in_private_tmp() {
    let r = Run::new();
    let dir = unique("dir");
    let folder = unique("shared");
    let rec = r
        .claim(
            "t1",
            std::process::id(),
            &[own(&dir, WantKind::Dir), shared(&folder, "hash")],
        )
        .record
        .unwrap();
    let prof = r.profile(&rec);
    let script = format!(
        r#"set -e
d=/tmp/{dir}; s=/tmp/{folder}
mkdir -p $d/sub && echo hi > $d/sub/f && cat $d/sub/f
# codex: mkdir of the folder that exists must say EEXIST, not EPERM.
mkdir $s 2>&1 | grep -q 'File exists'
: > $s/hash.lock && echo lock > $s/hash.lock && cat $s/hash.lock
/usr/bin/nc -U -l $s/hash > /dev/null & n=$!
i=0; while [ ! -S $s/hash ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done
test -S $s/hash
echo ping | /usr/bin/nc -U -w 2 $s/hash
kill $n 2>/dev/null || true
/usr/bin/nc -U -l $d/sock > /dev/null & n=$!
i=0; while [ ! -S $d/sock ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done
test -S $d/sock
kill $n 2>/dev/null || true
echo done"#
    );
    let (code, out) = r.sh(&prof, &script);
    assert_eq!(code, 0, "{out}\n{prof}");
    assert!(
        out.contains("hi") && out.contains("lock") && out.contains("done"),
        "{out}"
    );
    assert!(r.release("t1").is_empty());
    assert!(!Path::new(ROOT).join(&dir).exists());
    assert!(
        !Path::new(ROOT).join(&folder).exists(),
        "paddock made it, so it goes"
    );
}

#[test]
fn a_run_cannot_read_change_or_list_a_host_entry() {
    let r = Run::new();
    let host_file = Path::new(ROOT).join(unique("host"));
    std::fs::write(&host_file, "host secret").unwrap();
    let folder = unique("shared");
    let host_dir = Path::new(ROOT).join(&folder);
    std::fs::create_dir(&host_dir).unwrap();
    std::fs::set_permissions(
        &host_dir,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(host_dir.join("hostentry"), "host").unwrap();
    // The host file is refused, the run's own entry in the host folder is given.
    let c = r.claim(
        "t2",
        std::process::id(),
        &[
            own(
                host_file.file_name().unwrap().to_str().unwrap(),
                WantKind::File,
            ),
            shared(&folder, "runentry"),
        ],
    );
    assert_eq!(c.refused.len(), 1, "{c:?}");
    let rec = c.record.unwrap();
    let prof = r.profile(&rec);
    let h = host_file.display();
    let d = host_dir.display();
    for script in [
        format!("cat {h}"),
        format!("echo x > {h}"),
        format!("rm -f {h}"),
        format!("ls {d}"),
        format!("cat {d}/hostentry"),
        format!("echo x > {d}/hostentry"),
        format!("rm -f {d}/hostentry"),
        format!("echo x > {d}/other"),
        format!("rmdir {d}"),
        format!("echo x > /tmp/{}", unique("new")),
    ] {
        let (code, out) = r.sh(&prof, &script);
        assert_ne!(code, 0, "{script} must fail: {out}");
    }
    assert_eq!(std::fs::read_to_string(&host_file).unwrap(), "host secret");
    assert_eq!(
        std::fs::read_to_string(host_dir.join("hostentry")).unwrap(),
        "host"
    );
    let (code, out) = r.sh(
        &prof,
        &format!("echo mine > {d}/runentry && cat {d}/runentry"),
    );
    assert_eq!(code, 0, "{out}");
    assert!(r.release("t2").is_empty());
    assert!(
        host_dir.join("hostentry").exists(),
        "the host folder keeps its entry"
    );
    assert!(!host_dir.join("runentry").exists());
    std::fs::remove_dir_all(&host_dir).unwrap();
    std::fs::remove_file(&host_file).unwrap();
}

#[test]
fn two_runs_with_one_fixed_name_do_not_see_each_other() {
    let r = Run::new();
    let folder = unique("codex-daemon");
    let me = std::process::id();
    let a = r
        .claim("ta", me, &[shared(&folder, "hash-a")])
        .record
        .unwrap();
    let b = r
        .claim("tb", me, &[shared(&folder, "hash-b")])
        .record
        .unwrap();
    let d = Path::new(ROOT).join(&folder);
    let d = d.display();
    let (pa, pb) = (r.profile(&a), r.profile(&b));
    // Run a binds its socket.
    let (code, out) = r.sh(&pa, &format!("echo a > {d}/hash-a.lock"));
    assert_eq!(code, 0, "{out}");
    // Run b cannot read, change, connect to or remove what run a has.
    for script in [
        format!("cat {d}/hash-a.lock"),
        format!("echo b > {d}/hash-a.lock"),
        format!("rm -f {d}/hash-a.lock"),
        format!("ls {d}"),
    ] {
        let (code, out) = r.sh(&pb, &script);
        assert_ne!(code, 0, "{script} must fail in run b: {out}");
    }
    let (code, out) = r.sh(
        &pb,
        &format!("echo b > {d}/hash-b.lock && cat {d}/hash-b.lock"),
    );
    assert_eq!(code, 0, "{out}");
    // A third run with run a's entry (the same home) is told, and gets nothing.
    let c = r.claim("tc", me, &[shared(&folder, "hash-a")]);
    assert!(
        c.record.is_none() && c.refused[0].contains("run ta holds"),
        "{c:?}"
    );
    // One fixed name of its own: only the first run gets it.
    let fixed = unique("fixed");
    assert!(r
        .claim("td", me, &[own(&fixed, WantKind::Dir)])
        .record
        .is_some());
    let e = r.claim("te", me, &[own(&fixed, WantKind::Dir)]);
    assert!(e.refused[0].contains("run td holds it"), "{e:?}");
    for run in ["ta", "tb", "td"] {
        assert!(r.release(run).is_empty());
    }
    assert!(!Path::new(ROOT).join(&folder).exists());
    assert!(!Path::new(ROOT).join(&fixed).exists());
}

#[test]
fn only_a_process_of_the_run_counts_as_its_own() {
    let r = Run::new();
    let read = crate::backend::seatbelt::real(&r.work.path().join("runfolder"));
    let write = crate::backend::seatbelt::real(&r.work.path().join("tmpdir"));
    std::fs::create_dir_all(&read).unwrap();
    std::fs::create_dir_all(&write).unwrap();
    let mut p = Policy::terminal(Net::None, vec![]);
    p.add(&read, Access::ReadOnly, "run folder");
    p.add(&write, Access::ReadWrite, "tmpdir");
    let f = r.work.path().join("p.sb");
    std::fs::write(&f, crate::seatbelt::render(&p).unwrap()).unwrap();
    let mut inside = Command::new(SANDBOX_EXEC)
        .arg("-f")
        .arg(&f)
        .args(["/bin/sleep", "30"])
        .spawn()
        .unwrap();
    let mut outside = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let rec = Record {
        run: "t".into(),
        pid: 1,
        root: PathBuf::from(ROOT),
        items: vec![],
        stop: vec![],
        probe_read: read,
        probe_write: write,
        home: PathBuf::new(),
    };
    assert!(
        macos::of_run(inside.id(), &rec).unwrap(),
        "the sandboxed sleep"
    );
    assert!(
        !macos::of_run(outside.id(), &rec).unwrap(),
        "a host process"
    );
    assert!(macos::stop(outside.id(), &rec).is_err(), "left alone");
    macos::stop(inside.id(), &rec).unwrap();
    assert!(inside.wait().is_ok());
    outside.kill().unwrap();
    let _ = outside.wait();
}

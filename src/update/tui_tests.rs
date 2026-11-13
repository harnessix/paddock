//! The terminal interface comes with a release: `paddock update` installs
//! it, and the first `paddock tui` downloads it. The downloads of these
//! tests come from a release server on 127.0.0.1, never from the network.

use super::super::tests::{env_in, sha, tarball_with};
use super::super::{install as update, parse_response};
use super::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

const V: &str = "9.9.9";

/// A release server on 127.0.0.1. It serves the files of one release under
/// the paths of GitHub and records every request as `METHOD PATH`.
struct Server {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start(files: BTreeMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { return };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && s.read(&mut byte).is_ok_and(|n| n == 1) {
                    head.push(byte[0]);
                }
                let text = String::from_utf8_lossy(&head).to_string();
                let mut words = text.split_whitespace();
                let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                seen.lock().unwrap().push(format!("{method} {path}"));
                let _ = match files.get(path) {
                    Some(body) => {
                        let _ = write!(
                            s,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        if method == "GET" {
                            s.write_all(body)
                        } else {
                            Ok(())
                        }
                    }
                    None => s.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    ),
                };
            }
        });
        Self { port, log }
    }

    /// A server with the tarball of `rel` and its checksum file.
    fn with_release(rel: &Release, tarball: &[u8], sums: &[u8]) -> Self {
        let mut files = BTreeMap::new();
        for (url, body) in [(rel.tarball_url(), tarball), (rel.checksum_url(), sums)] {
            files.insert(path_of(&url), body.to_vec());
        }
        Self::start(files)
    }

    fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn path_of(url: &str) -> String {
    url::Url::parse(url).unwrap().path().to_string()
}

/// A release source that sends each GitHub URL to the local server, on
/// the same path, with real HTTP.
struct Local {
    port: u16,
}

impl Local {
    fn request(&self, method: &str, url: &str) -> Result<Vec<u8>> {
        check_url(url)?;
        let mut s = TcpStream::connect(("127.0.0.1", self.port))?;
        write!(
            s,
            "{method} {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            path_of(url)
        )?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw)?;
        Ok(raw)
    }
}

impl Source for Local {
    fn latest(&self) -> Result<Release> {
        bail!("not used")
    }
    fn download(&self, url: &str) -> Result<Vec<u8>> {
        let r = parse_response(&self.request("GET", url)?, 64 << 20, true)?;
        if r.status != 200 {
            bail!("downloading {url} failed with status {}", r.status);
        }
        Ok(r.body)
    }
    fn size(&self, url: &str) -> Option<u64> {
        let (status, _, length) = parse_head(&self.request("HEAD", url).ok()?).ok()?;
        length.filter(|_| status == 200)
    }
}

/// A host: an environment in `t` with tar on PATH, and a paddock binary.
fn host(t: &Path, vars: &[(&str, &str)]) -> (HostEnv, PathBuf) {
    let base = env_in(t, false);
    let mut all: BTreeMap<String, String> = [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "run"),
    ]
    .iter()
    .map(|(k, d)| (k.to_string(), t.join(d).display().to_string()))
    .collect();
    all.insert("PATH".into(), "/usr/bin:/bin".into());
    for (k, v) in vars {
        all.insert(k.to_string(), v.to_string());
    }
    let env = HostEnv::from_parts(base.home.clone(), "demo".into(), 1000, 1000, all);
    let exe = t.join("bin/paddock");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    program(&exe, "paddock old");
    (env, exe)
}

fn program(path: &Path, prints: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\necho '{prints}'\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn release() -> Release {
    Release::for_target(&format!("v{V}"), "x86_64", "linux").unwrap()
}

fn sums(rel: &Release, data: &[u8]) -> Vec<u8> {
    format!("{}  {}\n", sha(data), rel.tarball()).into_bytes()
}

/// A release build of version 9.9.9 for x86_64 Linux.
fn build(exe: &Path) -> Build<'_> {
    Build {
        version: V,
        release: true,
        os: "linux",
        arch: "x86_64",
        exe: Some(exe),
    }
}

/// A server with a good release 9.9.9.
fn good_release(t: &Path) -> Server {
    let rel = release();
    let tb = tarball_with(t, V, "paddock 9.9.9", Some("paddock-tui 9.9.9"));
    Server::with_release(&rel, &tb, &sums(&rel, &tb))
}

fn prints(bin: &Path) -> String {
    let out = Command::new(bin).arg("--version").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// The file names in the folder of `exe`.
fn files_next_to(exe: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(exe.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

use std::process::Command;

#[test]
fn the_first_start_downloads_checks_and_installs_the_tui() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[]);
    let server = good_release(t.path());
    let source = Local { port: server.port };
    let mut lines = Vec::new();
    let got = obtain(&env, &build(&exe), &source, false, &mut |l| {
        lines.push(l.to_string())
    })
    .unwrap();
    assert_eq!(got, exe.with_file_name("paddock-tui"));
    assert_eq!(prints(&got), "paddock-tui 9.9.9");
    assert_eq!(mode(&got), 0o755);
    // One line, with the version and the size of the download.
    assert_eq!(lines, ["Installing the paddock TUI 9.9.9 (1 MB)…"]);
    let rel = release();
    assert_eq!(
        server.requests(),
        [
            format!("HEAD {}", path_of(&rel.tarball_url())),
            format!("GET {}", path_of(&rel.tarball_url())),
            format!("GET {}", path_of(&rel.checksum_url())),
        ]
    );
    // The paddock binary is as it was, and no temporary file stays.
    assert_eq!(prints(&exe), "paddock old");
    assert_eq!(files_next_to(&exe), ["paddock", "paddock-tui"]);
    assert_eq!(
        std::fs::read_dir(env.paddock_cache().join("update"))
            .unwrap()
            .count(),
        0
    );
    // The next start finds it and downloads nothing.
    assert_eq!(find(&env, Some(&exe)).unwrap(), Some(got));
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn a_download_with_a_bad_checksum_installs_nothing() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[]);
    let rel = release();
    let tb = tarball_with(t.path(), V, "paddock 9.9.9", Some("paddock-tui 9.9.9"));
    let server = Server::with_release(&rel, &tb, &sums(&rel, b"another file"));
    let source = Local { port: server.port };
    let line = obtain(&env, &build(&exe), &source, false, &mut |_| {}).unwrap_err();
    assert!(line.contains("checksum"), "{line}");
    assert!(line.contains("could not install the TUI"), "{line}");
    assert_eq!(files_next_to(&exe), ["paddock"]);
    assert!(!private_copy(&env, V).exists());
}

#[test]
fn a_tui_of_another_version_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[]);
    let rel = release();
    let tb = tarball_with(t.path(), V, "paddock 9.9.9", Some("paddock-tui 0.0.1"));
    let server = Server::with_release(&rel, &tb, &sums(&rel, &tb));
    let source = Local { port: server.port };
    let line = obtain(&env, &build(&exe), &source, false, &mut |_| {}).unwrap_err();
    assert!(line.contains("paddock-tui 0.0.1"), "{line}");
    assert_eq!(files_next_to(&exe), ["paddock"]);
}

#[test]
fn a_release_without_the_tui_names_the_update() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[]);
    let rel = release();
    let tb = tarball_with(t.path(), V, "paddock 9.9.9", None);
    let server = Server::with_release(&rel, &tb, &sums(&rel, &tb));
    let source = Local { port: server.port };
    let line = obtain(&env, &build(&exe), &source, false, &mut |_| {}).unwrap_err();
    assert!(
        line.contains("release 9.9.9 has no usable paddock-tui") && line.contains("paddock update"),
        "{line}"
    );
    assert_eq!(files_next_to(&exe), ["paddock"]);
}

#[test]
fn an_offline_host_gets_one_line_with_the_command() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[]);
    // A port that no server listens on.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut lines = Vec::new();
    let line = obtain(&env, &build(&exe), &Local { port }, false, &mut |l| {
        lines.push(l.to_string())
    })
    .unwrap_err();
    assert_eq!(line.lines().count(), 1, "{line}");
    assert!(
        line.starts_with("paddock could not install the TUI: ")
            && line.ends_with("When the host is online, run this again: paddock tui"),
        "{line}"
    );
    // The size is unknown, so the line names none.
    assert_eq!(lines, ["Installing the paddock TUI 9.9.9…"]);
    assert_eq!(files_next_to(&exe), ["paddock"]);
}

#[test]
fn a_development_build_never_downloads() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[]);
    let server = good_release(t.path());
    let source = Local { port: server.port };
    let dev = Build {
        release: false,
        ..build(&exe)
    };
    for asked in [false, true] {
        let line = obtain(&env, &dev, &source, asked, &mut |l| panic!("{l}")).unwrap_err();
        assert_eq!(line.lines().count(), 1, "{line}");
        assert!(line.contains("development build"), "{line}");
        let command = format!("tui/scripts/install.sh {}", exe.parent().unwrap().display());
        assert!(line.ends_with(&command), "{line}");
        assert!(
            !line.contains("npx") && !line.contains("npm install"),
            "{line}"
        );
    }
    assert!(server.requests().is_empty(), "{:?}", server.requests());
    assert_eq!(files_next_to(&exe), ["paddock"]);
}

#[test]
fn the_switch_that_turns_updates_off_stops_the_download() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[("PADDOCK_NO_UPDATE_CHECK", "1")]);
    let server = good_release(t.path());
    let source = Local { port: server.port };
    let line = obtain(&env, &build(&exe), &source, false, &mut |l| panic!("{l}")).unwrap_err();
    assert_eq!(line.lines().count(), 1, "{line}");
    assert!(line.contains("PADDOCK_NO_UPDATE_CHECK=1"), "{line}");
    assert!(
        line.ends_with("Install it with: paddock update --tui"),
        "{line}"
    );
    assert!(server.requests().is_empty(), "{:?}", server.requests());
    // `paddock update --tui` is that command: the user asked for it.
    let got = obtain(&env, &build(&exe), &source, true, &mut |_| {}).unwrap();
    assert_eq!(prints(&got), "paddock-tui 9.9.9");
}

#[test]
fn a_sandbox_never_downloads() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[("PADDOCK", "1")]);
    let server = good_release(t.path());
    let source = Local { port: server.port };
    for asked in [false, true] {
        let line = obtain(&env, &build(&exe), &source, asked, &mut |l| panic!("{l}")).unwrap_err();
        assert!(line.contains("sandbox (PADDOCK=1)"), "{line}");
        assert!(line.ends_with("on the host: paddock tui"), "{line}");
    }
    assert!(server.requests().is_empty());
    // The install itself refuses too.
    let e = install(
        &env,
        &source,
        &release(),
        &exe,
        Path::new("tar"),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(e.to_string().contains("on the host only"), "{e}");
    assert!(server.requests().is_empty());
}

#[test]
fn a_system_without_a_release_binary_gets_its_own_command() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[]);
    let brew = Path::new("/opt/homebrew/Cellar/paddock/9.9.9/bin/paddock");
    let mac = Build {
        os: "macos",
        arch: "aarch64",
        exe: Some(brew),
        ..build(&exe)
    };
    let Plan::Refuse(line) = plan(&env, &mac, false) else {
        panic!("a download on macOS")
    };
    assert!(line.ends_with("Run: brew reinstall paddock"), "{line}");
    let cargo = Build {
        exe: Some(&exe),
        ..mac.clone()
    };
    let Plan::Refuse(line) = plan(&env, &cargo, false) else {
        panic!("a download on macOS")
    };
    assert!(line.contains("tui/scripts/install.sh "), "{line}");
    let riscv = Build {
        arch: "riscv64",
        ..build(&exe)
    };
    let Plan::Refuse(line) = plan(&env, &riscv, false) else {
        panic!("a download for riscv64")
    };
    assert!(
        line.contains("x86_64 and aarch64 only, not for riscv64")
            && line.contains("tui/scripts/install.sh "),
        "{line}"
    );
    assert_eq!(plan(&env, &build(&exe), false), Plan::Download(release()));
    // A folder with a space is one word of the command.
    let spaced = Build {
        release: false,
        exe: Some(Path::new("/opt/my tools/paddock")),
        ..build(&exe)
    };
    let Plan::Refuse(line) = plan(&env, &spaced, false) else {
        panic!("a download for a development build")
    };
    assert!(
        line.ends_with("tui/scripts/install.sh '/opt/my tools'"),
        "{line}"
    );
}

#[test]
fn a_folder_that_paddock_cannot_write_sends_the_tui_to_the_data_folder() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[]);
    let server = good_release(t.path());
    let source = Local { port: server.port };
    // An older copy of another version, and a folder like /usr/bin.
    let old = private_copy(&env, "9.9.8");
    program(&old, "paddock-tui 9.9.8");
    let bin = exe.parent().unwrap();
    std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o555)).unwrap();
    let got = obtain(&env, &build(&exe), &source, false, &mut |_| {});
    std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let got = got.unwrap();
    assert_eq!(got, private_copy(&env, V));
    assert_eq!(
        got,
        t.path().join("data/paddock/tui/9.9.9/paddock-tui"),
        "the copy is in the data folder of paddock"
    );
    assert_eq!(prints(&got), "paddock-tui 9.9.9");
    assert_eq!(files_next_to(&exe), ["paddock"]);
    assert!(!old.exists(), "the copy of the old version stays");
}

#[test]
fn the_order_is_override_then_sibling_then_own_copy_then_path() {
    let t = tempfile::tempdir().unwrap();
    let tools = t.path().join("tools");
    let (env, exe) = host(t.path(), &[("PATH", tools.to_str().unwrap())]);
    assert_eq!(find(&env, Some(&exe)).unwrap(), None);
    // npx and node on PATH change nothing: paddock never runs them.
    program(&tools.join("npx"), "npx");
    program(&tools.join("node"), "v22.11.0");
    assert_eq!(find(&env, Some(&exe)).unwrap(), None);

    let on_path = tools.join("paddock-tui");
    program(&on_path, "paddock-tui");
    assert_eq!(find(&env, Some(&exe)).unwrap(), Some(on_path));
    let own = private_copy(&env, CURRENT);
    program(&own, "paddock-tui");
    assert_eq!(find(&env, Some(&exe)).unwrap(), Some(own.clone()));
    // A copy of another version is not this paddock's.
    assert_eq!(installed(&env, None), Some(own));
    let sibling = exe.with_file_name("paddock-tui");
    program(&sibling, "paddock-tui");
    assert_eq!(find(&env, Some(&exe)).unwrap(), Some(sibling.clone()));
    // A file that is not executable is not a program.
    std::fs::set_permissions(&sibling, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_ne!(find(&env, Some(&exe)).unwrap(), Some(sibling));

    // PADDOCK_TUI comes first, and a wrong value is an error, not a
    // silent start of another program.
    let dev = t.path().join("dev/cli.js");
    program(&dev, "paddock-tui dev");
    let (env, exe) = host(t.path(), &[("PADDOCK_TUI", dev.to_str().unwrap())]);
    assert_eq!(find(&env, Some(&exe)).unwrap(), Some(dev));
    for bad in ["dev/cli.js", "/nonexistent/paddock-tui"] {
        let (env, exe) = host(t.path(), &[("PADDOCK_TUI", bad)]);
        let e = find(&env, Some(&exe)).unwrap_err().to_string();
        assert!(e.contains(&format!("PADDOCK_TUI={bad}")), "{e}");
    }
    // An empty value is no override.
    let (env, exe) = host(t.path(), &[("PADDOCK_TUI", "")]);
    assert!(find(&env, Some(&exe)).unwrap().is_some());
}

#[test]
fn update_installs_the_tui_next_to_paddock() {
    let t = tempfile::tempdir().unwrap();
    let (env, exe) = host(t.path(), &[]);
    program(&exe.with_file_name("paddock-tui"), "paddock-tui old");
    let server = good_release(t.path());
    let source = Local { port: server.port };
    update(&env, &source, &release(), &exe, None, Path::new("tar")).unwrap();
    assert_eq!(prints(&exe), "paddock 9.9.9");
    let tui = exe.with_file_name("paddock-tui");
    assert_eq!(prints(&tui), "paddock-tui 9.9.9");
    assert_eq!(mode(&tui), 0o755);
    assert_eq!(files_next_to(&exe), ["paddock", "paddock-tui"]);
    // One download serves both programs.
    assert_eq!(server.requests().len(), 2, "{:?}", server.requests());
}

#[test]
fn update_changes_nothing_when_the_tui_fails_a_check() {
    for tui in [None, Some("paddock-tui 0.0.1"), Some("paddock 9.9.9")] {
        let t = tempfile::tempdir().unwrap();
        let (env, exe) = host(t.path(), &[]);
        program(&exe.with_file_name("paddock-tui"), "paddock-tui old");
        let rel = release();
        let tb = tarball_with(t.path(), V, "paddock 9.9.9", tui);
        let server = Server::with_release(&rel, &tb, &sums(&rel, &tb));
        let source = Local { port: server.port };
        let e = update(&env, &source, &rel, &exe, None, Path::new("tar")).unwrap_err();
        assert!(e.to_string().contains("paddock-tui"), "{tui:?}: {e}");
        assert_eq!(prints(&exe), "paddock old", "{tui:?}");
        assert_eq!(
            prints(&exe.with_file_name("paddock-tui")),
            "paddock-tui old",
            "{tui:?}"
        );
        assert_eq!(files_next_to(&exe), ["paddock", "paddock-tui"]);
    }
}

#[test]
fn the_head_of_a_response_tells_the_size() {
    let (status, location, length) =
        parse_head(b"HTTP/1.1 200 OK\r\ncontent-length: 52013101\r\n\r\n").unwrap();
    assert_eq!((status, location, length), (200, None, Some(52_013_101)));
    let (status, location, length) =
        parse_head(b"HTTP/1.1 302 Found\r\nLocation: https://github.com/x\r\n\r\n").unwrap();
    assert_eq!(status, 302);
    assert_eq!(location.as_deref(), Some("https://github.com/x"));
    assert_eq!(length, None);
    assert!(parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n").is_err());
    assert_eq!(megabytes(52_013_101), "53 MB");
    assert_eq!(megabytes(12_000_000), "12 MB");
    assert_eq!(megabytes(1), "1 MB");
    assert_eq!(
        installing_line("0.2.3", Some(12_000_000)),
        "Installing the paddock TUI 0.2.3 (12 MB)…"
    );
}

#[test]
fn the_release_names_the_tui_in_the_tarball() {
    assert_eq!(
        release().tui_member(),
        "paddock-9.9.9-x86_64-linux/paddock-tui"
    );
}

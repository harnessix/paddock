//! The identity of a paddock build: release or development build, the
//! commit and a dirty mark. build.rs includes this file and writes the
//! result into `PADDOCK_VERSION` and `PADDOCK_BUILD` (src/lib.rs). The
//! crate includes it for its tests.
//!
//! The rule:
//! - In a git checkout, a build is a release only when the tree has no
//!   change to a tracked file and HEAD carries the tag `v` + the version of
//!   Cargo.toml. Every other checkout gives a development build.
//! - Without a git checkout (a source tarball), only the marker
//!   `PADDOCK_RELEASE_BUILD=1` makes a release. The release workflow and the
//!   Homebrew formula set it.
//! - The marker never makes a release of a checkout that is not one: build.rs
//!   stops with an error instead.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::Path;
use std::process::Command;

pub const RELEASE: &str = "release";
pub const DEVELOPMENT: &str = "development build";

/// What git says about the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Git {
    /// The full commit hash of HEAD.
    pub commit: String,
    /// A tracked file has a change, staged or not.
    pub dirty: bool,
    /// The tags of HEAD.
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub version: String,
    pub release: bool,
    /// The full commit hash, when git told it.
    pub commit: Option<String>,
    pub dirty: bool,
}

impl Identity {
    /// The identity of a build of `version`. `marker` is the value of
    /// `PADDOCK_RELEASE_BUILD`, `git` the state of the checkout, `None`
    /// without one. An error when the marker asks for a release of a
    /// checkout that is not a clean release tag.
    pub fn new(version: &str, marker: bool, git: Option<&Git>) -> Result<Self, String> {
        let tag = format!("v{version}");
        let release = match git {
            Some(g) => {
                let ok = !g.dirty && g.tags.contains(&tag);
                if marker && !ok {
                    return Err(format!(
                        "PADDOCK_RELEASE_BUILD is set, but the checkout is not a clean {tag}: \
                         commit {}, tags {:?}, dirty {}",
                        g.commit, g.tags, g.dirty
                    ));
                }
                ok
            }
            None => marker,
        };
        Ok(Self {
            version: version.to_string(),
            release,
            commit: git.map(|g| g.commit.clone()),
            dirty: git.is_some_and(|g| g.dirty),
        })
    }

    fn short(&self) -> &str {
        match &self.commit {
            Some(c) => &c[..c.len().min(7)],
            None => "unknown",
        }
    }

    /// The text after `paddock ` in `paddock --version`:
    /// `0.2.1` or `0.2.1-dev (commit 1a2b3c4, dirty)`.
    pub fn version_string(&self) -> String {
        if self.release {
            return self.version.clone();
        }
        let dirty = if self.dirty { ", dirty" } else { "" };
        format!("{}-dev (commit {}{dirty})", self.version, self.short())
    }

    /// `release (commit FULL)` or `development build (commit FULL, dirty)`.
    pub fn summary(&self) -> String {
        let kind = if self.release { RELEASE } else { DEVELOPMENT };
        let commit = self.commit.as_deref().unwrap_or("unknown");
        let dirty = if self.dirty { ", dirty" } else { "" };
        format!("{kind} (commit {commit}{dirty})")
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    git_bytes(dir, args).map(|b| String::from_utf8_lossy(&b).trim_end().to_string())
}

/// The standard output of a successful `git -C dir ARGS`, as bytes.
fn git_bytes(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    // `--no-optional-locks`: `git status` would else take index.lock and
    // write the index, so a `git commit` during a build could fail, and the
    // new index (watched by `git_watch`) would run build.rs again.
    let out = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// The state of the git checkout whose top folder is `dir`. `None` when git
/// is missing or `dir` is not the top of a checkout: a source tarball that
/// sits inside another repository is no checkout of paddock.
pub fn read_git(dir: &Path) -> Option<Git> {
    let top = git(dir, &["rev-parse", "--show-toplevel"])?;
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same(Path::new(&top), dir) {
        return None;
    }
    let commit = git(dir, &["rev-parse", "--verify", "HEAD"])?;
    // Untracked files do not count: a new file reaches the build only
    // through a change to a tracked one.
    let status = git(dir, &["status", "--porcelain", "--untracked-files=no"])?;
    let tags = git(dir, &["tag", "--points-at", "HEAD"])?;
    Some(Git {
        commit,
        dirty: !status.is_empty(),
        tags: tags.lines().map(str::to_string).collect(),
    })
}

/// The files whose change can change what `read_git` returns, for
/// `cargo:rerun-if-changed`: the git files and every tracked file. The
/// binary embeds files from profiles/, skills/, packaging/, docs/ and
/// tests/, and a change to any tracked file changes the dirty mark.
///
/// The list names files, not folders: cargo scans a folder with all its
/// subfolders, so an ignored subfolder such as pkgbuild/src or mac/.build
/// would run build.rs again. When `git ls-files` fails, the list falls back
/// to the top entries of `dir` (see `tracked_watch`).
pub fn git_watch(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let path = |p: &str| {
        git(
            dir,
            &["rev-parse", "--path-format=absolute", "--git-path", p],
        )
    };
    for p in ["HEAD", "index", "packed-refs", "refs/tags"] {
        out.extend(path(p));
    }
    if let Some(head) = git(dir, &["symbolic-ref", "-q", "HEAD"]) {
        out.extend(path(&head));
    }
    // Only git paths that exist: cargo runs a build script on every build
    // for a missing path, and packed-refs or a branch file can be absent.
    out.retain(|p| Path::new(p).exists());
    let files = git_bytes(dir, &["ls-files", "-z", "--full-name"]);
    out.extend(tracked_watch(dir, files.as_deref()));
    out
}

/// The watch paths for the tracked files. `ls_files` is the output of
/// `git ls-files -z`, `None` when git failed.
///
/// - A tracked file gives its own path. A tracked file that is missing on
///   disk stays in the list: cargo then runs build.rs on every build, and
///   so notices when the file comes back. Until then the build is dirty.
/// - A name that is not UTF-8, or that holds a line break, cannot be one
///   `cargo:` line. Its top folder takes its place.
/// - Without the list (git failed), every top entry of `dir` except `.git`
///   and `target` is watched, as before the list of files.
pub fn tracked_watch(dir: &Path, ls_files: Option<&[u8]>) -> Vec<String> {
    let clean = |s: &str| !s.is_empty() && !s.contains(['\n', '\r']);
    let mut out = Vec::new();
    match ls_files {
        Some(files) => {
            for f in files.split(|&b| b == 0).filter(|f| !f.is_empty()) {
                match std::str::from_utf8(f) {
                    Ok(name) if clean(name) => out.push(name.to_string()),
                    _ => {
                        let top = f.split(|&b| b == b'/').next().unwrap_or_default();
                        match std::str::from_utf8(top) {
                            // A top folder that exists: a broken name of a
                            // file directly in `dir` has none.
                            Ok(t) if clean(t) && f.len() > top.len() => out.push(t.to_string()),
                            _ => {}
                        }
                    }
                }
            }
        }
        None => {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    if let Some(name) = e.file_name().to_str() {
                        if clean(name) && name != ".git" && name != "target" {
                            out.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out.into_iter()
        .map(|p| dir.join(p).to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d";

    fn checkout(dirty: bool, tags: &[&str]) -> Git {
        Git {
            commit: SHA.into(),
            dirty,
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn a_commit_after_the_tag_is_a_development_build() {
        let id = Identity::new("0.2.1", false, Some(&checkout(false, &[]))).unwrap();
        assert!(!id.release);
        assert_eq!(id.version_string(), "0.2.1-dev (commit 1a2b3c4)");
        assert_eq!(id.summary(), format!("development build (commit {SHA})"));
    }

    #[test]
    fn a_change_marks_the_build_dirty_also_on_the_tag() {
        let id = Identity::new("0.2.1", false, Some(&checkout(true, &["v0.2.1"]))).unwrap();
        assert_eq!(id.version_string(), "0.2.1-dev (commit 1a2b3c4, dirty)");
        assert_eq!(
            id.summary(),
            format!("development build (commit {SHA}, dirty)")
        );
    }

    #[test]
    fn another_tag_does_not_make_a_release() {
        let id = Identity::new("0.2.1", false, Some(&checkout(false, &["v0.2.0", "x"]))).unwrap();
        assert!(!id.release);
    }

    #[test]
    fn a_clean_release_tag_prints_only_the_version() {
        for marker in [false, true] {
            let id = Identity::new("0.2.1", marker, Some(&checkout(false, &["v0.2.1"]))).unwrap();
            assert_eq!(id.version_string(), "0.2.1");
            assert_eq!(id.summary(), format!("release (commit {SHA})"));
        }
    }

    #[test]
    fn the_marker_cannot_make_a_release_of_another_checkout() {
        for g in [checkout(false, &[]), checkout(true, &["v0.2.1"])] {
            let err = Identity::new("0.2.1", true, Some(&g)).unwrap_err();
            assert!(err.contains("not a clean v0.2.1"), "{err}");
        }
    }

    #[test]
    fn a_tarball_is_a_release_only_with_the_marker() {
        let id = Identity::new("0.2.1", true, None).unwrap();
        assert_eq!(id.version_string(), "0.2.1");
        assert_eq!(id.summary(), "release (commit unknown)");
        let id = Identity::new("0.2.1", false, None).unwrap();
        assert_eq!(id.version_string(), "0.2.1-dev (commit unknown)");
        assert_eq!(id.summary(), "development build (commit unknown)");
    }

    fn run(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn reads_a_real_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        assert_eq!(read_git(d), None, "no checkout yet");
        run(d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join("a"), "1").unwrap();
        run(d, &["add", "a"]);
        run(d, &["commit", "-q", "-m", "one"]);
        let g = read_git(d).unwrap();
        assert_eq!(g.commit.len(), 40);
        assert!(!g.dirty);
        assert!(g.tags.is_empty());
        assert!(!git_watch(d).is_empty());

        run(d, &["tag", "v0.2.1"]);
        std::fs::write(d.join("untracked"), "x").unwrap();
        let g = read_git(d).unwrap();
        assert_eq!(g.tags, ["v0.2.1"]);
        assert!(!g.dirty, "an untracked file is no change");
        assert!(Identity::new("0.2.1", true, Some(&g)).unwrap().release);

        std::fs::write(d.join("a"), "2").unwrap();
        let g = read_git(d).unwrap();
        assert!(g.dirty);
        assert!(Identity::new("0.2.1", true, Some(&g)).is_err());

        // A folder inside the checkout is not the top of one.
        std::fs::create_dir(d.join("sub")).unwrap();
        assert_eq!(read_git(&d.join("sub")), None);
    }

    #[test]
    fn watches_every_tracked_path() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        run(d, &["init", "-q", "-b", "main"]);
        for f in ["profiles/codex.toml", "skills/a/SKILL.md", "README.md"] {
            std::fs::create_dir_all(d.join(f).parent().unwrap()).unwrap();
            std::fs::write(d.join(f), "1").unwrap();
        }
        run(d, &["add", "."]);
        run(d, &["commit", "-q", "-m", "one"]);
        std::fs::create_dir(d.join("target")).unwrap();
        let watch = git_watch(d);
        for t in ["profiles/codex.toml", "skills/a/SKILL.md", "README.md"] {
            let p = d.join(t).to_string_lossy().into_owned();
            assert!(watch.contains(&p), "{t} not in {watch:?}");
        }
        let target = d.join("target").to_string_lossy().into_owned();
        assert!(!watch.contains(&target), "untracked folder in {watch:?}");
    }

    #[test]
    fn an_ignored_subfolder_is_not_watched() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        run(d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join(".gitignore"), "/pkgbuild/src\n/mac/.build\n").unwrap();
        for f in ["pkgbuild/PKGBUILD", "mac/Package.swift"] {
            std::fs::create_dir_all(d.join(f).parent().unwrap()).unwrap();
            std::fs::write(d.join(f), "1").unwrap();
        }
        run(d, &["add", "."]);
        run(d, &["commit", "-q", "-m", "one"]);
        for f in ["pkgbuild/src/x.c", "mac/.build/debug.yaml"] {
            std::fs::create_dir_all(d.join(f).parent().unwrap()).unwrap();
            std::fs::write(d.join(f), "build output").unwrap();
        }
        let watch = git_watch(d);
        for f in [".gitignore", "pkgbuild/PKGBUILD", "mac/Package.swift"] {
            let p = d.join(f).to_string_lossy().into_owned();
            assert!(watch.contains(&p), "{f} not in {watch:?}");
        }
        // cargo scans a watched folder with all its subfolders: no watched
        // path of the work tree may be or hold an ignored folder. The git
        // folder refs/tags is meant (a new tag).
        assert!(watch.iter().any(|p| p.ends_with("refs/tags")), "{watch:?}");
        for p in &watch {
            let p = Path::new(p);
            if p.starts_with(d) && !p.starts_with(d.join(".git")) {
                assert!(!p.is_dir(), "folder {p:?} in {watch:?}");
            }
        }
    }

    #[test]
    fn a_broken_or_missing_name_still_has_a_watch() {
        let d = Path::new("/co");
        let at = |p: &str| d.join(p).to_string_lossy().into_owned();
        let list = b"README.md\0gone.rs\0docs/a\nb.md\0skills/\xff.md\0\xfe\0a\rb\0";
        let watch = tracked_watch(d, Some(list));
        // A missing file stays: cargo then reruns build.rs until it is back.
        assert_eq!(
            watch,
            [at("README.md"), at("docs"), at("gone.rs"), at("skills")],
            "a line break or a non-UTF-8 name gives its top folder"
        );
        for p in &watch {
            assert!(!p.contains(['\n', '\r']), "{p:?}");
        }
    }

    #[test]
    fn without_the_file_list_the_top_entries_are_watched() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        for f in [".git/HEAD", "target/x", "src/main.rs", "Cargo.toml"] {
            std::fs::create_dir_all(d.join(f).parent().unwrap()).unwrap();
            std::fs::write(d.join(f), "1").unwrap();
        }
        let at = |p: &str| d.join(p).to_string_lossy().into_owned();
        assert_eq!(tracked_watch(d, None), [at("Cargo.toml"), at("src")]);
    }

    #[test]
    fn reading_the_checkout_does_not_write_the_index() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        run(d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join("a"), "1").unwrap();
        run(d, &["add", "a"]);
        run(d, &["commit", "-q", "-m", "one"]);
        // Same content, another mtime: a plain `git status` refreshes the
        // stat data and writes the index.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(d.join("a"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let index = d.join(".git/index");
        let before = std::fs::read(&index).unwrap();
        let g = read_git(d).unwrap();
        assert!(!g.dirty);
        assert_eq!(
            std::fs::read(&index).unwrap(),
            before,
            "git wrote the index"
        );
    }
}

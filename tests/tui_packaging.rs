//! Every way to install paddock also installs `paddock-tui`, the terminal
//! interface: the release tarball, the installer, the Arch package, the
//! Homebrew formula and the steps of the README. The tools that build it
//! are pinned by version and checksum. These tests read the files. No
//! build runs and nothing is downloaded.

use std::path::Path;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn read(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The text of one job of a workflow: from its key to the next job.
fn job<'a>(text: &'a str, name: &str) -> &'a str {
    let start = text
        .find(&format!("\n  {name}:\n"))
        .unwrap_or_else(|| panic!("no job {name}"));
    let rest = &text[start + 1..];
    let end = rest[3..]
        .match_indices("\n  ")
        .find(|(i, _)| {
            let line = rest[3 + i + 1..].lines().next().unwrap_or("");
            line.starts_with("  ")
                && !line.starts_with("   ")
                && line.trim_end().ends_with(':')
                && !line.trim_start().starts_with('#')
        })
        .map_or(rest.len(), |(i, _)| 3 + i);
    &rest[..end]
}

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("missing: {needle}"))
}

#[test]
fn the_release_tarball_holds_the_tui_next_to_paddock() {
    let yml = read(".github/workflows/release.yml");
    let build = job(&yml, "build");
    // One program per Linux target, built on that target.
    for (arch, target) in [("x86_64", "linux-x64"), ("aarch64", "linux-arm64")] {
        assert!(
            build.contains(&format!(
                "- arch: {arch}\n            os: linux\n            runner: ubuntu-22.04{}\n            tui: {target}\n",
                if arch == "aarch64" { "-arm" } else { "" }
            )),
            "no paddock-tui for {arch}"
        );
    }
    let compile = position(
        build,
        r#"bun scripts/bun-build.mjs native "bin/paddock-tui-${{ matrix.tui }}""#,
    );
    let check = position(
        build,
        r#"[ "$out" = "paddock-tui ${TAG#v}" ] || { echo "::error::"#,
    );
    let pack = position(
        build,
        r#"install -m 755 "tui/bin/paddock-tui-${{ matrix.tui }}" "dist/$name/paddock-tui""#,
    );
    let tar = position(build, r#"tar -C dist -czf "dist/$name.tar.gz" "$name""#);
    let sum = position(build, r#"sha256sum "$name.tar.gz" > "$name.tar.gz.sha256""#);
    assert!(compile < check && check < pack && pack < tar && tar < sum);
    // The tarball is checked for both programs before the upload.
    assert!(build.contains("for program in paddock paddock-tui; do"));
    // The publish job attests the tarball, and so both programs in it.
    let publish = job(&yml, "publish");
    let attest = &publish[position(publish, "uses: actions/attest-build-provenance@")..];
    assert!(attest.contains("dist/*.tar.gz"), "{attest}");
    // The release waits for the CI, which builds the program too.
    assert!(publish.contains("needs: [commit, ci, build, source, vm]"));
}

#[test]
fn the_build_tools_are_pinned_by_version_and_checksum() {
    let toml = read("tui/mise.toml");
    let lock = read("tui/mise.lock");
    for tool in ["node", "bun"] {
        let version = toml
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{tool} = \"")))
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or_else(|| panic!("tui/mise.toml has no exact {tool}"));
        assert!(
            version.split('.').count() == 3 && version.split('.').all(|p| p.parse::<u32>().is_ok()),
            "{tool} = {version} is not an exact version"
        );
        let head = format!("[[tools.{tool}]]\nversion = \"{version}\"\n");
        assert!(
            lock.contains(&head),
            "tui/mise.lock does not lock {tool} {version}"
        );
        // The platforms of the release, of the CI and of a Mac build.
        for platform in ["linux-x64", "linux-arm64", "macos-arm64"] {
            let key = format!("[tools.{tool}.\"platforms.{platform}\"]\n");
            let entry = &lock[position(&lock, &key) + key.len()..];
            let entry = entry.split("\n\n").next().unwrap();
            let sum = entry
                .lines()
                .find_map(|l| l.strip_prefix("checksum = \"sha256:"))
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or_else(|| panic!("{tool} {platform} has no sha256"));
            assert!(
                sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()),
                "{tool} {platform}: {sum}"
            );
            assert!(
                entry.contains("url = \"https://") && entry.contains(version),
                "{tool} {platform}: {entry}"
            );
        }
    }
    // Each job that installs the tools refuses one without a lock entry.
    let release = read(".github/workflows/release.yml");
    let ci = read(".github/workflows/ci.yml");
    for (name, text) in [
        ("release build", job(&release, "build")),
        ("CI tui", job(&ci, "tui")),
    ] {
        let uses = position(text, "uses: jdx/mise-action@");
        assert!(text.contains("working_directory: tui"), "{name}");
        assert!(text.contains("MISE_LOCKED: \"1\""), "{name}");
        // The last one: a comment names the command before the step.
        assert!(text.rfind("npm ci").unwrap() > uses, "{name}");
    }
    // npm installs from the lockfile only, in every build.
    for file in [
        ".github/workflows/release.yml",
        "tui/scripts/install.sh",
        "packaging/homebrew/paddock.rb",
    ] {
        let text = read(file);
        assert!(
            text.contains("ci") && !text.contains("npm install"),
            "{file}"
        );
        assert!(!text.contains("npx"), "{file}");
    }
    assert!(read("tui/scripts/install.sh").contains("export MISE_LOCKED=1"));
}

#[test]
fn the_ci_builds_and_runs_the_tui_of_each_release_target() {
    let ci = read(".github/workflows/ci.yml");
    let tui = job(&ci, "tui");
    for (target, runner) in [
        ("linux-x64", "ubuntu-24.04"),
        ("linux-arm64", "ubuntu-24.04-arm"),
    ] {
        assert!(
            tui.contains(&format!(
                "- target: {target}\n            runner: {runner}\n"
            )),
            "{target}"
        );
    }
    assert!(tui.contains("node scripts/compile.mjs ${{ matrix.target }}"));
    assert!(tui.contains(
        "PADDOCK_TUI_UNDER_TEST: ${{ github.workspace }}/tui/bin/paddock-tui-${{ matrix.target }}"
    ));
    assert!(tui.contains(r#"scripts/install.sh "$RUNNER_TEMP/tui-bin""#));
    // No step of the build may be skipped on a failure.
    assert!(!tui.contains("continue-on-error"));
}

#[test]
fn the_installer_and_the_arch_package_install_the_tui() {
    let install = read("bin/install");
    assert!(install.contains(r#""$root/tui/scripts/install.sh" "$HOME/.local/bin""#));
    let pkg = read("pkgbuild/PKGBUILD");
    let build = position(&pkg, r#"tui/scripts/install.sh "$PWD/target/release""#);
    let package = position(
        &pkg,
        r#"install -Dm755 target/release/paddock-tui "$pkgdir/usr/bin/paddock-tui""#,
    );
    assert!(position(&pkg, "cargo build --release --locked") < build && build < package);
    // strip removes the code of the program: it must stay off.
    assert!(pkg.contains("\noptions=('!strip')\n"));
    assert!(pkg.contains("makedepends=('cargo' 'mise')"));

    let script = read("tui/scripts/install.sh");
    use std::os::unix::fs::PermissionsExt;
    let mode =
        std::fs::metadata(Path::new(env!("CARGO_MANIFEST_DIR")).join("tui/scripts/install.sh"))
            .unwrap()
            .permissions()
            .mode();
    assert!(
        mode & 0o111 != 0,
        "tui/scripts/install.sh is not executable"
    );
    // The program replaces the old one only after it printed its version.
    let version = position(&script, r#"got="$("$new" --version)""#);
    let rename = position(&script, r#"mv -f "$new" "$dest/paddock-tui""#);
    assert!(position(&script, "bun scripts/bun-build.mjs native") < version && version < rename);
}

#[test]
fn the_homebrew_formula_builds_the_tui_with_build_dependencies_only() {
    let rb = read("packaging/homebrew/paddock.rb");
    for dep in ["bun", "node", "rust"] {
        assert!(
            rb.contains(&format!("  depends_on \"{dep}\" => :build\n")),
            "{dep}"
        );
    }
    // No dependency stays after the build.
    for line in rb
        .lines()
        .filter(|l| l.trim_start().starts_with("depends_on"))
    {
        assert!(line.trim_end().ends_with("=> :build"), "{line}");
    }
    let cargo = position(&rb, r#"system "cargo", "install", *std_cargo_args"#);
    let npm = position(&rb, r#"system "npm", "ci", "--ignore-scripts""#);
    let build = position(&rb, r#"system "npm", "run", "build""#);
    let bun = position(
        &rb,
        r#"system "bun", "scripts/bun-build.mjs", "native", bin/"paddock-tui""#,
    );
    // Before the early return of a Linux install.
    let early = position(&rb, "return unless OS.mac?");
    assert!(cargo < npm && npm < build && build < bun && bun < early);
    assert!(rb.contains(
        r##"assert_equal "paddock-tui #{version}", shell_output("#{bin}/paddock-tui --version").strip"##
    ));
}

#[test]
fn the_documents_install_the_tui_and_drop_the_build_by_hand() {
    let readme = read("README.md");
    let paddock = position(
        &readme,
        &format!("install -m 755 paddock-{VERSION}-x86_64-linux/paddock ~/.local/bin/paddock\n"),
    );
    let tui = position(
        &readme,
        &format!(
            "install -m 755 paddock-{VERSION}-x86_64-linux/paddock-tui ~/.local/bin/paddock-tui\n"
        ),
    );
    assert!(paddock < tui);
    let guide = read("src/help/guide-install.md");
    assert!(guide.contains("/paddock-tui ~/.local/bin/paddock-tui\n"));
    assert!(guide.contains("paddock update --tui"));
    // The old steps told the user to build the interface and to link it.
    for file in [
        "README.md",
        "src/help/guide-install.md",
        "tui/README.md",
        "src/cli/tui.rs",
        "src/update/tui.rs",
    ] {
        let text = read(file);
        for old in [
            "cd tui && npm ci && npm run build",
            "put tui/dist/cli.js on PATH",
            "does not download it with npx",
        ] {
            assert!(!text.contains(old), "{file} still says: {old}");
        }
    }
}

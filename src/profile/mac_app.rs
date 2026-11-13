use serde::Deserialize;

/// `[app.macos]`: where the app lives on macOS and how it starts there.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacApp {
    /// Candidate app bundles, in order. `~/` expands to the home directory.
    #[serde(default)]
    pub bundles: Vec<String>,
    /// Arguments that replace `[launch] args` on macOS.
    #[serde(default)]
    pub args: Vec<String>,
    /// The app's data folder, relative to the private home. Electron apps
    /// get it as `--user-data-dir`. Default: `Library/Application
    /// Support/DISPLAY_NAME`.
    pub user_data: Option<String>,
    /// The app uses Apple's App Sandbox, which cannot run inside
    /// `sandbox-exec`. paddock refuses it even before it reads the
    /// signature.
    #[serde(default)]
    pub app_sandbox: bool,
    /// Host globs (`x.y` or `*.x.y`, port 443) that the firewall of the
    /// macOS guest route allows for this app with `by = "profile"`, so a
    /// plain login asks nothing. The user's own rules win over them.
    #[serde(default)]
    pub seed_hosts: Vec<String>,
}

/// `[macos]` of a harness profile: the backend choice of a terminal run on
/// macOS (vm/select.rs).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileMacos {
    /// "seatbelt": `--backend auto` keeps this harness on Seatbelt, for
    /// tools that need macOS itself (Xcode, swift for Apple targets,
    /// codesign).
    pub backend: Option<String>,
    /// The harness needs new ptys. When its program ignores paddock's pty
    /// library on Seatbelt, the run says so in one line. `--backend auto`
    /// still picks Seatbelt: the VM only when the user asks for it.
    #[serde(default)]
    pub needs_pty: bool,
    /// `[[macos.tmp]]`: fixed names in `/private/tmp` that the harness uses
    /// in a Seatbelt run (tmpclaim.rs).
    #[serde(default)]
    pub tmp: Vec<crate::tmpclaim::Want>,
}

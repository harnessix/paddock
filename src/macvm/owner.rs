//! Which guest process belongs to the app of the session. In network mode
//! `ask` only the app may ask: a process of the guest OS (a daemon, a
//! widget, an Apple service) follows a fixed policy and never makes a
//! question.
//!
//! A connection belongs to the app when the identity that the guest proxy
//! found names it:
//! - its bundle identifier is the app's, or starts with the app's and a
//!   dot (`com.example.notes.helper`),
//! - its outermost app (`parent_app`, a child of the app such as a shell
//!   or a `curl`) is the app, by the same test,
//! - or its executable lies inside the copy of the app bundle in the guest
//!   (an XPC service of the bundle, which launchd starts).
//!
//! An unknown identity belongs to nobody. Pure, with Linux tests.

use super::proxy_header::Identity;
use std::path::Path;

/// The app of a session, as the firewall tells it from the guest OS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppOwner {
    /// `CFBundleIdentifier` of the app, empty when unknown.
    bundle_id: String,
    /// The bundle folders of the app in the guest, without a trailing `/`.
    bundles: Vec<String>,
    /// More signing identifiers that count as the app: only
    /// [`BENCH_TOOLS`] of a `--bench` session.
    also: Vec<String>,
}

/// The tools that count as the app in a `--bench` session, so a Mac check
/// can ask with `curl` from `paddock macvm bench spawn-child`: the child
/// of the guest agent is no process of the app.
pub const BENCH_TOOLS: [&str; 1] = ["com.apple.curl"];

impl AppOwner {
    pub fn new(bundle_id: &str, bundles: &[&Path]) -> Self {
        Self {
            bundle_id: bundle_id.trim().to_string(),
            bundles: bundles
                .iter()
                .map(|p| p.to_string_lossy().trim_end_matches('/').to_string())
                .filter(|p| p.len() > 1)
                .collect(),
            also: Vec::new(),
        }
    }

    /// The owner of a `--bench` session: the app and [`BENCH_TOOLS`].
    pub fn with_bench_tools(mut self) -> Self {
        self.also = BENCH_TOOLS.iter().map(|s| s.to_string()).collect();
        self
    }

    /// An owner that owns nothing: every connection of the session is the
    /// guest OS's.
    pub fn nobody() -> Self {
        Self::default()
    }

    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }

    fn is_app(&self, bundle: &str) -> bool {
        !self.bundle_id.is_empty()
            && (bundle == self.bundle_id
                || bundle
                    .strip_prefix(self.bundle_id.as_str())
                    .is_some_and(|rest| rest.starts_with('.')))
    }

    fn in_bundle(&self, path: &str) -> bool {
        self.bundles.iter().any(|b| {
            path.strip_prefix(b.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
                && !path.contains("/../")
        })
    }

    /// Whether the guest process of this identity is the app or one of
    /// its own processes.
    pub fn owns(&self, id: Option<&Identity>) -> bool {
        let Some(id) = id else {
            return false;
        };
        id.app
            .as_deref()
            .is_some_and(|b| self.is_app(b) || self.also.iter().any(|a| a == b))
            || id.parent_app.as_deref().is_some_and(|b| self.is_app(b))
            || id.path.as_deref().is_some_and(|p| self.in_bundle(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> AppOwner {
        AppOwner::new(
            "com.example.notes",
            &[
                Path::new("/Users/paddock/Applications/Sample Notes.app/"),
                Path::new("/Volumes/paddock/apps/Sample Notes.app"),
            ],
        )
    }

    fn id(app: Option<&str>, parent: Option<&str>, path: Option<&str>) -> Identity {
        Identity {
            pid: Some(5),
            app: app.map(str::to_string),
            parent_app: parent.map(str::to_string),
            path: path.map(str::to_string),
            team: None,
        }
    }

    #[test]
    fn the_app_its_helpers_and_its_children_belong_to_it() {
        let o = owner();
        assert!(o.owns(Some(&id(Some("com.example.notes"), None, None))));
        assert!(o.owns(Some(&id(Some("com.example.notes.helper"), None, None))));
        // A shell or a curl of the app: the outermost app is the app.
        assert!(o.owns(Some(&id(
            Some("com.apple.curl"),
            Some("com.example.notes"),
            None
        ))));
        assert!(o.owns(Some(&id(None, None, Some("/Users/paddock/Applications/Sample Notes.app/Contents/XPCServices/x.xpc/Contents/MacOS/x")))));
    }

    #[test]
    fn the_guest_os_and_the_unknown_do_not() {
        let o = owner();
        for i in [
            id(
                Some("com.apple.weather.widget"),
                None,
                Some("/System/Applications/Weather.app/Contents/MacOS/Weather"),
            ),
            id(
                Some("com.apple.nsurlsessiond"),
                None,
                Some("/usr/libexec/nsurlsessiond"),
            ),
            id(Some("com.example.notesgpt-other"), None, None),
            id(Some("com.example"), None, None),
            id(
                None,
                None,
                Some("/Users/paddock/Applications/Sample Notes.app.evil/x"),
            ),
            id(
                None,
                None,
                Some("/Users/paddock/Applications/Sample Notes.app/../Evil.app/x"),
            ),
            Identity::default(),
        ] {
            assert!(!o.owns(Some(&i)), "{i:?}");
        }
        assert!(!o.owns(None));
        assert!(!AppOwner::nobody().owns(Some(&id(Some("com.example.notes"), None, None))));
        // No bundle id: only the path tells.
        let p = AppOwner::new("", &[Path::new("/A.app"), Path::new("/")]);
        assert!(!p.owns(Some(&id(Some(""), None, None))));
        assert!(p.owns(Some(&id(None, None, Some("/A.app/Contents/MacOS/A")))));
        assert!(
            !p.owns(Some(&id(None, None, Some("/usr/bin/curl")))),
            "/ is no bundle"
        );
        assert_eq!(owner().bundle_id(), "com.example.notes");
    }

    #[test]
    fn a_bench_session_also_owns_curl_and_nothing_else() {
        let curl = id(Some("com.apple.curl"), None, Some("/usr/bin/curl"));
        assert!(!owner().owns(Some(&curl)));
        let o = owner().with_bench_tools();
        assert!(o.owns(Some(&curl)));
        assert!(!o.owns(Some(&id(Some("com.apple.weather.widget"), None, None))));
    }
}

//! One test per function of text.rs, named as the function: the refusals,
//! the setup, the start lines and the badges. text_more_tests.rs has the
//! rest, and text_rules_tests.rs the rules over all of them.

use super as t;
use super::{GoldenSize, Holder, Share};
use super::{SetupPhase, StartKind, BADGE_MAX};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(super) const GIB: u64 = 1 << 30;
pub(super) const SIZE: GoldenSize = GoldenSize {
    download_gb: 15,
    disk_gb: 25,
};

pub(super) fn has(text: &str, names: &[&str]) {
    for n in names {
        assert!(text.contains(n), "{text:?} does not name {n:?}");
    }
}

pub(super) fn badge(text: &str) {
    assert!(
        text.chars().count() <= BADGE_MAX,
        "badge {text:?} is longer than {BADGE_MAX}"
    );
    assert!(!text.contains('\n'), "{text:?}");
}

/// The texts in the order of their sentences: what happened, then what to
/// do. A refusal has two sentences at least.
fn two_sentences(text: &str) {
    let parts = text.split(". ").count();
    assert!(parts >= 2, "{text:?} has no second sentence");
}

// ---- Refusals ----------------------------------------------------------

#[test]
fn no_golden() {
    let s = t::no_golden("ChatGPT", SIZE, 30);
    assert_eq!(
        s,
        "ChatGPT runs in paddock's macOS guest. The guest needs a one-time setup of about 30 minutes (15 GB download, 25 GB on disk): paddock macvm setup"
    );
}

#[test]
fn golden_unsealed() {
    let s = t::golden_unsealed();
    has(&s, &["did not finish", "paddock macvm setup"]);
    two_sentences(&s);
}

#[test]
fn third_session() {
    let s = t::third_session(&[
        Holder::Mine {
            app: "chatgpt-desktop".into(),
            pid: 812,
        },
        Holder::User { name: "ana".into() },
    ]);
    assert_eq!(
        s,
        "two macOS VMs already run on this Mac: chatgpt-desktop (you, pid 812) and a VM of user ana. Stop one (paddock app stop chatgpt-desktop), then try again"
    );
    let other = t::third_session(&[]);
    has(&other, &["another program", "Tart", "Stop one"]);
    let three = t::third_session(&[
        Holder::User { name: "a".into() },
        Holder::User { name: "b".into() },
        Holder::OtherProgram,
    ]);
    has(
        &three,
        &["user a, a VM of user b and a VM of another program"],
    );
    let one = t::third_session(&[Holder::User { name: "ana".into() }]);
    has(&one, &["user ana and one more VM"]);
}

#[test]
fn profile_running() {
    let s = t::profile_running("chatgpt-desktop", 812);
    has(
        &s,
        &[
            "chatgpt-desktop already runs (pid 812)",
            "paddock app stop chatgpt-desktop",
        ],
    );
}

#[test]
fn no_space() {
    let s = t::no_space(8 * GIB, GIB * 12 / 10, "the clone");
    has(
        &s,
        &["the clone", "needs 8 GiB", "1.2 GiB free", "paddock gc"],
    );
}

#[test]
fn mas_bundle() {
    let s = t::mas_bundle(Path::new("/Applications/Things.app"));
    has(
        &s,
        &[
            "/Applications/Things.app",
            "App Store",
            "Apple ID",
            "paddock app run",
        ],
    );
    two_sentences(&s);
}

#[test]
fn host_too_old() {
    let s = t::host_too_old("13.6");
    has(&s, &["macOS 13.6", "macOS 14", "Software Update"]);
}

#[test]
fn not_apple_silicon() {
    let s = t::not_apple_silicon();
    has(&s, &["Apple silicon", "Intel", "paddock run"]);
}

#[test]
fn memory_too_large() {
    let s = t::memory_too_large("vscode", 32, 16);
    has(
        &s,
        &[
            "32 GiB",
            "16 GiB",
            "vm_memory = \"8G\"",
            "[apps.vscode]",
            "config.toml",
        ],
    );
}

#[test]
fn env_file() {
    let s = t::env_file("vscode", Path::new("/Users/me/src/web/.env"));
    has(
        &s,
        &[
            "/Users/me/src/web/.env",
            "unprotected_workspaces",
            "[apps.vscode]",
        ],
    );
}

#[test]
fn share_refused() {
    let s = t::share_refused("vscode", Path::new("/Users/me"), "it is the home folder");
    has(
        &s,
        &["/Users/me", "the home folder", "paddock app run vscode"],
    );
}

#[test]
fn network_host_refused() {
    has(&t::network_host_refused(), &["host", "--network egress"]);
}

#[test]
fn ask_mode_elsewhere() {
    has(
        &t::ask_mode_elsewhere(),
        &["ask", "macOS guest", "--network egress"],
    );
}

#[test]
fn data_unmountable() {
    let s = t::data_unmountable("chatgpt-desktop", Some("2026-11-02T10-00"));
    has(
        &s,
        &[
            "does not mount",
            "paddock app restore chatgpt-desktop 2026-11-02T10-00",
        ],
    );
    let none = t::data_unmountable("chatgpt-desktop", None);
    has(
        &none,
        &[
            "no backup",
            "paddock app reset chatgpt-desktop --data",
            "login",
        ],
    );
}

// ---- Setup -------------------------------------------------------------

#[test]
fn fetch_question() {
    let url = "https://updates.cdn-apple.com/x/UniversalMac_15.2_Restore.ipsw";
    let s = t::fetch_question(url, "15.2", SIZE, 30);
    has(
        &s,
        &[
            url,
            "macOS 15.2",
            "15 GB download",
            "25 GB on disk",
            "30 minutes",
        ],
    );
    assert!(s.ends_with("Download and install? [y/N]"), "{s}");
}

#[test]
fn progress() {
    let s = t::progress(SetupPhase::Download, 35, Some(Duration::from_secs(400)));
    assert_eq!(s, "download 35%, about 7 min left");
    let s = t::progress(SetupPhase::Install, 100, Some(Duration::from_secs(0)));
    assert_eq!(s, "install 100%, about 0 s left");
    let s = t::progress(SetupPhase::Install, 5, Some(Duration::from_secs(3900)));
    has(&s, &["1 h 5 min left"]);
    assert!(t::progress(SetupPhase::Install, 120, Some(Duration::ZERO)).contains("100%"));
    assert_eq!(t::progress(SetupPhase::Install, 5, None), "install 5%");
}

#[test]
fn panel_stopped() {
    has(
        &t::panel_stopped(),
        &["keeps what is done", "paddock macvm setup"],
    );
}

#[test]
fn golden_ready() {
    let s = t::golden_ready("15.2", GIB * 234 / 10 + 1);
    has(
        &s,
        &["golden ready (macOS 15.2, 23.4 GiB)", "paddock app run"],
    );
}

#[test]
fn keyring_missing() {
    has(
        &t::keyring_missing(),
        &["keyring", "paddock macvm remove", "paddock macvm setup"],
    );
}

// ---- Start lines: each names its number and its unit --------------------

#[test]
fn start_backend() {
    let s = t::start_backend("the app uses Apple's App Sandbox");
    has(&s, &["macvm", "App Sandbox"]);
}

#[test]
fn start_memory() {
    has(
        &t::start_memory("chatgpt-desktop", 4),
        &["4 GiB", "vm_memory", "[apps.chatgpt-desktop]"],
    );
}

#[test]
fn start_display() {
    let s = t::start_display(3024, 1964, 2, false);
    has(&s, &["3024 x 1964 points", "2x"]);
    assert!(!s.contains("two scales"));
    has(
        &t::start_display(4480, 1440, 2, true),
        &["4480 x 1440 points", "two scales"],
    );
}

#[test]
fn start_shares() {
    let a = PathBuf::from("/Users/me/src/web");
    let b = PathBuf::from("/Users/me/src/api");
    let s = t::start_shares(
        "vscode",
        &[
            Share {
                path: &a,
                writable: true,
            },
            Share {
                path: &b,
                writable: false,
            },
        ],
    );
    has(
        &s,
        &[
            "2 folder(s)",
            "/Users/me/src/web (rw)",
            "/Users/me/src/api (ro)",
        ],
    );
    has(
        &t::start_shares("vscode", &[]),
        &["0 folders", "paddock app run vscode"],
    );
}

#[test]
fn start_downloads() {
    let p = "/Users/me/Library/Application Support/paddock/macvm/profiles/chatgpt-desktop/default/downloads";
    has(&t::start_downloads(Path::new(p)), &["downloads", p]);
}

#[test]
fn start_network() {
    has(&t::start_network("ask"), &["network   ask", "asks before"]);
    for m in ["egress", "api", "allowlist", "none"] {
        let s = t::start_network(m);
        assert!(s.starts_with(&format!("network   {m}: ")), "{s}");
    }
    assert_eq!(t::start_network("other"), "network   other");
}

#[test]
fn start_kind() {
    assert_eq!(
        t::start_kind(StartKind::Cold, 40, None),
        "start     cold start, about 40 s"
    );
    has(
        &t::start_kind(StartKind::Cold, 40, Some("golden changed")),
        &["cold start, about 40 s (golden changed)"],
    );
    has(
        &t::start_kind(StartKind::Warm, 6, None),
        &["warm start, about 6 s"],
    );
}

#[test]
fn start_copy() {
    has(
        &t::start_copy("ChatGPT", 20),
        &["copy of ChatGPT", "20 s", "once per version"],
    );
}

#[test]
fn start_continues() {
    let s = t::start_continues("chatgpt-desktop");
    has(
        &s,
        &[
            "continues after this terminal closes",
            "paddock app stop chatgpt-desktop",
        ],
    );
}

// ---- Badges --------------------------------------------------------------

#[test]
fn guest_not_answering() {
    assert_eq!(t::guest_not_answering(), "guest not answering");
    badge(&t::guest_not_answering());
}

#[test]
fn restarting_guest() {
    assert_eq!(t::restarting_guest(), "restarting the guest");
    badge(&t::restarting_guest());
}

#[test]
fn waking() {
    assert_eq!(t::waking(), "waking");
}

#[test]
fn display_changed() {
    assert_eq!(
        t::display_changed(),
        "display changed, restart for full size"
    );
    badge(&t::display_changed());
}

#[test]
fn layout_missing() {
    assert_eq!(
        t::layout_missing("Dvorak"),
        "keyboard layout Dvorak not in the guest"
    );
    let long = t::layout_missing("A very long third party layout name");
    badge(&long);
    has(&long, &["A very long t…", "not in the guest"]);
}

#[test]
fn input_source() {
    assert_eq!(
        t::input_source("Pinyin - Simplified"),
        "input: Pinyin - Simplified"
    );
    badge(&t::input_source(&"漢".repeat(80)));
}

#[test]
fn full_screen_refused() {
    assert_eq!(
        t::full_screen_refused(),
        "full screen is not available in the guest"
    );
    badge(&t::full_screen_refused());
}

#[test]
fn disk_warn() {
    assert_eq!(t::disk_warn(GIB * 19 / 10), "disk low: 1.9 GiB free");
    badge(&t::disk_warn(u64::MAX));
}

#[test]
fn image_too_large() {
    badge(&t::image_too_large());
    has(&t::image_too_large(), &["clipboard"]);
}

#[test]
fn keyboard_layout_not_set() {
    assert_eq!(t::keyboard_layout_not_set(), "keyboard layout not set");
}

#[test]
fn every_badge_is_short() {
    let long = "x".repeat(200);
    for b in [
        t::guest_not_answering(),
        t::restarting_guest(),
        t::waking(),
        t::display_changed(),
        t::drop_failed(),
        t::dropped_as_open(),
        t::layout_missing(&long),
        t::input_source(&long),
        t::full_screen_refused(),
        t::disk_warn(u64::MAX),
        t::image_too_large(),
        t::keyboard_layout_not_set(),
    ] {
        badge(&b);
    }
}

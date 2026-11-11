//! Which route `paddock app run NAME` takes on macOS: paddock's own macOS
//! guest VM, or the older Seatbelt route. Pure, so the tests run on Linux
//! (`app::tests::backend_auto_cases`).

use crate::profile::Profile;
use anyhow::{bail, Result};

/// Which route `paddock app run NAME` takes on macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacRoute {
    /// paddock's own macOS guest VM, where the app runs under Apple's
    /// own sandbox and each of its windows is a host window.
    Guest,
    /// Seatbelt, the route of the apps that ran before the guest. It gets
    /// no new work.
    Seatbelt,
}

/// The route of an app profile on macOS. `golden` is the state of the
/// golden guest, None when this Mac has none.
///
/// No `--backend` and `--backend auto` pick the guest for every app profile
/// once the golden is sealed, and otherwise stop with the line that names
/// `paddock macvm setup`. `--backend seatbelt` keeps the old route. Any other
/// backend is a Linux one, and the Seatbelt route refuses it by name.
pub fn mac_route(
    asked: Option<&str>,
    app_label: &str,
    golden: Option<crate::macvm::golden::State>,
) -> Result<MacRoute> {
    use crate::macvm::golden::State;
    use crate::macvm::host::{GOLDEN_SIZE, SETUP_MINUTES};
    use crate::macvm::text;
    match asked {
        Some(b) if b == crate::macvm::host::BACKEND => Ok(MacRoute::Guest),
        None | Some("auto") => match golden {
            Some(State::Sealed) => Ok(MacRoute::Guest),
            Some(_) => bail!(text::golden_unsealed()),
            None => bail!(text::no_golden(app_label, GOLDEN_SIZE, SETUP_MINUTES)),
        },
        Some(_) => Ok(MacRoute::Seatbelt),
    }
}

/// The name of an app for a user line: its `display_name`, else the
/// profile name.
pub fn app_label(profile: &Profile) -> String {
    profile
        .app
        .as_ref()
        .map(|a| a.display_name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| profile.name.clone())
}

/// `paddock app run NAME` on macOS: the guest VM when the route says so,
/// else the Seatbelt route.
#[cfg(target_os = "macos")]
pub fn run_macos(env: &crate::env::HostEnv, o: &super::AppOptions) -> Result<i32> {
    let label = app_label(&super::app_profile(env, &o.name)?);
    let golden = crate::macvm::layout::Layout::new(env)
        .golden
        .read_manifest()
        .ok()
        .flatten()
        .map(|m| m.state);
    match mac_route(o.backend.as_deref(), &label, golden)? {
        MacRoute::Guest => crate::macvm::host::run_app(env, o),
        MacRoute::Seatbelt => crate::backend::seatbelt::run_app(env, o),
    }
}

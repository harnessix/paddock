//! `paddock-guest`: the agent inside paddock's macOS guest VM. launchd
//! runs it at login (`dev.paddock.guest`); it dials the host over vsock
//! and has no command line of its own. Exit 0 after an update or a
//! restart request, so launchd starts it again.
//!
//! The same binary is the link helper `Paddock Links.app`, the default
//! browser of the guest: run from the helper bundle, it hands the links of
//! Launch Services to the agent and exits (`guest/link.rs`).

fn main() {
    std::process::exit(run());
}

#[cfg(target_os = "macos")]
fn run() -> i32 {
    use paddock::guest::{log as guest_log, macos, main_loop, vsock, HOST_CID};
    use paddock::vm::ports::GUEST_CONTROL;
    use std::time::Duration;

    let exe = std::env::current_exe().unwrap_or_default();
    if paddock::guest::link::is_helper(&exe) {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return 1;
        };
        return paddock::guest::link_macos::helper_main(mtm);
    }
    let path = std::env::var_os("PADDOCK_GUEST_LOG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(guest_log::default_path);
    if let Err(e) = guest_log::init(path.clone()) {
        eprintln!("paddock-guest: cannot open the log {}: {e}", path.display());
    }
    log::info!(
        "paddock-guest {} starts, protocol {}",
        paddock::macvm::version::paddock_version(),
        paddock::macvm::proto::VERSION
    );
    // The start line and sw_vers on the serial console: logs/console.log.
    paddock::guest::console::announce(&paddock::macvm::text::console_start(
        paddock::macvm::version::paddock_version(),
        &paddock::macvm::proto::VERSION.to_string(),
        std::process::id(),
    ));
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        log::error!("paddock-guest must run on the main thread");
        return 1;
    };
    paddock::guest::proxy::start();
    // Computer use: its own port and threads (guest/computer_macos.rs).
    paddock::guest::computer_macos::approve_capture();
    paddock::guest::computer_macos::start();
    paddock::guest::link_macos::start(std::sync::Arc::new(main_loop::send_link));
    let _ = std::thread::Builder::new()
        .name("wallpaper".into())
        .spawn(|| {
            paddock::guest::wallpaper::set_white(std::path::Path::new(paddock::guest::SUPPORT_DIR))
        });
    let mut mac = macos::Mac::new(mtm);
    log::info!(
        "Accessibility {}",
        if macos::accessibility_granted(&mac) {
            "granted"
        } else {
            "not granted"
        }
    );
    main_loop::run(&mut mac, || {
        vsock::connect(HOST_CID, GUEST_CONTROL, Duration::from_secs(5))
    })
}

#[cfg(not(target_os = "macos"))]
fn run() -> i32 {
    eprintln!("paddock-guest runs only inside the macOS guest of paddock");
    1
}

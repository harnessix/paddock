//! `scope = "shared"` with the real systemd user manager of the host: the
//! transient unit, `systemctl --user stop`, and the socket unit of
//! `paddock device install`. The scenario is in tests/common/
//! unit_scenario.rs, and tests/devices_stress.rs runs it twice at once.
//!
//! A host without a systemd user instance skips the test. packaging/macos/checks/devices-shared.sh runs it on a real host.

mod common;

/// With a systemd user manager: the transient unit exists while the
/// instance runs, `systemctl --user stop` ends it and the next request
/// starts it again, and `paddock device install` writes the socket unit,
/// after which the first connection starts the service with no
/// `systemd-run` from a broker.
#[test]
fn with_a_systemd_user_manager_the_unit_owns_the_instance() {
    common::unit_scenario::unit_owns_instance();
}

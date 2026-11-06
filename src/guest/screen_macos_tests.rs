//! The lock calls of this Mac: the session dictionary and the idle time
//! read without a crash. The lock itself (`lock_now`) is not called here:
//! it would lock the screen of the person who runs the tests. The Mac
//! check `lock` of `macvm-session.sh` runs it in a guest.

use super::*;

#[test]
fn the_lock_state_and_the_idle_time_read() {
    // A test run happens in an unlocked session, or over ssh with no
    // window server session: both read as unlocked.
    assert!(!locked());
    if let Some(s) = idle_secs() {
        assert!(s < 10 * 365 * 24 * 3600, "{s}");
    }
}

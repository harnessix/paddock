//! The Text Input Sources calls of this Mac. The test harness runs each
//! test on a thread that is not the main thread.

use super::*;

#[test]
fn key_char_off_the_main_thread_calls_no_tis_and_gives_none() {
    // A thread of computer use, as "computer-conn" is. Since macOS 14 a
    // TIS call here asserts and ends the process, so the test process
    // would end before the assert below.
    let r = std::thread::Builder::new()
        .name("computer-conn".into())
        .spawn(|| key_char(44, true))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(r, None);
}

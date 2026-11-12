use super::*;
use std::cell::Cell;

#[path = "../tests/fixtures/launcher-protocol/released_0_2_1_root.rs"]
mod released_0_2_1_root;

const MAC_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/launcher-protocol/mac-v1.json");
const OTHER_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/launcher-protocol/other-v1.json");

fn args(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

/// Runs `dispatch` and counts the calls of the normal start.
fn run(words: &[&str]) -> (i32, Vec<u8>, Vec<u8>, u32) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let starts = Cell::new(0);
    let code = dispatch(&args(words), &mut out, &mut err, || {
        starts.set(starts.get() + 1);
        17
    });
    (code, out, err, starts.get())
}

#[test]
fn the_reply_bytes_are_the_fixtures() {
    assert_eq!(reply(MAC_FEATURES), MAC_FIXTURE);
    assert_eq!(reply(&[]), OTHER_FIXTURE);
    for bytes in [MAC_FIXTURE, OTHER_FIXTURE] {
        assert!(bytes.len() <= MAX_REPLY);
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
        assert!(bytes.ends_with(b"\n"));
    }
}

#[test]
fn this_build_answers_with_its_own_features() {
    let (code, out, err, starts) = run(&["paddock", "--launcher-protocol"]);
    assert_eq!((code, starts), (0, 0));
    assert!(err.is_empty());
    let expected = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        MAC_FIXTURE
    } else {
        OTHER_FIXTURE
    };
    assert_eq!(out, expected);
}

#[test]
fn the_reply_is_one_object_with_a_protocol_and_sorted_unique_features() {
    let value: serde_json::Value = serde_json::from_slice(MAC_FIXTURE).unwrap();
    let object = value.as_object().unwrap();
    assert_eq!(object.len(), 2);
    assert_eq!(object["protocol"], PROTOCOL);
    let features: Vec<&str> = object["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    let mut sorted = features.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(features, sorted);
    assert_eq!(features, MAC_FEATURES);
}

#[test]
fn completeness_is_not_claimed_while_app_list_has_no_profiles_complete() {
    // `app list --json` (src/cli/app_list.rs) prints `v` and `profiles`
    // only. List this feature when the output carries `profiles_complete`.
    assert!(!MAC_FEATURES.contains(&"app-list-v1-completeness"));
    let source = include_str!("cli/app_list.rs");
    assert!(!source.contains("profiles_complete"));
}

#[test]
fn the_option_with_other_root_arguments_is_refused_without_a_start() {
    for words in [
        &["paddock", "--launcher-protocol", "--help"][..],
        &["paddock", "--launcher-protocol", "run"],
        &["paddock", "--launcher-protocol", "claude"],
        &["paddock", "--launcher-protocol", "--launcher-protocol"],
        &["paddock", "-v", "--launcher-protocol"],
        &["paddock", "--quiet", "--launcher-protocol"],
        &["paddock", "--help", "--launcher-protocol"],
        &["paddock", "--launcher-protocol="],
        &["paddock", "--launcher-protocol=1"],
    ] {
        let (code, out, err, starts) = run(words);
        assert_eq!((code, starts), (2, 0), "{words:?}");
        assert!(out.is_empty(), "{words:?}");
        assert_eq!(
            err, b"paddock: --launcher-protocol must be the only argument\n",
            "{words:?}"
        );
    }
}

#[test]
fn other_command_lines_take_the_normal_start() {
    for words in [
        &["paddock"][..],
        &["paddock", "--version"],
        &["paddock", "--launcher-protocols"],
        &["paddock", "--launcher-protocol-other"],
        &["paddock", "claude", "--launcher-protocol"],
        &["paddock", "run", "codex", "--", "--launcher-protocol"],
        &["paddock", "--", "--launcher-protocol"],
        &["paddock", "-", "--launcher-protocol"],
    ] {
        let (code, out, err, starts) = run(words);
        assert_eq!((code, starts), (17, 1), "{words:?}");
        assert!(out.is_empty() && err.is_empty(), "{words:?}");
    }
}

#[test]
fn a_closed_stdout_is_a_failure_and_no_start() {
    struct Closed;
    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    struct FlushFails;
    impl Write for FlushFails {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }
    let argv = args(&["paddock", "--launcher-protocol"]);
    let mut err = Vec::new();
    let code = dispatch(&argv, &mut Closed, &mut err, || panic!("normal start"));
    assert_eq!(code, 1);
    let code = dispatch(&argv, &mut FlushFails, &mut err, || panic!("normal start"));
    assert_eq!(code, 1);
    assert!(err.is_empty());
}

#[test]
fn the_released_0_2_1_parser_rejects_the_option_before_a_harness_starts() {
    use clap::error::ErrorKind;
    let starts = Cell::new(0);
    let result = released_0_2_1_root::dispatch(&["paddock", "--launcher-protocol"], |_| {
        starts.set(starts.get() + 1)
    });
    assert_eq!(result, Err(ErrorKind::UnknownArgument));
    assert_eq!(starts.get(), 0);
    // The control: the fixture does dispatch an unknown word to a harness.
    let result = released_0_2_1_root::dispatch(&["paddock", "fake-agent"], |argv| {
        assert_eq!(argv, ["fake-agent"]);
        starts.set(starts.get() + 1);
    });
    assert_eq!(result, Ok(()));
    assert_eq!(starts.get(), 1);
}

#[test]
fn the_parser_of_this_release_also_rejects_the_option() {
    use clap::{error::ErrorKind, Parser};
    // This parser is the one of 0.2.2, which has no early answer. The full
    // command tree needs the stack size of the main thread.
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(|| {
            for words in [
                &["paddock", "--launcher-protocol"][..],
                &["paddock", "-v", "--launcher-protocol"],
                &["paddock", "--launcher-protocol", "claude"],
            ] {
                let Err(e) = crate::cli::Cli::try_parse_from(words) else {
                    panic!("{words:?} parsed");
                };
                assert_eq!(e.kind(), ErrorKind::UnknownArgument, "{words:?}");
            }
            // The stderr of 0.2.2, which the launcher replays as "no answer".
            let e = crate::cli::Cli::try_parse_from(["paddock", "--launcher-protocol"])
                .err()
                .unwrap();
            assert_eq!(e.exit_code(), 2);
            assert_eq!(
                e.render().to_string(),
                include_str!("../tests/fixtures/launcher-protocol/unknown-option.err")
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

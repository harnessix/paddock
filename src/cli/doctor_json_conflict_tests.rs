use super::Cli;
use clap::Parser;

#[test]
fn json_rejects_explicit_backend_and_device() {
    // The complete command tree needs the same stack as the main thread.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            for args in [
                vec!["paddock", "doctor", "--json", "--backend", "auto"],
                vec!["paddock", "doctor", "--json", "--device", "x"],
            ] {
                assert!(Cli::try_parse_from(args).is_err());
            }
            assert!(Cli::try_parse_from(["paddock", "doctor", "--json"]).is_ok());
        })
        .unwrap()
        .join()
        .unwrap();
}

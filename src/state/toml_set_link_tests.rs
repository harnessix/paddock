use super::*;
use std::os::unix::fs::symlink;

#[test]
fn never_writes_through_a_planted_link() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let outside = t.path().join("outside");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let profile: Profile = toml::from_str(
        "name = \"c\"\n[[state.toml_set]]\nfile = \".codex/config.toml\"\npath = [\"projects\", \"{project}\", \"trust_level\"]\nvalue = \"trusted\"\n",
    )
    .unwrap();
    // A folder link: refused, nothing created outside.
    symlink(&outside, home.join(".codex")).unwrap();
    assert!(toml_set(&profile, &home, Path::new("/w/app"), false).is_err());
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    // A file link: refused to read; the host file stays as it was.
    std::fs::remove_file(home.join(".codex")).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(outside.join("rc"), "keep").unwrap();
    symlink(outside.join("rc"), home.join(".codex/config.toml")).unwrap();
    assert!(toml_set(&profile, &home, Path::new("/w/app"), false).is_err());
    assert_eq!(std::fs::read_to_string(outside.join("rc")).unwrap(), "keep");
}

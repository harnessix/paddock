//! Which device mounts count as stale. No FUSE: the mount table is text.
use super::*;

/// One mountinfo line of a paddock device mount at `mp`.
fn line(mp: &Path) -> String {
    format!(
        "120 30 0:61 / {} rw,nosuid,nodev,noexec,noatime - fuse.paddock paddock rw,user_id=1000,group_id=1000\n",
        mp.display()
    )
}

/// The pid of a process that ended.
fn dead_pid() -> u32 {
    let mut c = std::process::Command::new("true").spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

#[test]
fn a_mount_whose_sandbox_is_still_starting_is_not_stale() {
    let t = tempfile::tempdir().unwrap();
    let run = t.path();
    let registered = run.join("claude-spawn-aaaa/dev");
    let starting = run.join("claude-spawn-bbbb/dev");
    let killed = run.join("claude-spawn-cccc/dev");
    let unmarked = run.join("claude-spawn-dddd/dev");
    let srt = run.join("devices/claude-spawn-eeee");
    // The sibling that starts at the same moment: mounted and marked,
    // not registered yet.
    claim(&starting).unwrap();
    claim(&srt).unwrap();
    crate::privfs::write(&owner_file(&killed), dead_pid().to_string()).unwrap();
    let mut info = String::from("25 1 0:22 / / rw,relatime - ext4 /dev/sda1 rw\n");
    for mp in [&registered, &starting, &killed, &unmarked, &srt] {
        info.push_str(&line(mp));
    }
    let running = vec!["claude-spawn-aaaa".to_string()];
    assert_eq!(stale_mounts(&info, run, &running), vec![killed, unmarked]);

    // Once the owner releases its mark, a mount that stays is stale.
    release(&starting);
    assert!(!owner_file(&starting).exists());
    assert!(stale_mounts(&info, run, &running).contains(&starting));
}

#[test]
fn the_owner_mark_sits_next_to_the_mount_folder() {
    let p = Path::new("/home/demo/.local/state/paddock/run/x/dev");
    assert_eq!(
        owner_file(p),
        Path::new("/home/demo/.local/state/paddock/run/x/dev.owner")
    );
    let p = Path::new("/home/demo/run/devices/x");
    assert_eq!(owner_file(p), Path::new("/home/demo/run/devices/x.owner"));
}

#[test]
fn mounts_outside_the_run_folder_and_other_file_systems_stay() {
    let t = tempfile::tempdir().unwrap();
    let run = t.path().join("run");
    let other = t.path().join("elsewhere/x/dev");
    let info = format!(
        "{}121 30 0:62 / {} rw - fuse.sshfs host: rw\n",
        line(&other),
        run.join("y/dev").display()
    );
    assert!(stale_mounts(&info, &run, &[]).is_empty());
}

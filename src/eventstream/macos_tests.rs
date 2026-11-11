//! Tests that only a Mac can run: the short socket folders of macOS under
//! /tmp/paddock-<uid>, and the doctor JSON of a real Mac.

use super::bus::Follower;
use super::schema::{Disk, Event, Line};
use super::Bus;
use std::time::Duration;

/// An env of the real user with a long home and no XDG_RUNTIME_DIR, as on
/// a Mac: the run folder is under the state folder, too long for a socket.
fn mac_env(t: &std::path::Path) -> crate::env::HostEnv {
    let uid = unsafe { libc::getuid() };
    let home = t.join("a-long-home-folder-name-for-the-socket-path-limit-of-macos");
    crate::env::HostEnv::from_parts(home, "demo".into(), uid, 20, Default::default())
}

#[test]
fn the_bus_lives_in_the_short_socket_folder_and_carries_a_line() {
    let t = tempfile::tempdir().unwrap();
    let env = mac_env(t.path());
    let bus = Bus::of(&env);
    let top = format!("/tmp/paddock-{}", env.uid);
    assert!(bus.dir().starts_with(&top), "{:?}", bus.dir());
    let f = Follower::bind(&bus, std::process::id()).unwrap();
    assert!(f.path().as_os_str().len() < crate::sockpath::MACOS_MAX);
    let (tx, rx) = crate::eventstream::bus::queue();
    f.serve(tx).unwrap();
    let ev = Event::Disk(Disk {
        dir: "/m".into(),
        free: 3,
    });
    assert_eq!(bus.send(&Line::at(1, ev.clone()).to_text()), 1);
    let got = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let l: Line = serde_json::from_str(&got).unwrap();
    assert_eq!(l.event, ev);
    drop(f);
    assert!(bus.followers().is_empty());
}

#[test]
fn doctor_json_reads_the_chip_the_memory_and_the_version() {
    let t = tempfile::tempdir().unwrap();
    let env = mac_env(t.path());
    let (view, facts) = crate::doctor::macvm::json::collect(&env);
    assert!(facts.chip.is_some(), "machdep.cpu.brand_string");
    assert!(facts.memory.unwrap_or(0) >= 4 << 30, "hw.memsize");
    assert!(view.host_version.is_some(), "sw_vers");
    let j = crate::doctor::macvm::json::build(&view, &facts);
    assert_eq!(j["v"], 1);
    assert!(j["macos"]["version"].is_string());
    assert!(j["disk"]["free"].as_u64().unwrap() > 0);
}

#[test]
fn short_runtime_directory_keeps_event_socket_inside_it() {
    let t = tempfile::Builder::new()
        .prefix("pdk-")
        .tempdir_in("/tmp")
        .unwrap();
    let run = t.path().join("run");
    let bus = crate::eventstream::bus::Bus::at(crate::sockpath::socket_dir(
        &run,
        unsafe { libc::getuid() },
        "events",
    ));
    let follower = crate::eventstream::bus::Follower::bind(&bus, std::process::id()).unwrap();
    assert!(follower.path().starts_with(&run));
}

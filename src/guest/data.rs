//! The data disk of a profile. On `data.prepare` the agent finds the second
//! virtio disk by its serial `paddock-data`, mounts its APFS volume `Data`
//! at `/Volumes/paddock`, moves the home of the account there, installs the
//! launchd daemon that mounts it on later boots, and leaves a symbolic link
//! at `/Users/paddock`. The host formats a new data disk before the first
//! boot (`macvm::data_disk`), so the agent finds a formatted, empty volume
//! and only moves the home onto it; the disk of an older profile can still
//! be blank, and the agent then makes the APFS container itself. The
//! commands run with `sudo -S`, the password on stdin. The parsers of the
//! `ioreg` and `diskutil` output are pure and tested on Linux.

use super::configure::{spawn, Plan, SUDO};
use super::data_copy::{self, Watch};
use super::platform::Fail;
use crate::macvm::proto::ErrorCode;
use crate::macvm::text;
use std::path::Path;

/// The serial of the data disk, as the host configures it.
pub use crate::macvm::proto::DATA_DISK_SERIAL as SERIAL;
/// The volume name.
pub const VOLUME: &str = "Data";
/// The mount point.
pub const MOUNT: &str = "/Volumes/paddock";
/// The home on the data disk.
pub const HOME: &str = "/Volumes/paddock/home";
/// The copy of the home while it is made: renamed to [`HOME`] when
/// complete, so a copy that broke off is never taken for the home.
pub const HOME_PARTIAL: &str = "/Volumes/paddock/home.partial";
/// The account of the guest.
pub const USER_HOME: &str = "/Users/paddock";
/// The file that names the profile of the disk.
pub const ID_FILE: &str = "/Volumes/paddock/.paddock-data-id";
/// Present while a session runs; still there at a start after an
/// unclean end, which makes the agent verify the volume.
pub const SESSION_MARK: &str = "/Volumes/paddock/.paddock-session";
/// The stamp on the system disk: this profile prepared its data disk.
pub const PREPARED_STAMP: &str = "/Library/Application Support/paddock/data-prepared";
/// The daemon that mounts the volume before login.
pub const DAEMON: &str = "/Library/LaunchDaemons/dev.paddock.data.plist";
/// The downloads folder of the host share.
pub const DOWNLOADS: &str = "/Volumes/My Shared Files/downloads";

/// The whole disk (`disk2`) whose storage device has the serial, from
/// the text of `ioreg -r -c IOBlockStorageDevice -l -w 0`.
pub fn find_disk(ioreg: &str, serial: &str) -> Option<String> {
    let mut device_matches = false;
    for line in ioreg.lines() {
        let t = line.trim_start_matches([' ', '|', '+', '-', 'o']).trim();
        if line.contains("+-o ") && line.contains("IOBlockStorageDevice") {
            device_matches = false;
        }
        // The serial is a key of its own on some systems, and inside the
        // "Device Characteristics" dictionary on one line on others
        // (`{"Serial Number"="paddock-data",...}`, as macOS 27 prints it).
        if serial_on_line(t, serial) {
            device_matches = true;
        }
        if device_matches && t.starts_with("\"BSD Name\"") {
            // `disk2` or a partition `disk2s1`: the whole disk.
            let name = t.split('"').nth(3)?;
            let rest = name.strip_prefix("disk")?;
            let number = rest.split('s').next().unwrap_or("");
            if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
                return Some(format!("disk{number}"));
            }
        }
    }
    None
}

/// The data disk when no device carries the serial: a macOS guest does not
/// show the identifier that the host gives a virtio disk. The system disk
/// always holds the `iBootSystemContainer` that macOS boots from; the data
/// disk never does. Of the virtio whole disks without it, exactly one must
/// be left, else `None` (never a guess). The text is the output of
/// `ioreg -r -c IOBlockStorageDevice -l -w 0`.
pub fn find_disk_by_elimination(ioreg: &str) -> Option<String> {
    // Each device starts at a top-level `+-o` line (no indent).
    let mut disks: Vec<(Option<String>, bool)> = Vec::new();
    for line in ioreg.lines() {
        if line.starts_with("+-o ") {
            let storage = line.contains("StorageDevice");
            disks.push((None, !storage));
            continue;
        }
        let Some(cur) = disks.last_mut() else {
            continue;
        };
        if line.contains("iBootSystemContainer") {
            cur.1 = true;
        }
        let t = line.trim_start_matches([' ', '|', '+', '-', 'o']).trim();
        if cur.0.is_none() && t.starts_with("\"BSD Name\"") {
            if let Some(name) = t.split('"').nth(3) {
                let rest = name.strip_prefix("disk").unwrap_or("");
                if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                    cur.0 = Some(name.to_string());
                }
            }
        }
    }
    let left: Vec<String> = disks
        .into_iter()
        .filter(|(_, system)| !system)
        .filter_map(|(name, _)| name)
        .collect();
    match left.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// Whether an `ioreg` line names the serial, with or without spaces
/// around `=`.
fn serial_on_line(line: &str, serial: &str) -> bool {
    let squeezed: String = line.chars().filter(|c| *c != ' ').collect();
    squeezed.contains(&format!("\"SerialNumber\"=\"{serial}\""))
}

/// Whether `diskutil list /dev/diskN` shows a partition table on the
/// disk: a `_partition_scheme` row or an `Apple_APFS` row. A blank disk
/// shows neither. Pure; the test holds both shapes.
pub fn has_partitions(diskutil_list: &str) -> bool {
    diskutil_list
        .lines()
        .any(|l| l.contains("_partition_scheme") || l.contains("Apple_APFS"))
}

/// The APFS container on the disk, from `diskutil list /dev/diskN`: the
/// `Apple_APFS Container disk3` row gives `disk3`. Pure.
pub fn container(diskutil_list: &str) -> Option<String> {
    diskutil_list
        .lines()
        .filter(|l| l.contains("Apple_APFS"))
        .find_map(|l| {
            let mut words = l.split_whitespace();
            words.find(|w| *w == "Container")?;
            let c = words.next()?;
            let n = c.strip_prefix("disk")?;
            (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| c.to_string())
        })
}

/// The volume UUID from `diskutil info`. Pure.
pub fn volume_uuid(info: &str) -> Option<String> {
    info.lines().find_map(|l| {
        let v = l.trim().strip_prefix("Volume UUID:")?.trim();
        (v.len() == 36 && v.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
            .then(|| v.to_string())
    })
}

/// The entries that macOS puts at the root of any volume it mounts.
pub const SYSTEM_ENTRIES: &[&str] = &[
    ".fseventsd",
    ".Spotlight-V100",
    ".Trashes",
    ".TemporaryItems",
    ".DocumentRevisions-V100",
    ".DS_Store",
    ".VolumeIcon.icns",
    ".PKInstallSandboxManager-SystemSoftware",
];

/// True for the root of a volume that holds nothing but what macOS puts
/// there, and at most a copy of the home that broke off: the volume that
/// the host made. Pure.
pub fn fresh(entries: &[String]) -> bool {
    let partial = HOME_PARTIAL.rsplit('/').next().unwrap_or_default();
    entries
        .iter()
        .all(|e| SYSTEM_ENTRIES.contains(&e.as_str()) || e.starts_with("._") || e == partial)
}

fn root_entries(dir: &str) -> Vec<String> {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        // Unreadable: not fresh, so nothing is written there.
        Err(_) => vec![String::from("?")],
    }
}

/// The launchd daemon that mounts the volume `id` (its UUID, or the name
/// `Data`) at the mount point. It does nothing when the mount point is
/// mounted already; otherwise it lets go of the mount that the system made
/// at boot, by the UUID only, then mounts.
pub fn daemon_plist(id: &str) -> String {
    let release = if id == VOLUME {
        String::new()
    } else {
        format!("/usr/sbin/diskutil unmount {id} >/dev/null 2>&amp;1; ") // not user text
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>dev.paddock.data</string>
  <key>RunAtLoad</key><true/>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>-c</string>
    <string>/sbin/mount | /usr/bin/grep -q " on {MOUNT} (" &amp;&amp; exit 0; /bin/mkdir -p {MOUNT}; {release}/usr/sbin/diskutil mount -mountPoint {MOUNT} {id}</string>
  </array>
</dict>
</plist>
"#
    )
}

fn root(argv: &[&str]) -> Plan {
    let mut v = vec![
        SUDO.to_string(),
        "-S".into(),
        "-p".into(),
        String::new(),
        "--".into(),
    ];
    v.extend(argv.iter().map(|s| s.to_string()));
    Plan {
        argv: v,
        password_on_stdin: true,
    }
}

fn user(argv: &[&str]) -> Plan {
    Plan {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        password_on_stdin: false,
    }
}

/// Run one plan; a failure is an `Io` error with the output.
fn run(plan: &Plan, password: &str) -> Result<String, Fail> {
    let stdin = plan.password_on_stdin.then_some(password);
    match spawn(plan, stdin) {
        Ok((true, out)) => Ok(out),
        Ok((false, out)) => Err((
            ErrorCode::Io,
            text::command_failed(&plan.argv[plan.argv.len().min(5)..].join(" "), &out),
        )),
        Err(e) => Err((
            ErrorCode::Io,
            text::cannot_run_command(&plan.argv.join(" "), &e.to_string()),
        )),
    }
}

/// The steps of `data.prepare`, as the error texts name them.
pub const STEP_FIND: &str = "find the data disk";
pub const STEP_LIST: &str = "read the partition table";
pub const STEP_PARTITION: &str = "partition the blank disk";
pub const STEP_MOUNT: &str = "mount the volume";
pub const STEP_COPY: &str = "copy the home onto the volume";
pub const STEP_LINK: &str = "point the account at the home";
pub const STEP_DAEMON: &str = "install the mount daemon";
pub const STEP_ID: &str = "write the data id";
pub const STEP_VERIFY: &str = "verify the volume";

/// The error of one step names the step: `mount the volume: ...`.
pub fn step<T>(name: &str, r: Result<T, Fail>) -> Result<T, Fail> {
    r.map_err(|(code, detail)| (code, text::data_step_failed(name, &detail)))
}

/// The whole output of `ioreg -r -c IOBlockStorageDevice -l -w 0`. Not
/// through `run`: that keeps only the last part of a long output, and the
/// device lines are at its start. ioreg needs no root.
fn storage_ioreg() -> Result<String, Fail> {
    let argv = ["-r", "-c", "IOBlockStorageDevice", "-l", "-w", "0"];
    let out = std::process::Command::new("/usr/sbin/ioreg")
        .args(argv)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            (
                ErrorCode::Io,
                text::cannot_run_command("/usr/sbin/ioreg", &e.to_string()),
            )
        })?;
    if !out.status.success() {
        return Err((
            ErrorCode::Io,
            text::command_failed("ioreg", &String::from_utf8_lossy(&out.stderr)),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// True when the volume is mounted at the mount point.
pub fn mounted() -> bool {
    Path::new(ID_FILE).exists() || Path::new(HOME).is_dir()
}

/// `data.prepare`: the verify output after the move. A prepared disk is left
/// as it is, whatever id the host names. A disk with a partition table is
/// never partitioned again, so its data is never lost: it is mounted, and
/// when it does not mount the reply is the error `locked`. Only a blank disk
/// is partitioned. A cancel of `watch` (the quit) stops the prepare before
/// its next step and during the copy of the home; from the link of the
/// account on, the steps run to their end, so the account always has a
/// home.
pub fn prepare(password: &str, data_id: &str, watch: &Watch) -> Result<String, Fail> {
    let r = prepare_disk(password, data_id, watch);
    if r.is_ok() {
        super::configure::machine_steps(password);
    }
    r
}

fn prepare_disk(password: &str, data_id: &str, watch: &Watch) -> Result<String, Fail> {
    watch.check()?;
    if mounted() && Path::new(USER_HOME).is_symlink() {
        let have = std::fs::read_to_string(ID_FILE).unwrap_or_default();
        if have.trim() == data_id {
            log::info!("the data disk {data_id} is prepared already");
        } else {
            log::warn!("{}", text::data_id_differs(have.trim(), data_id));
        }
        return verify(password);
    }
    let ioreg = step(STEP_FIND, storage_ioreg())?;
    let found = find_disk(&ioreg, SERIAL).or_else(|| {
        let d = find_disk_by_elimination(&ioreg);
        if let Some(d) = &d {
            log::info!("no disk carries the serial {SERIAL}: the data disk is {d}, the one virtio disk without the boot container");
        }
        d
    });
    if found.is_none() {
        // What the guest sees, for the report: the device and serial lines.
        for l in ioreg
            .lines()
            .filter(|l| l.contains("+-o ") || l.contains("Serial") || l.contains("BSD Name"))
            .take(40)
        {
            log::warn!("data disk search: {}", l.trim());
        }
    }
    let disk = step(
        STEP_FIND,
        found.ok_or((ErrorCode::Io, text::no_data_disk(SERIAL))),
    )?;
    log::info!("the data disk is /dev/{disk}");
    let dev = format!("/dev/{disk}");
    let listing = step(
        STEP_LIST,
        run(&user(&["/usr/sbin/diskutil", "list", &dev]), password),
    )?;
    if has_partitions(&listing) {
        // A formatted disk: mount it, never partition it. The system
        // mounts a readable disk at boot at `/Volumes/Data`, so the agent
        // lets go of that mount first; the device of the volume names it
        // without the name `Data`, which the system volume can carry too.
        let volume = container(&listing)
            .map(|c| format!("{c}s1")) // not user text
            .unwrap_or_else(|| VOLUME.to_string());
        if !mounted() {
            release(&volume, password);
            step(
                STEP_MOUNT,
                run(&root(&["/bin/mkdir", "-p", MOUNT]), password),
            )?;
            let mount = root(&["/usr/sbin/diskutil", "mount", "-mountPoint", MOUNT, &volume]);
            match spawn(&mount, Some(password)) {
                Ok((true, _)) if mounted() || Path::new(MOUNT).is_dir() => {}
                Ok((_, out)) => {
                    return step(
                        STEP_MOUNT,
                        Err((
                            ErrorCode::Locked,
                            text::data_disk_locked_guest(&last_line(&out)),
                        )),
                    )
                }
                Err(e) => {
                    return step(
                        STEP_MOUNT,
                        Err((
                            ErrorCode::Locked,
                            text::data_disk_locked_guest(&e.to_string()),
                        )),
                    )
                }
            }
        }
        if !Path::new(HOME).is_dir() {
            if !fresh(&root_entries(MOUNT)) {
                return step(
                    STEP_MOUNT,
                    Err((
                        ErrorCode::Locked,
                        text::data_disk_locked_guest("the volume mounts but holds no home"),
                    )),
                );
            }
            // The empty volume that the host made: the home moves onto it.
            log::info!("the data disk is formatted and empty: the home moves onto it");
            return populate(password, data_id, &volume, watch);
        }
        let have = std::fs::read_to_string(ID_FILE).unwrap_or_default();
        if have.trim() != data_id {
            log::warn!("{}", text::data_id_differs(have.trim(), data_id));
        }
        link_home(password, &volume)?;
        return verify(password);
    }
    // A blank disk of an older profile: the agent makes the container.
    watch.check()?;
    step(
        STEP_PARTITION,
        run(
            &root(&[
                "/usr/sbin/diskutil",
                "partitionDisk",
                &dev,
                "1",
                "GPT",
                "APFS",
                VOLUME,
                "100%",
            ]),
            password,
        ),
    )?;
    let listing = step(
        STEP_LIST,
        run(&user(&["/usr/sbin/diskutil", "list", &dev]), password),
    )?;
    let volume = container(&listing)
        .map(|c| format!("{c}s1")) // not user text
        .unwrap_or_else(|| VOLUME.to_string());
    release(&volume, password);
    step(
        STEP_MOUNT,
        run(&root(&["/bin/mkdir", "-p", MOUNT]), password),
    )?;
    step(
        STEP_MOUNT,
        run(
            &root(&["/usr/sbin/diskutil", "mount", "-mountPoint", MOUNT, &volume]),
            password,
        ),
    )?;
    populate(password, data_id, &volume, watch)
}

/// Let go of the mount that the system made at boot. Only by the device:
/// an unmount by the name `Data` could reach the system volume.
fn release(volume: &str, password: &str) {
    if volume != VOLUME {
        let _ = run(&root(&["/usr/sbin/diskutil", "unmount", volume]), password);
    }
}

/// Move the home onto the mounted, empty volume: the copy of the home,
/// the owners, the link, the Downloads link, the id file and the stamp.
fn populate(password: &str, data_id: &str, volume: &str, watch: &Watch) -> Result<String, Fail> {
    watch.check()?;
    // A volume that the host made has no owners in the guest's volume
    // database; the home needs them, as on a volume the guest made.
    if let Err((_, e)) = run(
        &root(&["/usr/sbin/diskutil", "enableOwnership", MOUNT]),
        password,
    ) {
        log::warn!("{e}");
    }
    let _ = run(&root(&["/bin/rm", "-rf", HOME_PARTIAL]), password);
    step(STEP_COPY, copy_home(password, watch))?;
    // The last point to stop: from here on the account moves to the copy.
    watch.check()?;
    link_home(password, volume)?;
    let downloads = format!("{HOME}/Downloads");
    let _ = run(&root(&["/bin/rm", "-rf", &downloads]), password);
    let _ = run(&root(&["/bin/ln", "-s", DOWNLOADS, &downloads]), password);
    step(
        STEP_ID,
        write_file(ID_FILE, &format!("{data_id}\n"), password),
    )?;
    if let Some(dir) = Path::new(PREPARED_STAMP).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(PREPARED_STAMP, format!("{data_id}\n"));
    // The move is complete. A quit that came meanwhile skips the verify,
    // so the shutdown does not wait for it; the next start verifies.
    watch.check()?;
    (watch.progress)(text::data_step_now(STEP_VERIFY), STEP_WAIT);
    verify(password)
}

/// The wait that the host gets for one step after the copy.
const STEP_WAIT: std::time::Duration = super::configure::STEP_TIMEOUT;

/// The copy of the home as `home.partial`, part by part with a time of
/// its own (`data_copy.rs`), owned by the account, then renamed to the
/// home: a copy that broke off is never the home.
fn copy_home(password: &str, watch: &Watch) -> Result<(), Fail> {
    use std::os::unix::fs::PermissionsExt;
    let home = Path::new(USER_HOME);
    let mode = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.permissions().mode())
            .unwrap_or(0o40750)
    };
    let entries = data_copy::read_entries(home).map_err(|e| {
        (
            ErrorCode::Io,
            text::cannot_read_named(USER_HOME, &e.to_string()),
        )
    })?;
    let lib = home.join("Library");
    let lib_entries = data_copy::read_entries(&lib).ok();
    let (ops, measure) = data_copy::plan(
        home,
        Path::new(HOME_PARTIAL),
        mode(home),
        &entries,
        lib_entries.as_deref().map(|e| (mode(&lib), e)),
    );
    // The size of the parts that are not guarded, for the time limit.
    (watch.progress)(
        text::data_step_now(STEP_COPY),
        data_copy::MEASURE_LIMIT + STEP_WAIT,
    );
    let mut du = vec!["/usr/bin/du".to_string(), "-sk".into(), "--".into()];
    du.extend(measure.iter().map(|m| m.to_string_lossy().into_owned()));
    let du = user(&du.iter().map(String::as_str).collect::<Vec<_>>());
    let measured =
        super::configure::run_plan_cancel(&du, None, data_copy::MEASURE_LIMIT, &watch.cancel);
    watch.check()?;
    let (sizes, total) = match measured {
        Ok(r) if !r.timed_out => {
            let sizes: std::collections::BTreeMap<_, _> = data_copy::du_sizes(&r.output)
                .into_iter()
                .map(|(p, kib)| (p, kib * 1024))
                .collect();
            let total = (!sizes.is_empty()).then(|| sizes.values().sum::<u64>());
            (sizes, total)
        }
        Ok(_) => {
            log::warn!("du did not end in {} s", data_copy::MEASURE_LIMIT.as_secs());
            Default::default()
        }
        Err(e) => {
            log::warn!("cannot run du: {e}");
            Default::default()
        }
    };
    let limit = data_copy::copy_limit(total);
    let measured = text::data_copy_measured(total, limit);
    log::info!("{measured}");
    (watch.progress)(measured, limit + STEP_WAIT);
    let report = data_copy::execute(&ops, &sizes, limit, watch, |plan, wait| {
        super::configure::run_plan_cancel(plan, Some(password), wait, &watch.cancel).map_err(|e| {
            (
                ErrorCode::Io,
                text::cannot_run_command(&plan.argv.join(" "), &e.to_string()),
            )
        })
    })?;
    let skipped: Vec<String> = report
        .skipped
        .iter()
        .map(|p| p.to_string_lossy().replacen(USER_HOME, "~", 1))
        .collect();
    let done = text::data_copy_done(report.parts, report.denied, &skipped);
    log::info!("{done}");
    (watch.progress)(done, STEP_WAIT * 2);
    let uid = format!("{}", unsafe { libc::getuid() });
    let chown = root(&["/usr/sbin/chown", "-R", &uid, HOME_PARTIAL]);
    match super::configure::run_plan_cancel(&chown, Some(password), limit, &watch.cancel) {
        Ok(r) if r.ok() => {}
        Ok(r) => return Err((ErrorCode::Io, text::command_failed("chown", &r.output))),
        Err(e) => {
            return Err((
                ErrorCode::Io,
                text::cannot_run_command("chown", &e.to_string()),
            ))
        }
    }
    watch.check()?;
    run(&root(&["/bin/mv", HOME_PARTIAL, HOME]), password)?;
    Ok(())
}

/// Write a small file as the user, or through `sudo cp` when the volume
/// root belongs to root.
fn write_file(path: &str, content: &str, password: &str) -> Result<(), Fail> {
    if std::fs::write(path, content).is_ok() {
        return Ok(());
    }
    let tmp = std::env::temp_dir().join("paddock-data-file");
    std::fs::write(&tmp, content).map_err(|e| {
        (
            ErrorCode::Io,
            text::cannot_write_named(path, &e.to_string()),
        )
    })?;
    let tmp_s = tmp.to_string_lossy().into_owned();
    let r = run(&root(&["/bin/cp", &tmp_s, path]), password)
        .and_then(|_| run(&root(&["/bin/chmod", "644", path]), password));
    let _ = std::fs::remove_file(&tmp);
    r.map(|_| ())
}

/// Point the account at the home on the data disk, leave the link at
/// `/Users/paddock`, and install the mount daemon. Every step is safe to
/// run again: nothing here copies into the home.
fn link_home(password: &str, volume: &str) -> Result<(), Fail> {
    step(STEP_LINK, link_account(password))?;
    // The user's preferences daemon still works with the home of the
    // session's start, which is gone now: every write of a preference
    // failed ("Could not write domain"). launchd starts a new one on the
    // next request, with the home on the data disk.
    if let Err(e) = run(&user(&["/usr/bin/killall", "cfprefsd"]), password) {
        log::warn!("{}", text::cfprefsd_restart_failed(&e.1));
    }
    step(STEP_DAEMON, install_daemon(password, volume))
}

/// The account's home is the one on the data disk, and `/Users/paddock`
/// is a link to it.
fn link_account(password: &str) -> Result<(), Fail> {
    run(
        &root(&[
            "/usr/bin/dscl",
            ".",
            "-create",
            USER_HOME,
            "NFSHomeDirectory",
            HOME,
        ]),
        password,
    )?;
    if !Path::new(USER_HOME).is_symlink() {
        let moved = format!("{USER_HOME}.moved");
        run(&root(&["/bin/mv", USER_HOME, &moved]), password)?;
        run(&root(&["/bin/ln", "-s", HOME, USER_HOME]), password)?;
        let _ = run(&root(&["/bin/rm", "-rf", &moved]), password);
    }
    Ok(())
}

/// The launchd daemon that mounts the volume before login.
fn install_daemon(password: &str, volume: &str) -> Result<(), Fail> {
    // The daemon mounts the volume by its UUID, which stays the same
    // across boots, unlike the device name.
    let id = run(&user(&["/usr/sbin/diskutil", "info", volume]), password)
        .ok()
        .and_then(|info| volume_uuid(&info))
        .unwrap_or_else(|| VOLUME.to_string());
    let plist = std::env::temp_dir().join("dev.paddock.data.plist");
    std::fs::write(&plist, daemon_plist(&id)).map_err(|e| {
        (
            ErrorCode::Io,
            text::cannot_write_named("the daemon plist", &e.to_string()),
        )
    })?;
    let tmp = plist.to_string_lossy().into_owned();
    run(&root(&["/bin/cp", &tmp, DAEMON]), password)?;
    run(&root(&["/usr/sbin/chown", "root:wheel", DAEMON]), password)?;
    run(&root(&["/bin/chmod", "644", DAEMON]), password)?;
    let _ = run(
        &root(&["/bin/launchctl", "bootstrap", "system", DAEMON]),
        password,
    );
    Ok(())
}

/// `diskutil verifyVolume` of the mount point.
pub fn verify(password: &str) -> Result<String, Fail> {
    step(
        STEP_VERIFY,
        run(
            &user(&["/usr/sbin/diskutil", "verifyVolume", MOUNT]),
            password,
        ),
    )
}

/// The check at the start of the agent: a session mark from an unclean end
/// makes the agent verify the volume, repair it once when the verify fails,
/// and report the text. The mark is set again for this session.
pub fn start_check() -> Option<String> {
    if !mounted() {
        return None;
    }
    let unclean = Path::new(SESSION_MARK).exists();
    let _ = std::fs::write(SESSION_MARK, b"");
    if !unclean {
        return None;
    }
    log::warn!("the last session did not end clean: verifying the data volume");
    let plan = user(&["/usr/sbin/diskutil", "verifyVolume", MOUNT]);
    let result = match spawn(&plan, None) {
        Ok((true, out)) => text::verify_result(true, None, &last_line(&out)),
        Ok((false, out)) => {
            log::warn!("the verify failed: {}", last_line(&out));
            let repair = user(&["/usr/sbin/diskutil", "repairVolume", MOUNT]);
            match spawn(&repair, None) {
                Ok((true, r)) => text::verify_result(false, Some(Ok(&last_line(&r))), ""),
                Ok((false, r)) => text::verify_result(false, Some(Err(&last_line(&r))), ""),
                Err(e) => text::verify_result(false, None, &e.to_string()),
            }
        }
        Err(e) => text::verify_no_run(&e.to_string()),
    };
    Some(result)
}

/// The session ended clean: remove the mark.
pub fn end_session() {
    let _ = std::fs::remove_file(SESSION_MARK);
}

/// A prepared disk that is not mounted: try the mount once, and give the
/// error text when it fails.
pub fn unmountable_check() -> Option<String> {
    if mounted() || !Path::new(PREPARED_STAMP).exists() {
        return None;
    }
    let plan = user(&["/usr/sbin/diskutil", "mount", "-mountPoint", MOUNT, VOLUME]);
    match spawn(&plan, None) {
        Ok((true, _)) if mounted() => None,
        Ok((_, out)) => Some(text::data_volume_no_mount(&last_line(&out))),
        Err(e) => Some(text::cannot_run_command("diskutil", &e.to_string())),
    }
}

fn last_line(s: &str) -> String {
    s.lines().last().unwrap_or("").trim().to_string()
}

/// The free bytes of the data disk, when it is mounted.
pub fn free_bytes() -> Option<u64> {
    if !mounted() {
        return None;
    }
    statvfs_free(Path::new(MOUNT))
}

/// The free bytes of the file system of a path.
pub fn statvfs_free(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the struct; the path is a valid C string.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    (r == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(test)]
#[path = "data_tests.rs"]
mod tests;

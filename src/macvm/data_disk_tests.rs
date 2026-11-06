use super::*;
use crate::macvm::session::{decide_files, prepare, Profile, Start, DATA_DISK_BYTES};
use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom, Write};

/// The attach output of a blank raw image, without a mount: one entity,
/// the whole disk, no partition hint. Written by hand after the shape of
/// `testdata/hdiutil-attach.plist`; `macvm-session.sh data-disk` prints
/// the real one.
const BLANK_ATTACH: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>system-entities</key>
	<array>
		<dict>
			<key>dev-entry</key>
			<string>/dev/disk7</string>
			<key>potentially-mountable</key>
			<false/>
			<key>unmapped-content-hint</key>
			<string>/dev/disk7</string>
		</dict>
	</array>
</dict>
</plist>
"#;

/// A format that writes what `diskutil partitionDisk` writes first: the
/// protective MBR and the GPT header.
fn gpt(p: &Path) -> Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).open(p)?;
    f.seek(SeekFrom::Start(510))?;
    f.write_all(&[0x55, 0xAA])?;
    f.write_all(b"EFI PART")?;
    Ok(())
}

fn head(p: &Path) -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    std::fs::File::open(p).unwrap().read_exact(&mut b).unwrap();
    b
}

#[test]
fn a_new_data_disk_is_formatted_before_the_first_boot() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("data.img");
    assert_eq!(make(&path, 1 << 20, &gpt).unwrap(), Made::Formatted);
    assert!(data_formatted(&path));
    assert!(!partial(&path).exists());
    let meta = std::fs::metadata(&path).unwrap();
    assert_eq!(meta.len(), 1 << 20);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    // The format got the partial name, before the rename.
    let seen = RefCell::new(Vec::new());
    let path2 = t.path().join("two.img");
    let record = |p: &Path| {
        seen.borrow_mut().push(p.to_path_buf());
        gpt(p)
    };
    make(&path2, 1 << 20, &record).unwrap();
    assert_eq!(*seen.borrow(), vec![partial(&path2)]);
}

#[test]
fn a_failed_format_leaves_zeros_for_the_agent() {
    let t = tempfile::tempdir().unwrap();
    // The format wrote half a table, then failed.
    let half = |p: &Path| -> Result<()> {
        gpt(p)?;
        anyhow::bail!("diskutil: permission denied")
    };
    let path = t.path().join("data.img");
    match make(&path, 1 << 20, &half).unwrap() {
        Made::Blank(why) => assert!(why.contains("permission denied"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(!data_formatted(&path));
    assert!(head(&path).iter().all(|b| *b == 0));
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 1 << 20);
    // A format that says Ok and wrote nothing.
    let path = t.path().join("b.img");
    let nothing = |_: &Path| -> Result<()> { Ok(()) };
    assert_eq!(
        make(&path, 1 << 20, &nothing).unwrap(),
        Made::Blank(text::data_format_no_table())
    );
    // A format that changed the size of the file.
    let path = t.path().join("c.img");
    let grow = |p: &Path| -> Result<()> {
        gpt(p)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(p)?
            .set_len(2 << 20)?;
        Ok(())
    };
    assert_eq!(
        make(&path, 1 << 20, &grow).unwrap(),
        Made::Blank(text::data_format_size_changed())
    );
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 1 << 20);
    assert!(!data_formatted(&path));
}

#[test]
fn a_crash_in_the_format_leaves_a_partial_that_the_next_start_repairs() {
    let t = tempfile::tempdir().unwrap();
    let g = crate::macvm::session::tests::golden(t.path());
    let p = Profile::new(t.path().join("profiles").join("app").join("home"));
    let crash = |p: &Path| -> Result<()> {
        gpt(p)?;
        panic!("crash in the format");
    };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut b = |_: &Profile, _: &str| Ok(());
        let mut h = crate::macvm::session::Hooks {
            clone: &crate::macvm::session::copy_clone,
            machine_id: &crate::macvm::session::tests::mid,
            backup: &mut b,
            format_data: &crash,
        };
        prepare(
            &p,
            &g,
            &crate::macvm::session::tests::now(),
            Start::Fresh,
            &mut h,
        )
    }));
    assert!(r.is_err());
    assert!(partial(&p.data()).exists() && !p.data().exists());
    let now = crate::macvm::session::tests::now();
    assert_eq!(decide_files(&p, &now), Ok(Start::Repair));
    // The repair makes the disk again, formatted this time.
    let mut b = |_: &Profile, _: &str| Ok(());
    let mut h = crate::macvm::session::Hooks {
        clone: &crate::macvm::session::copy_clone,
        machine_id: &crate::macvm::session::tests::mid,
        backup: &mut b,
        format_data: &gpt,
    };
    prepare(&p, &g, &now, Start::Repair, &mut h).unwrap();
    assert!(!partial(&p.data()).exists());
    assert!(data_formatted(&p.data()));
    assert_eq!(std::fs::metadata(p.data()).unwrap().len(), DATA_DISK_BYTES);
}

#[test]
fn whole_disk_of_a_blank_attach() {
    assert_eq!(whole_disk(BLANK_ATTACH).unwrap(), Some("/dev/disk7".into()));
    // The attach of a partitioned image: the whole disk is the entry
    // without a slice.
    let formatted = include_str!("testdata/hdiutil-attach.plist");
    let d = whole_disk(formatted).unwrap().unwrap();
    assert!(d.starts_with("/dev/disk") && !d["/dev/disk".len()..].contains('s'));
    assert!(whole_disk("<plist><dict></dict></plist>").is_err());
}

/// A fake `hdiutil` and `diskutil`: the answers by the first two words.
struct Tools {
    calls: Vec<String>,
    partition_ok: bool,
    detach_busy: usize,
}

impl Tools {
    fn run(&mut self, argv: &[String]) -> Result<(bool, String, String)> {
        let line = argv.join(" ");
        self.calls.push(line.clone());
        if line.contains("hdiutil attach") {
            return Ok((true, BLANK_ATTACH.into(), String::new()));
        }
        if line.contains("partitionDisk") {
            return Ok((self.partition_ok, String::new(), "Error: -69888".into()));
        }
        if line.contains("hdiutil detach") {
            if self.detach_busy > 0 {
                self.detach_busy -= 1;
                return Ok((false, String::new(), "Resource busy".into()));
            }
            return Ok((true, String::new(), String::new()));
        }
        anyhow::bail!("unexpected {line}")
    }
}

#[test]
fn the_format_attaches_without_a_mount_partitions_and_detaches() {
    let mut tools = Tools {
        calls: Vec::new(),
        partition_ok: true,
        detach_busy: 1,
    };
    let pauses = RefCell::new(0);
    format_image(
        Path::new("/p/data.img.partial"),
        &mut |a| tools.run(a),
        &|_| *pauses.borrow_mut() += 1,
    )
    .unwrap();
    assert_eq!(
        tools.calls,
        [
            "/usr/bin/hdiutil attach -imagekey diskimage-class=CRawDiskImage -nomount -plist /p/data.img.partial",
            "/usr/sbin/diskutil partitionDisk /dev/disk7 1 GPT APFS Data 100%",
            "/usr/bin/hdiutil detach /dev/disk7",
            "/usr/bin/hdiutil detach /dev/disk7",
        ]
    );
    assert_eq!(*pauses.borrow(), 1);
    assert_eq!(VOLUME, crate::guest::data::VOLUME);
}

#[test]
fn a_failed_partition_still_detaches_and_names_the_error() {
    let mut tools = Tools {
        calls: Vec::new(),
        partition_ok: false,
        detach_busy: 0,
    };
    let e = format_image(Path::new("/p/d"), &mut |a| tools.run(a), &|_| {}).unwrap_err();
    assert!(format!("{e:#}").contains("-69888"), "{e:#}");
    assert!(tools
        .calls
        .last()
        .unwrap()
        .contains("hdiutil detach /dev/disk7"));
    // A disk that stays busy: the fourth detach is forced, and the error
    // names it.
    let mut tools = Tools {
        calls: Vec::new(),
        partition_ok: true,
        detach_busy: 9,
    };
    let e = format_image(Path::new("/p/d"), &mut |a| tools.run(a), &|_| {}).unwrap_err();
    assert!(tools.calls.last().unwrap().ends_with("-force"));
    assert!(format!("{e:#}").contains("busy"));
}

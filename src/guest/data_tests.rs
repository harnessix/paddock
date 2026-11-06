use super::*;

const IOREG: &str = r#"+-o IOBlockStorageDevice  <class IOBlockStorageDevice, id 0x100000a1>
  | {
  |   "Device Characteristics" = {"Serial Number"="system-disk","Product Name"="Virtual Disk"}
  | }
  +-o IOMedia  <class IOMedia, id 0x100000a2>
  {
    "BSD Name" = "disk0"
  }
+-o IOBlockStorageDevice  <class IOBlockStorageDevice, id 0x100000b1>
  | {
  |   "Device Characteristics" = {"Product Name"="Virtual Disk"}
  |   "Serial Number" = "paddock-data"
  | }
  +-o IOMedia  <class IOMedia, id 0x100000b2>
| {
|   "BSD Name" = "disk2"
| }
+-o IOMedia  <class IOMedia, id 0x100000b3>
    {
      "BSD Name" = "disk2s1"
    }
"#;

#[test]
fn a_macos_27_guest_shows_no_serial_and_the_disk_without_the_boot_container_is_data() {
    // ioreg of a macOS 27.2 guest with the system disk and a 20 GiB
    // data disk (the device, BSD name, whole and size lines only).
    let ioreg = IOREG_MACOS_27;
    assert_eq!(find_disk(ioreg, "paddock-data"), None);
    assert_eq!(find_disk_by_elimination(ioreg), Some("disk2".into()));
    // Only the system disk: nothing.
    let system_only: String = ioreg
        .split("+-o AppleVirtIODiskStorageDevice@1")
        .take(2)
        .collect::<Vec<_>>()
        .join("+-o AppleVirtIODiskStorageDevice@1");
    assert_eq!(find_disk_by_elimination(&system_only), None);
    // Two candidate disks: never a guess.
    let two = format!(
        "{ioreg}+-o AppleVirtIODiskStorageDevice@1  <class AppleVirtIODiskStorageDevice>\n  +-o IOBlockStorageDriver\n    +-o Media\n      |   \"BSD Name\" = \"disk9\"\n"
    );
    assert_eq!(find_disk_by_elimination(&two), None);
}

#[test]
fn find_disk_by_serial_inside_device_characteristics() {
    let ioreg = r#"+-o AppleVirtIOBlockStorageDevice  <class AppleVirtIOBlockStorageDevice, id 0x1000004c1, registered, matched, active, busy 0 (1 ms), retain 8>
  | {
  |   "Device Characteristics" = {"Product Name"="VirtIO Block Device","Serial Number"="paddock-data","Vendor Name"="Apple"}
  |   "Protocol Characteristics" = {"Physical Interconnect"="Virtual Interface"}
  | }
  +-o IOBlockStorageDriver  <class IOBlockStorageDriver, id 0x1000004c3>
+-o Apple VirtIO Block Device Media  <class IOMedia, id 0x1000004c5>
  | {
  |   "BSD Name" = "disk3"
  | }
"#;
    assert_eq!(find_disk(ioreg, "paddock-data"), Some("disk3".into()));
    assert_eq!(find_disk(ioreg, "other"), None);
}

#[test]
fn find_disk_by_serial() {
    assert_eq!(find_disk(IOREG, "paddock-data"), Some("disk2".into()));
    assert_eq!(find_disk(IOREG, "other"), None);
    assert_eq!(find_disk("", "paddock-data"), None);
    // A partition name gives the whole disk.
    let only_partition = IOREG.replace("\"BSD Name\" = \"disk2\"", "\"Other\" = 1");
    assert_eq!(
        find_disk(&only_partition, "paddock-data"),
        Some("disk2".into())
    );
}

const BLANK: &str = "/dev/disk2 (external, physical):
   #:                       TYPE NAME                    SIZE       IDENTIFIER
   0:                                                   *21.5 GB    disk2
";
const FORMATTED: &str = "/dev/disk2 (external, physical):
   #:                       TYPE NAME                    SIZE       IDENTIFIER
   0:      GUID_partition_scheme                        *21.5 GB    disk2
   1:                 Apple_APFS Container disk3         21.5 GB    disk2s1
";

#[test]
fn formatted_disk_is_never_partitioned() {
    assert!(!has_partitions(BLANK));
    assert!(has_partitions(FORMATTED));
    assert!(has_partitions(
        "   0:      GUID_partition_scheme   *1 GB  disk2\n"
    ));
    assert!(!has_partitions(""));
    // The source of `prepare` partitions only after the blank check,
    // and the formatted branch holds no `partitionDisk`.
    let src = include_str!("data.rs");
    let formatted = src.find("if has_partitions(&listing) {").unwrap();
    let partition = src.find("\"partitionDisk\",").unwrap();
    let blank_end = src[formatted..].find("return verify(password);").unwrap() + formatted;
    assert!(
        partition > blank_end,
        "partitionDisk only on the blank path"
    );
}

#[test]
fn container_uuid_and_fresh() {
    assert_eq!(container(FORMATTED), Some("disk3".into()));
    assert_eq!(container(BLANK), None);
    assert_eq!(container(""), None);
    let info = "   Device Identifier:         disk3s1
   Volume Name:               Data
   Volume UUID:               0A81F3B1-51D9-3335-B3E3-169C3640360D
   Disk / Partition UUID:     0A81F3B1-51D9-3335-B3E3-169C3640360D
";
    assert_eq!(
        volume_uuid(info),
        Some("0A81F3B1-51D9-3335-B3E3-169C3640360D".into())
    );
    assert_eq!(volume_uuid("   Volume UUID:   not a uuid\n"), None);
    assert_eq!(volume_uuid(""), None);
    let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(fresh(&[]));
    assert!(fresh(&v(&[".fseventsd", ".Spotlight-V100", "._x"])));
    assert!(fresh(&v(&[".Trashes", "home.partial"])));
    assert!(!fresh(&v(&[".fseventsd", "home"])));
    assert!(!fresh(&v(&[".paddock-data-id"])));
    assert!(!fresh(&v(&["Projects"])));
}

#[test]
fn a_failed_step_names_it() {
    let r: Result<(), Fail> = step(STEP_MOUNT, Err((ErrorCode::Locked, "no".into())));
    assert_eq!(r, Err((ErrorCode::Locked, "mount the volume: no".into())));
    assert_eq!(step(STEP_COPY, Ok::<_, Fail>(3)), Ok(3));
    // Every fallible call of the prepare names its step.
    let src = include_str!("data.rs");
    let body = &src[src.find("pub fn prepare(").unwrap()..src.find("fn release(").unwrap()];
    for line in body.lines().filter(|l| l.trim_end().ends_with(")?;")) {
        assert!(
            line.contains("step(")
                || line.contains("link_home(")
                || line.trim().starts_with(")")
                // The cancel of the quit is not a failed step.
                || line.trim() == "watch.check()?;",
            "a call without its step: {line}"
        );
    }
}

#[test]
fn host_made_disk_is_filled_not_partitioned() {
    // The formatted branch moves the home onto an empty volume, and the
    // copy is renamed into place only when complete.
    let src = include_str!("data.rs");
    let formatted = src.find("if has_partitions(&listing) {").unwrap();
    let fill = src[formatted..].find("return populate(").unwrap() + formatted;
    let partition = src.find("\"partitionDisk\",").unwrap();
    assert!(fill < partition);
    let body = &src[src.find("fn copy_home(").unwrap()..];
    // The parts copy into the partial folder, and only then the rename.
    let plan = body.find("data_copy::plan(").unwrap();
    assert!(body[plan..].contains("Path::new(HOME_PARTIAL)"));
    let copy = body.find("data_copy::execute(").unwrap();
    let mv = body.find("\"/bin/mv\", HOME_PARTIAL, HOME").unwrap();
    assert!(plan < copy && copy < mv);
}

#[test]
fn plist_and_plans() {
    let p = daemon_plist("0A81F3B1-51D9-3335-B3E3-169C3640360D");
    assert!(p.contains("dev.paddock.data") && p.contains(MOUNT));
    assert!(p.contains("diskutil unmount 0A81F3B1-51D9-3335-B3E3-169C3640360D"));
    assert!(p.contains("mount -mountPoint /Volumes/paddock 0A81F3B1"));
    assert!(
        p.contains("&amp;&amp; exit 0"),
        "a mounted volume is left alone"
    );
    // By the name only, never an unmount by the name: the system
    // volume can be called Data too.
    let p = daemon_plist(VOLUME);
    assert!(p.contains("-mountPoint /Volumes/paddock Data</string>"));
    assert!(!p.contains("diskutil unmount"));
    assert!(!p.contains("&&") && !p.contains("2>&1"), "XML escapes");
    let r = root(&["/bin/mkdir", "-p", MOUNT]);
    assert_eq!(r.argv[..5], ["/usr/bin/sudo", "-S", "-p", "", "--"]);
    assert!(r.password_on_stdin);
    assert!(!user(&["/usr/sbin/ioreg"]).password_on_stdin);
    assert!(statvfs_free(Path::new("/")).is_some());
}

/// ioreg of a macOS 27.2 guest with the system disk and a 20 GiB data
/// disk: the device, BSD name, whole and size lines only.
const IOREG_MACOS_27: &str = r#"+-o AppleVirtIODiskStorageDevice@1  <class AppleVirtIODiskStorageDevice, id 0x1000001fd, registered, matched, active, busy 0 (33 ms), retain 11>
  +-o IOBlockStorageDriver  <class IOBlockStorageDriver, id 0x1000001ff, registered, matched, active, busy 0 (33 ms), retain 8>
    +-o Apple Inc. VirtIO Block Media  <class IOMedia, id 0x100000200, registered, matched, active, busy 0 (33 ms), retain 12>
      |   "Whole" = Yes
      |   "BSD Name" = "disk1"
      |   "Size" = 68719476736
      +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0x100000207, registered, matched, active, busy 0 (0 ms), retain 6>
      +-o IOGUIDPartitionScheme  <class IOGUIDPartitionScheme, id 0x100000216, !registered, !matched, active, busy 0 (30 ms), retain 8>
        |   "IOPropertyMatch" = {"Whole"=Yes}
        +-o iBootSystemContainer@1  <class IOMedia, id 0x100000230, registered, matched, active, busy 0 (6 ms), retain 12>
        | |   "Size" = 524288000
        | |   "Whole" = No
        | |   "BSD Name" = "disk1s1"
        | +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0x100000233, registered, matched, active, busy 0 (0 ms), retain 6>
        | +-o AppleAPFSContainerScheme  <class AppleAPFSContainerScheme, id 0x100000234, !registered, !matched, active, busy 0 (5 ms), retain 7>
        |   +-o AppleAPFSMedia  <class AppleAPFSMedia, id 0x100000241, registered, matched, active, busy 0 (5 ms), retain 11>
        |     |   "Size" = 524288000
        |     |   "Whole" = Yes
        |     |   "BSD Name" = "disk4"
        |     +-o AppleAPFSMediaBSDClient  <class AppleAPFSMediaBSDClient, id 0x100000242, registered, matched, active, busy 0 (0 ms), retain 7>
        |     +-o AppleAPFSContainer  <class AppleAPFSContainer, id 0x100000252, registered, matched, active, busy 0 (0 ms), retain 36>
        |       +-o iSCPreboot@1  <class AppleAPFSVolume, id 0x100000268, registered, matched, active, busy 0 (0 ms), retain 11>
        |       | |   "Size" = 524288000
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk4s1"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x10000026c, registered, matched, active, busy 0 (0 ms), retain 7>
        |       +-o xART@2  <class AppleAPFSVolume, id 0x100000269, registered, matched, active, busy 0 (0 ms), retain 11>
        |       | |   "Size" = 524288000
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk4s2"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x100000270, registered, matched, active, busy 0 (0 ms), retain 7>
        |       +-o Hardware@3  <class AppleAPFSVolume, id 0x10000026a, registered, matched, active, busy 0 (0 ms), retain 11>
        |       | |   "Size" = 524288000
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk4s3"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x10000026e, registered, matched, active, busy 0 (0 ms), retain 7>
        |       +-o Recovery@4  <class AppleAPFSVolume, id 0x10000026b, registered, matched, active, busy 0 (0 ms), retain 9>
        |         |   "Size" = 524288000
        |         |   "Whole" = No
        |         |   "BSD Name" = "disk4s4"
        |         +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x100000271, registered, matched, active, busy 0 (0 ms), retain 6>
        +-o Container@2  <class IOMedia, id 0x100000231, registered, matched, active, busy 0 (30 ms), retain 12>
        | |   "Size" = 62826479616
        | |   "Whole" = No
        | |   "BSD Name" = "disk1s2"
        | +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0x100000238, registered, matched, active, busy 0 (0 ms), retain 6>
        | +-o AppleAPFSContainerScheme  <class AppleAPFSContainerScheme, id 0x100000239, !registered, !matched, active, busy 0 (28 ms), retain 7>
        |   +-o AppleAPFSMedia  <class AppleAPFSMedia, id 0x100000254, registered, matched, active, busy 0 (28 ms), retain 11>
        |     |   "Size" = 62826479616
        |     |   "Whole" = Yes
        |     |   "BSD Name" = "disk6"
        |     +-o AppleAPFSMediaBSDClient  <class AppleAPFSMediaBSDClient, id 0x100000258, registered, matched, active, busy 0 (0 ms), retain 7>
        |     +-o AppleAPFSContainer  <class AppleAPFSContainer, id 0x100000264, registered, matched, active, busy 0 (0 ms), retain 90>
        |       +-o Macintosh HD@1  <class AppleAPFSVolume, id 0x10000027a, registered, matched, active, busy 0 (0 ms), retain 14>
        |       | |   "BSD Name" = "disk6s1"
        |       | |   "Size" = 62826479616
        |       | |   "Whole" = No
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x100000280, registered, matched, active, busy 0 (0 ms), retain 6>
        |       | +-o com.apple.os.update-182F06D70C9D02A9A9077E3BC880E61BFCEE97BDF8F6804B657B29E28EBBCDF0@1  <class AppleAPFSSnapshot, id 0x10000028e, registered, matched, active, busy 0 (0 ms), retain 10>
        |       |   |   "Size" = 62826479616
        |       |   |   "Whole" = No
        |       |   |   "BSD Name" = "disk6s1s1"
        |       |   +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0x10000028f, registered, matched, active, busy 0 (0 ms), retain 7>
        |       +-o Preboot@2  <class AppleAPFSVolume, id 0x10000027b, registered, matched, active, busy 0 (0 ms), retain 15>
        |       | |   "Size" = 62826479616
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk6s2"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x100000282, registered, matched, active, busy 0 (0 ms), retain 7>
        |       | +-o FizzB26B123.arm64eSystemCryptex  <class AppleAPFSGraft, id 0x1000002bd, !registered, !matched, active, busy 0, retain 5>
        |       | +-o AppCryptex  <class AppleAPFSGraft, id 0x1000002be, !registered, !matched, active, busy 0, retain 5>
        |       | +-o FizzB26B123.arm64eSystemCryptex  <class AppleAPFSGraft, id 0x1000002bf, !registered, !matched, active, busy 0, retain 5>
        |       | +-o FizzB26B123.RosettaSystemCryptex  <class AppleAPFSGraft, id 0x1000002c0, !registered, !matched, active, busy 0, retain 5>
        |       +-o Recovery@3  <class AppleAPFSVolume, id 0x10000027c, registered, matched, active, busy 0 (0 ms), retain 9>
        |       | |   "Size" = 62826479616
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk6s3"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x10000028a, registered, matched, active, busy 0 (0 ms), retain 6>
        |       +-o Update@4  <class AppleAPFSVolume, id 0x10000027d, registered, matched, active, busy 0 (0 ms), retain 11>
        |       | |   "BSD Name" = "disk6s4"
        |       | |   "Size" = 62826479616
        |       | |   "Whole" = No
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x100000288, registered, matched, active, busy 0 (0 ms), retain 7>
        |       +-o Data@5  <class AppleAPFSVolume, id 0x10000027e, registered, matched, active, busy 0 (0 ms), retain 11>
        |       | |   "Size" = 62826479616
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk6s5"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x100000284, registered, matched, active, busy 0 (0 ms), retain 7>
        |       +-o VM@6  <class AppleAPFSVolume, id 0x10000027f, registered, matched, active, busy 0 (0 ms), retain 11>
        |         |   "Size" = 62826479616
        |         |   "Whole" = No
        |         |   "BSD Name" = "disk6s6"
        |         +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x100000285, registered, matched, active, busy 0 (0 ms), retain 7>
        +-o RecoveryOSContainer@3  <class IOMedia, id 0x100000232, registered, matched, active, busy 0 (19 ms), retain 11>
          |   "Size" = 5368668160
          |   "Whole" = No
          |   "BSD Name" = "disk1s3"
          +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0x100000235, registered, matched, active, busy 0 (0 ms), retain 6>
          +-o AppleAPFSContainerScheme  <class AppleAPFSContainerScheme, id 0x100000236, !registered, !matched, active, busy 0 (17 ms), retain 6>
            +-o AppleAPFSMedia  <class AppleAPFSMedia, id 0x100000253, registered, matched, active, busy 0 (17 ms), retain 10>
              |   "Size" = 5368668160
              |   "Whole" = Yes
              |   "BSD Name" = "disk5"
              +-o AppleAPFSMediaBSDClient  <class AppleAPFSMediaBSDClient, id 0x100000255, registered, matched, active, busy 0 (0 ms), retain 6>
              +-o AppleAPFSContainer  <class AppleAPFSContainer, id 0x100000263, registered, matched, active, busy 0 (0 ms), retain 12>
                +-o Recovery@1  <class AppleAPFSVolume, id 0x100000274, registered, matched, active, busy 0 (0 ms), retain 9>
                | |   "Size" = 5368668160
                | |   "Whole" = No
                | |   "BSD Name" = "disk5s1"
                | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x100000276, registered, matched, active, busy 0 (0 ms), retain 6>
                +-o Update@2  <class AppleAPFSVolume, id 0x100000275, registered, matched, active, busy 0 (0 ms), retain 9>
                  |   "BSD Name" = "disk5s2"
                  |   "Size" = 5368668160
                  |   "Whole" = No
                  +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x100000278, registered, matched, active, busy 0 (0 ms), retain 6>
+-o AppleVirtIODiskStorageDevice@1  <class AppleVirtIODiskStorageDevice, id 0x100000209, registered, matched, active, busy 0 (7 ms), retain 10>
  +-o IOBlockStorageDriver  <class IOBlockStorageDriver, id 0x100000213, registered, matched, active, busy 0 (7 ms), retain 8>
    +-o Apple Inc. VirtIO Block Media  <class IOMedia, id 0x100000214, registered, matched, active, busy 0 (7 ms), retain 12>
      |   "Whole" = Yes
      |   "BSD Name" = "disk2"
      |   "Size" = 21474836480
      +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0x10000021f, registered, matched, active, busy 0 (0 ms), retain 6>
      +-o IOGUIDPartitionScheme  <class IOGUIDPartitionScheme, id 0x10000022e, !registered, !matched, active, busy 0 (5 ms), retain 7>
        |   "IOPropertyMatch" = {"Whole"=Yes}
        +-o EFI System Partition@1  <class IOMedia, id 0x10000023c, registered, matched, active, busy 0 (0 ms), retain 10>
        | |   "Size" = 209715200
        | |   "Whole" = No
        | |   "BSD Name" = "disk2s1"
        | +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0x10000023f, registered, matched, active, busy 0 (0 ms), retain 6>
        +-o Untitled 2@2  <class IOMedia, id 0x10000023d, registered, matched, active, busy 0 (5 ms), retain 12>
          |   "Size" = 21265080320
          |   "Whole" = No
          |   "BSD Name" = "disk2s2"
          +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0x10000023e, registered, matched, active, busy 0 (0 ms), retain 6>
          +-o AppleAPFSContainerScheme  <class AppleAPFSContainerScheme, id 0x100000240, !registered, !matched, active, busy 0 (5 ms), retain 7>
            +-o AppleAPFSMedia  <class AppleAPFSMedia, id 0x100000244, registered, matched, active, busy 0 (5 ms), retain 11>
              |   "Size" = 21265080320
              |   "Whole" = Yes
              |   "BSD Name" = "disk3"
              +-o AppleAPFSMediaBSDClient  <class AppleAPFSMediaBSDClient, id 0x100000246, registered, matched, active, busy 0 (0 ms), retain 7>
              +-o AppleAPFSContainer  <class AppleAPFSContainer, id 0x10000024e, registered, matched, active, busy 0 (0 ms), retain 17>
                +-o Data@1  <class AppleAPFSVolume, id 0x100000265, registered, matched, active, busy 0 (0 ms), retain 11>
                  |   "Size" = 21265080320
                  |   "Whole" = No
                  |   "BSD Name" = "disk3s1"
                  +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0x100000266, registered, matched, active, busy 0 (0 ms), retain 7>
+-o AppleVirtIOFSDevice  <class AppleVirtIOFSDevice, id 0x1000001ef, registered, matched, active, busy 0 (1 ms), retain 9>
  +-o IOBlockStorageDriver  <class IOBlockStorageDriver, id 0x1000001f0, registered, matched, active, busy 0 (0 ms), retain 6>
  +-o AppleVirtIOFSNub  <class AppleVirtIOFSNub, id 0x1000001fe, registered, matched, active, busy 0 (0 ms), retain 9>
    |   "Whole" = Yes
    |   "BSD Name" = "disk0"
    |   "Size" = 16384
    +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0x100000205, registered, matched, active, busy 0 (0 ms), retain 6>
"#;

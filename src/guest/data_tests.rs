use super::*;

const IOREG: &str = r#"+-o IOBlockStorageDevice  <class IOBlockStorageDevice, id 0xa0000001>
  | {
  |   "Device Characteristics" = {"Serial Number"="system-disk","Product Name"="Virtual Disk"}
  | }
  +-o IOMedia  <class IOMedia, id 0xa0000002>
  {
    "BSD Name" = "disk0"
  }
+-o IOBlockStorageDevice  <class IOBlockStorageDevice, id 0xa0000003>
  | {
  |   "Device Characteristics" = {"Product Name"="Virtual Disk"}
  |   "Serial Number" = "paddock-data"
  | }
  +-o IOMedia  <class IOMedia, id 0xa0000004>
| {
|   "BSD Name" = "disk2"
| }
+-o IOMedia  <class IOMedia, id 0xa0000005>
    {
      "BSD Name" = "disk2s1"
    }
"#;

#[test]
fn a_macos_27_guest_shows_no_serial_and_the_disk_without_the_boot_container_is_data() {
    // Synthetic ioreg of a macOS guest with a system disk and a data disk.
    // The device, BSD name, whole and size lines keep the parser shapes.
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
    let ioreg = r#"+-o AppleVirtIOBlockStorageDevice  <class AppleVirtIOBlockStorageDevice, id 0xa0000006, registered, matched, active, busy 0 (1 ms), retain 8>
  | {
  |   "Device Characteristics" = {"Product Name"="VirtIO Block Device","Serial Number"="paddock-data","Vendor Name"="Apple"}
  |   "Protocol Characteristics" = {"Physical Interconnect"="Virtual Interface"}
  | }
  +-o IOBlockStorageDriver  <class IOBlockStorageDriver, id 0xa0000007>
+-o Apple VirtIO Block Device Media  <class IOMedia, id 0xa0000008>
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
   Volume UUID:               11111111-2222-4333-8444-555555555555
   Disk / Partition UUID:     11111111-2222-4333-8444-555555555555
";
    assert_eq!(
        volume_uuid(info),
        Some("11111111-2222-4333-8444-555555555555".into())
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
    let p = daemon_plist("11111111-2222-4333-8444-555555555555");
    assert!(p.contains("dev.paddock.data") && p.contains(MOUNT));
    assert!(p.contains("diskutil unmount 11111111-2222-4333-8444-555555555555"));
    assert!(p.contains("mount -mountPoint /Volumes/paddock 11111111"));
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

/// Synthetic ioreg of a macOS guest with a system disk and a data disk.
/// The node IDs, timings, snapshot and sizes are invented.
const IOREG_MACOS_27: &str = r#"+-o AppleVirtIODiskStorageDevice@1  <class AppleVirtIODiskStorageDevice, id 0xa0000009, registered, matched, active, busy 0 (0 ms), retain 1>
  +-o IOBlockStorageDriver  <class IOBlockStorageDriver, id 0xa000000a, registered, matched, active, busy 0 (0 ms), retain 1>
    +-o Apple Inc. VirtIO Block Media  <class IOMedia, id 0xa000000b, registered, matched, active, busy 0 (0 ms), retain 1>
      |   "Whole" = Yes
      |   "BSD Name" = "disk1"
      |   "Size" = 8000000000
      +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0xa000000c, registered, matched, active, busy 0 (0 ms), retain 1>
      +-o IOGUIDPartitionScheme  <class IOGUIDPartitionScheme, id 0xa000000d, !registered, !matched, active, busy 0 (0 ms), retain 1>
        |   "IOPropertyMatch" = {"Whole"=Yes}
        +-o iBootSystemContainer@1  <class IOMedia, id 0xa000000e, registered, matched, active, busy 0 (0 ms), retain 1>
        | |   "Size" = 3000000000
        | |   "Whole" = No
        | |   "BSD Name" = "disk1s1"
        | +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0xa000000f, registered, matched, active, busy 0 (0 ms), retain 1>
        | +-o AppleAPFSContainerScheme  <class AppleAPFSContainerScheme, id 0xa0000010, !registered, !matched, active, busy 0 (0 ms), retain 1>
        |   +-o AppleAPFSMedia  <class AppleAPFSMedia, id 0xa0000011, registered, matched, active, busy 0 (0 ms), retain 1>
        |     |   "Size" = 3000000000
        |     |   "Whole" = Yes
        |     |   "BSD Name" = "disk4"
        |     +-o AppleAPFSMediaBSDClient  <class AppleAPFSMediaBSDClient, id 0xa0000012, registered, matched, active, busy 0 (0 ms), retain 1>
        |     +-o AppleAPFSContainer  <class AppleAPFSContainer, id 0xa0000013, registered, matched, active, busy 0 (0 ms), retain 1>
        |       +-o iSCPreboot@1  <class AppleAPFSVolume, id 0xa0000014, registered, matched, active, busy 0 (0 ms), retain 1>
        |       | |   "Size" = 3000000000
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk4s1"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa0000015, registered, matched, active, busy 0 (0 ms), retain 1>
        |       +-o xART@2  <class AppleAPFSVolume, id 0xa0000016, registered, matched, active, busy 0 (0 ms), retain 1>
        |       | |   "Size" = 3000000000
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk4s2"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa0000017, registered, matched, active, busy 0 (0 ms), retain 1>
        |       +-o Hardware@3  <class AppleAPFSVolume, id 0xa0000018, registered, matched, active, busy 0 (0 ms), retain 1>
        |       | |   "Size" = 3000000000
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk4s3"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa0000019, registered, matched, active, busy 0 (0 ms), retain 1>
        |       +-o Recovery@4  <class AppleAPFSVolume, id 0xa000001a, registered, matched, active, busy 0 (0 ms), retain 1>
        |         |   "Size" = 3000000000
        |         |   "Whole" = No
        |         |   "BSD Name" = "disk4s4"
        |         +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa000001b, registered, matched, active, busy 0 (0 ms), retain 1>
        +-o Container@2  <class IOMedia, id 0xa000001c, registered, matched, active, busy 0 (0 ms), retain 1>
        | |   "Size" = 7000000000
        | |   "Whole" = No
        | |   "BSD Name" = "disk1s2"
        | +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0xa000001d, registered, matched, active, busy 0 (0 ms), retain 1>
        | +-o AppleAPFSContainerScheme  <class AppleAPFSContainerScheme, id 0xa000001e, !registered, !matched, active, busy 0 (0 ms), retain 1>
        |   +-o AppleAPFSMedia  <class AppleAPFSMedia, id 0xa000001f, registered, matched, active, busy 0 (0 ms), retain 1>
        |     |   "Size" = 7000000000
        |     |   "Whole" = Yes
        |     |   "BSD Name" = "disk6"
        |     +-o AppleAPFSMediaBSDClient  <class AppleAPFSMediaBSDClient, id 0xa0000020, registered, matched, active, busy 0 (0 ms), retain 1>
        |     +-o AppleAPFSContainer  <class AppleAPFSContainer, id 0xa0000021, registered, matched, active, busy 0 (0 ms), retain 1>
        |       +-o Macintosh HD@1  <class AppleAPFSVolume, id 0xa0000022, registered, matched, active, busy 0 (0 ms), retain 1>
        |       | |   "BSD Name" = "disk6s1"
        |       | |   "Size" = 7000000000
        |       | |   "Whole" = No
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa0000023, registered, matched, active, busy 0 (0 ms), retain 1>
        |       | +-o com.apple.os.update-0000000000000000000000000000000000000000000000000000000000000000@1  <class AppleAPFSSnapshot, id 0xa0000024, registered, matched, active, busy 0 (0 ms), retain 1>
        |       |   |   "Size" = 7000000000
        |       |   |   "Whole" = No
        |       |   |   "BSD Name" = "disk6s1s1"
        |       |   +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0xa0000025, registered, matched, active, busy 0 (0 ms), retain 1>
        |       +-o Preboot@2  <class AppleAPFSVolume, id 0xa0000026, registered, matched, active, busy 0 (0 ms), retain 1>
        |       | |   "Size" = 7000000000
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk6s2"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa0000027, registered, matched, active, busy 0 (0 ms), retain 1>
        |       | +-o ExampleBuild.arm64eSystemCryptex  <class AppleAPFSGraft, id 0xa0000028, !registered, !matched, active, busy 0, retain 1>
        |       | +-o AppCryptex  <class AppleAPFSGraft, id 0xa0000029, !registered, !matched, active, busy 0, retain 1>
        |       | +-o ExampleBuild.arm64eSystemCryptex  <class AppleAPFSGraft, id 0xa000002a, !registered, !matched, active, busy 0, retain 1>
        |       | +-o ExampleBuild.RosettaSystemCryptex  <class AppleAPFSGraft, id 0xa000002b, !registered, !matched, active, busy 0, retain 1>
        |       +-o Recovery@3  <class AppleAPFSVolume, id 0xa000002c, registered, matched, active, busy 0 (0 ms), retain 1>
        |       | |   "Size" = 7000000000
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk6s3"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa000002d, registered, matched, active, busy 0 (0 ms), retain 1>
        |       +-o Update@4  <class AppleAPFSVolume, id 0xa000002e, registered, matched, active, busy 0 (0 ms), retain 1>
        |       | |   "BSD Name" = "disk6s4"
        |       | |   "Size" = 7000000000
        |       | |   "Whole" = No
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa000002f, registered, matched, active, busy 0 (0 ms), retain 1>
        |       +-o Data@5  <class AppleAPFSVolume, id 0xa0000030, registered, matched, active, busy 0 (0 ms), retain 1>
        |       | |   "Size" = 7000000000
        |       | |   "Whole" = No
        |       | |   "BSD Name" = "disk6s5"
        |       | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa0000031, registered, matched, active, busy 0 (0 ms), retain 1>
        |       +-o VM@6  <class AppleAPFSVolume, id 0xa0000032, registered, matched, active, busy 0 (0 ms), retain 1>
        |         |   "Size" = 7000000000
        |         |   "Whole" = No
        |         |   "BSD Name" = "disk6s6"
        |         +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa0000033, registered, matched, active, busy 0 (0 ms), retain 1>
        +-o RecoveryOSContainer@3  <class IOMedia, id 0xa0000034, registered, matched, active, busy 0 (0 ms), retain 1>
          |   "Size" = 4000000000
          |   "Whole" = No
          |   "BSD Name" = "disk1s3"
          +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0xa0000035, registered, matched, active, busy 0 (0 ms), retain 1>
          +-o AppleAPFSContainerScheme  <class AppleAPFSContainerScheme, id 0xa0000036, !registered, !matched, active, busy 0 (0 ms), retain 1>
            +-o AppleAPFSMedia  <class AppleAPFSMedia, id 0xa0000037, registered, matched, active, busy 0 (0 ms), retain 1>
              |   "Size" = 4000000000
              |   "Whole" = Yes
              |   "BSD Name" = "disk5"
              +-o AppleAPFSMediaBSDClient  <class AppleAPFSMediaBSDClient, id 0xa0000038, registered, matched, active, busy 0 (0 ms), retain 1>
              +-o AppleAPFSContainer  <class AppleAPFSContainer, id 0xa0000039, registered, matched, active, busy 0 (0 ms), retain 1>
                +-o Recovery@1  <class AppleAPFSVolume, id 0xa000003a, registered, matched, active, busy 0 (0 ms), retain 1>
                | |   "Size" = 4000000000
                | |   "Whole" = No
                | |   "BSD Name" = "disk5s1"
                | +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa000003b, registered, matched, active, busy 0 (0 ms), retain 1>
                +-o Update@2  <class AppleAPFSVolume, id 0xa000003c, registered, matched, active, busy 0 (0 ms), retain 1>
                  |   "BSD Name" = "disk5s2"
                  |   "Size" = 4000000000
                  |   "Whole" = No
                  +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa000003d, registered, matched, active, busy 0 (0 ms), retain 1>
+-o AppleVirtIODiskStorageDevice@1  <class AppleVirtIODiskStorageDevice, id 0xa000003e, registered, matched, active, busy 0 (0 ms), retain 1>
  +-o IOBlockStorageDriver  <class IOBlockStorageDriver, id 0xa000003f, registered, matched, active, busy 0 (0 ms), retain 1>
    +-o Apple Inc. VirtIO Block Media  <class IOMedia, id 0xa0000040, registered, matched, active, busy 0 (0 ms), retain 1>
      |   "Whole" = Yes
      |   "BSD Name" = "disk2"
      |   "Size" = 6000000000
      +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0xa0000041, registered, matched, active, busy 0 (0 ms), retain 1>
      +-o IOGUIDPartitionScheme  <class IOGUIDPartitionScheme, id 0xa0000042, !registered, !matched, active, busy 0 (0 ms), retain 1>
        |   "IOPropertyMatch" = {"Whole"=Yes}
        +-o EFI System Partition@1  <class IOMedia, id 0xa0000043, registered, matched, active, busy 0 (0 ms), retain 1>
        | |   "Size" = 2000000000
        | |   "Whole" = No
        | |   "BSD Name" = "disk2s1"
        | +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0xa0000044, registered, matched, active, busy 0 (0 ms), retain 1>
        +-o Untitled 2@2  <class IOMedia, id 0xa0000045, registered, matched, active, busy 0 (0 ms), retain 1>
          |   "Size" = 5000000000
          |   "Whole" = No
          |   "BSD Name" = "disk2s2"
          +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0xa0000046, registered, matched, active, busy 0 (0 ms), retain 1>
          +-o AppleAPFSContainerScheme  <class AppleAPFSContainerScheme, id 0xa0000047, !registered, !matched, active, busy 0 (0 ms), retain 1>
            +-o AppleAPFSMedia  <class AppleAPFSMedia, id 0xa0000048, registered, matched, active, busy 0 (0 ms), retain 1>
              |   "Size" = 5000000000
              |   "Whole" = Yes
              |   "BSD Name" = "disk3"
              +-o AppleAPFSMediaBSDClient  <class AppleAPFSMediaBSDClient, id 0xa0000049, registered, matched, active, busy 0 (0 ms), retain 1>
              +-o AppleAPFSContainer  <class AppleAPFSContainer, id 0xa000004a, registered, matched, active, busy 0 (0 ms), retain 1>
                +-o Data@1  <class AppleAPFSVolume, id 0xa000004b, registered, matched, active, busy 0 (0 ms), retain 1>
                  |   "Size" = 5000000000
                  |   "Whole" = No
                  |   "BSD Name" = "disk3s1"
                  +-o AppleAPFSVolumeBSDClient  <class AppleAPFSVolumeBSDClient, id 0xa000004c, registered, matched, active, busy 0 (0 ms), retain 1>
+-o AppleVirtIOFSDevice  <class AppleVirtIOFSDevice, id 0xa000004d, registered, matched, active, busy 0 (0 ms), retain 1>
  +-o IOBlockStorageDriver  <class IOBlockStorageDriver, id 0xa000004e, registered, matched, active, busy 0 (0 ms), retain 1>
  +-o AppleVirtIOFSNub  <class AppleVirtIOFSNub, id 0xa000004f, registered, matched, active, busy 0 (0 ms), retain 1>
    |   "Whole" = Yes
    |   "BSD Name" = "disk0"
    |   "Size" = 1000000000
    +-o IOMediaBSDClient  <class IOMediaBSDClient, id 0xa0000050, registered, matched, active, busy 0 (0 ms), retain 1>
"#;

//! The check for disk encryption, on a made-up mount table and /sys.

use crate::copy::disk::{linux, Encryption};
use std::path::Path;

const INFO: &str = "\
22 1 0:20 / /proc rw,nosuid shared:5 - proc proc rw
30 1 254:1 / / rw,relatime shared:1 - ext4 /dev/mapper/root rw
40 30 259:3 / /boot rw,relatime shared:2 - vfat /dev/nvme0n1p1 rw
50 30 0:45 /@home /home rw,relatime shared:3 - btrfs /dev/mapper/home rw,subvol=/@home
60 30 0:60 / /tmp rw shared:4 - tmpfs tmpfs rw
70 30 8:17 / /mnt/my\\040disk rw shared:6 - ext4 /dev/sdb1 rw
80 30 0:70 / /home/demo/Private rw shared:7 - ecryptfs /home/demo/.Private rw
";

fn sys() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let put = |rel: &str, text: &str| {
        let f = t.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    // / is LVM on LUKS: 254:1 (lv) -> 254:0 (crypt) -> 259:2 (partition).
    put("dev/block/254:1/dm/uuid", "LVM-abcdef\n");
    put("dev/block/254:1/slaves/dm-0/dev", "254:0\n");
    put(
        "dev/block/254:0/dm/uuid",
        "CRYPT-LUKS2-0123456789abcdef-root\n",
    );
    put("dev/block/254:0/slaves/nvme0n1p2/dev", "259:2\n");
    put("dev/block/259:2/dev", "259:2\n");
    // /boot: a plain partition.
    put("dev/block/259:3/dev", "259:3\n");
    // /home: btrfs on plain dm-crypt, found through the device file.
    put("dev/block/254:5/dm/uuid", "CRYPT-PLAIN-home\n");
    // The disk with a space in its name: plain.
    put("dev/block/8:17/dev", "8:17\n");
    t
}

fn check(path: &str) -> Encryption {
    let t = sys();
    linux(INFO, t.path(), Path::new(path), &|src| {
        (src == Path::new("/dev/mapper/home")).then_some((254, 5))
    })
}

#[test]
fn an_encrypted_disk_is_found_below_other_layers() {
    assert_eq!(check("/var/lib/x"), Encryption::Encrypted("LUKS".into()));
    assert_eq!(
        check("/home/demo/.local/share/paddock/copies"),
        Encryption::Encrypted("dm-crypt".into())
    );
    assert_eq!(
        check("/home/demo/Private/x"),
        Encryption::Encrypted("eCryptfs".into())
    );
}

#[test]
fn a_plain_disk_is_named_as_not_encrypted() {
    assert_eq!(check("/boot/efi"), Encryption::NotEncrypted);
    assert_eq!(check("/mnt/my disk/copies"), Encryption::NotEncrypted);
}

#[test]
fn what_paddock_cannot_see_is_unknown_and_never_a_yes() {
    assert_eq!(check("/tmp/x"), Encryption::Unknown);
    // A device that /sys does not have.
    let t = tempfile::tempdir().unwrap();
    assert_eq!(
        linux(INFO, t.path(), Path::new("/var"), &|_| None),
        Encryption::Unknown
    );
    assert_eq!(
        linux("", t.path(), Path::new("/var"), &|_| None),
        Encryption::Unknown
    );
    assert_eq!(
        linux("garbage line\n", t.path(), Path::new("/"), &|_| None),
        Encryption::Unknown
    );
}

#[test]
fn the_words_for_the_user() {
    let p = Path::new("/data/copies");
    assert_eq!(
        Encryption::Encrypted("LUKS".into()).word(),
        "encrypted (LUKS)"
    );
    assert_eq!(
        Encryption::Encrypted("LUKS".into()).doctor_line(p),
        "  copies   /data/copies: the disk is encrypted (LUKS)"
    );
    let no = Encryption::NotEncrypted.doctor_line(p);
    assert!(
        no.contains("the disk is not encrypted") && no.contains("LUKS, FileVault"),
        "{no}"
    );
    assert!(Encryption::Unknown.doctor_line(p).contains("cannot tell"));
    // The doctor names the folder of the stores, also before the first copy.
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let line = crate::copy::disk::doctor_text(&env);
    let root = env.paddock_data().join("copies");
    assert!(
        line.starts_with(&format!("  copies   {}: ", root.display())),
        "{line}"
    );
    assert!(crate::storage::guard::doctor_lines(&env).contains(&line));
    let v = serde_json::to_value(Encryption::Encrypted("FileVault".into())).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"state": "encrypted", "kind": "FileVault"})
    );
    let v = serde_json::to_value(Encryption::NotEncrypted).unwrap();
    assert_eq!(v, serde_json::json!({"state": "not_encrypted"}));
}

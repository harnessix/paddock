use super::*;
use crate::macvm::apfs_fixture::{self as fx, Fixture, BS};

const PLIST: &str = "Library/LaunchDaemons/dev.paddock.firstboot.plist";
const SCRIPT: &str = "Library/Application Support/paddock-firstboot/firstboot.sh";

fn seeded() -> Fixture {
    Fixture::new()
        .dir("Library", 0, 0)
        .dir("Library/LaunchDaemons", 0, 0)
        .dir("Library/Application Support", 0, 0)
        .dir("Library/Application Support/paddock-firstboot", 99, 99)
        .file(PLIST, 99, 99)
        .file(SCRIPT, 99, 99)
        .file("Users/paddock/notes.txt", 501, 20)
}

fn image(f: &Fixture) -> (tempfile::TempDir, std::path::PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("disk.img");
    fx::write(f, &p);
    (t, p)
}

/// Every block that carries an object header still has a right checksum.
fn all_checksums_ok(bytes: &[u8]) {
    for (i, b) in bytes.chunks(BS).enumerate() {
        if b.iter().all(|x| *x == 0) || b.iter().all(|x| *x == 0xA5) {
            continue;
        }
        assert!(checksum_ok(b), "block {i} has a wrong checksum");
    }
}

#[test]
fn fletcher64_round_trip_and_corruption() {
    let mut b: Vec<u8> = (0..BS).map(|i| (i % 251) as u8).collect();
    set_checksum(&mut b);
    assert!(checksum_ok(&b));
    b[100] = b[100].wrapping_add(1);
    assert!(!checksum_ok(&b));
    let mut z = vec![0u8; BS];
    let c = fletcher64(&z);
    set_checksum(&mut z);
    assert_eq!(u64::from_le_bytes(z[..8].try_into().unwrap()), c);
    assert!(checksum_ok(&z));
}

#[test]
fn crc32c_matches_the_standard_check_value() {
    // The standard CRC-32C of "123456789" is 0xE3069283 with the final
    // complement; the raw register is its complement.
    assert_eq!(!crc32c_raw(b"123456789"), 0xE306_9283);
    // The hash folds case and keeps 22 bits.
    assert_eq!(name_hash("Library"), name_hash("library"));
    assert!(name_hash("LaunchDaemons") <= 0x3F_FFFF);
    assert_ne!(name_hash("a"), name_hash("b"));
}

#[test]
fn a_mount_without_owners_leaves_99_and_chown_makes_root() {
    let (_t, p) = image(&seeded());
    let before = std::fs::read(&p).unwrap();
    assert_eq!(
        owners(&p, ROLE_DATA, &[PLIST, SCRIPT]).unwrap(),
        vec![(99, 99), (99, 99)]
    );
    let n = chown(
        &p,
        ROLE_DATA,
        0,
        0,
        &[
            "Library/Application Support/paddock-firstboot",
            PLIST,
            SCRIPT,
        ],
    )
    .unwrap();
    assert_eq!(n, 3);
    assert_eq!(
        owners(
            &p,
            ROLE_DATA,
            &[
                PLIST,
                SCRIPT,
                "Library/Application Support/paddock-firstboot",
                "Users/paddock/notes.txt",
                "Library",
            ]
        )
        .unwrap(),
        vec![(0, 0), (0, 0), (0, 0), (501, 20), (0, 0)]
    );
    let after = std::fs::read(&p).unwrap();
    assert_eq!(before.len(), after.len());
    all_checksums_ok(&after);
    // Only the leaf of the file-system tree changed.
    let changed: Vec<usize> = before
        .chunks(BS)
        .zip(after.chunks(BS))
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    // A second run finds the owners in place and writes nothing.
    assert_eq!(chown(&p, ROLE_DATA, 0, 0, &[PLIST, SCRIPT]).unwrap(), 0);
    assert_eq!(std::fs::read(&p).unwrap(), after);
}

#[test]
fn an_inode_with_another_owner_or_a_missing_path_leaves_the_image_as_it_was() {
    let (_t, p) = image(&seeded());
    let before = std::fs::read(&p).unwrap();
    let e = chown(&p, ROLE_DATA, 0, 0, &[PLIST, "Users/paddock/notes.txt"]).unwrap_err();
    let msg = format!("{e:#}");
    assert!(
        msg.contains("Users/paddock/notes.txt") && msg.contains("501:20"),
        "{msg}"
    );
    assert_eq!(std::fs::read(&p).unwrap(), before, "nothing written");
    let e = chown(
        &p,
        ROLE_DATA,
        0,
        0,
        &[PLIST, "Library/LaunchDaemons/nope.plist"],
    )
    .unwrap_err();
    assert!(format!("{e:#}").contains("nope.plist"), "{e:#}");
    assert_eq!(std::fs::read(&p).unwrap(), before, "nothing written");
}

#[test]
fn a_two_level_tree_with_many_files_and_a_split_object_map() {
    let mut f = seeded();
    for i in 0..300 {
        f = f.file(&format!("Library/Caches/f{i:03}"), 0, 0);
    }
    f = f.file("Library/Caches/zz-last", 99, 99);
    f.leaf_cap = 9;
    f.omap_cap = 4;
    let (_t, p) = image(&f);
    let bytes = std::fs::read(&p).unwrap();
    all_checksums_ok(&bytes);
    assert_eq!(
        chown(
            &p,
            ROLE_DATA,
            0,
            0,
            &[PLIST, SCRIPT, "Library/Caches/zz-last"]
        )
        .unwrap(),
        3
    );
    assert_eq!(
        owners(
            &p,
            ROLE_DATA,
            &[
                PLIST,
                SCRIPT,
                "Library/Caches/zz-last",
                "Library/Caches/f150",
                "Users/paddock/notes.txt"
            ]
        )
        .unwrap(),
        vec![(0, 0), (0, 0), (0, 0), (0, 0), (501, 20)]
    );
    all_checksums_ok(&std::fs::read(&p).unwrap());
}

#[test]
fn a_gpt_disk_finds_the_apfs_partition() {
    let mut f = seeded();
    f.gpt = true;
    let (_t, p) = image(&f);
    assert_eq!(chown(&p, ROLE_DATA, 0, 0, &[PLIST]).unwrap(), 1);
    assert_eq!(owners(&p, ROLE_DATA, &[PLIST]).unwrap(), vec![(0, 0)]);
    // The bytes before the partition are untouched.
    let bytes = std::fs::read(&p).unwrap();
    assert_eq!(&bytes[512..520], b"EFI PART");
    all_checksums_ok(&bytes[(fx::GPT_START_LBA as usize * 512)..]);
}

#[test]
fn damaged_images_are_errors() {
    let (_t, p) = image(&seeded());
    let good = std::fs::read(&p).unwrap();
    let expect_err = |bytes: &[u8], what: &str| {
        std::fs::write(&p, bytes).unwrap();
        let e = owners(&p, ROLE_DATA, &[PLIST]).expect_err(what);
        assert!(!format!("{e:#}").is_empty());
    };
    // Not APFS, not GPT.
    expect_err(&vec![0u8; BS * 2], "zeros");
    // A short file.
    expect_err(&good[..100], "short");
    // A bad block size.
    let mut b = good.clone();
    b[NX_BLOCK_SIZE_OFF..NX_BLOCK_SIZE_OFF + 4].copy_from_slice(&4095u32.to_le_bytes());
    expect_err(&b, "block size");
    // Block 0 with a wrong checksum.
    let mut b = good.clone();
    b[300] ^= 1;
    expect_err(&b, "block 0 checksum");
    // The file-system leaf with a wrong checksum (block 7).
    let mut b = good.clone();
    b[7 * BS + 200] ^= 1;
    expect_err(&b, "leaf checksum");
    // A table of contents larger than the node, checksum made right.
    let mut b = good.clone();
    let leaf = &mut b[7 * BS..8 * BS];
    leaf[BTN_NKEYS_OFF..BTN_NKEYS_OFF + 4].copy_from_slice(&5000u32.to_le_bytes());
    set_checksum(leaf);
    expect_err(&b, "toc");
    // No Data volume: its role changed.
    let mut b = good.clone();
    let vol = &mut b[5 * BS..6 * BS];
    vol[APFS_ROLE_OFF..APFS_ROLE_OFF + 2].copy_from_slice(&0x80u16.to_le_bytes());
    set_checksum(vol);
    expect_err(&b, "role");
}

/// Random byte flips with right checksums never panic and never write.
#[test]
fn random_damage_never_panics() {
    let (_t, p) = image(&seeded());
    let good = std::fs::read(&p).unwrap();
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let blocks = good.len() / BS;
    for _ in 0..400 {
        let mut b = good.clone();
        let blk = (next() as usize) % blocks;
        for _ in 0..4 {
            let at = blk * BS + 8 + (next() as usize) % (BS - 8);
            b[at] = next() as u8;
        }
        set_checksum(&mut b[blk * BS..(blk + 1) * BS]);
        std::fs::write(&p, &b).unwrap();
        let _ = owners(&p, ROLE_DATA, &[PLIST, SCRIPT]);
        let _ = chown(&p, ROLE_DATA, 0, 0, &[PLIST]);
    }
}

//! A builder of small APFS disk images for the tests of `apfs.rs` and of
//! the offline seed: a container superblock with its checkpoint copy, the
//! container object map, a System volume and a Data volume, the object map
//! of the Data volume and its file-system B-tree, in one level or two, and
//! a GPT in front when asked. The layout follows Apple's "Apple File
//! System Reference" and the reader of `apfs.rs`; nothing else is written.

use super::apfs::*;

pub const BS: usize = 4096;
/// The virtual object ids of the volume superblocks and of the root of the
/// file-system tree.
const SYSTEM_VOL_OID: u64 = 1026;
const DATA_VOL_OID: u64 = 1027;
const FS_ROOT_OID: u64 = 1028;
const XID: u64 = 7;
/// The first sector of the APFS partition in the GPT form.
pub const GPT_START_LBA: u64 = 40;

/// One file or folder of the Data volume.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: String,
    pub dir: bool,
    pub uid: u32,
    pub gid: u32,
}

/// What to build.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub entries: Vec<Entry>,
    /// File-system records per leaf: fewer than all makes a two-level tree.
    pub leaf_cap: usize,
    /// Object map entries per leaf.
    pub omap_cap: usize,
    pub gpt: bool,
}

impl Fixture {
    pub fn new() -> Self {
        Fixture {
            entries: Vec::new(),
            leaf_cap: 1000,
            omap_cap: 1000,
            gpt: false,
        }
    }

    pub fn file(mut self, path: &str, uid: u32, gid: u32) -> Self {
        self.entries.push(Entry {
            path: path.into(),
            dir: false,
            uid,
            gid,
        });
        self
    }

    pub fn dir(mut self, path: &str, uid: u32, gid: u32) -> Self {
        self.entries.push(Entry {
            path: path.into(),
            dir: true,
            uid,
            gid,
        });
        self
    }
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// One B-tree node block. `fixed` nodes use `kvoff_t`, the others
/// `kvloc_t`.
pub fn node(oid: u64, root: bool, leaf: bool, fixed: bool, kv: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut b = vec![0u8; BS];
    put64(&mut b, OBJ_OID_OFF, oid);
    put64(&mut b, OBJ_XID_OFF, XID);
    let kind = if root {
        OBJECT_TYPE_BTREE
    } else {
        OBJECT_TYPE_BTREE_NODE
    };
    put32(&mut b, OBJ_TYPE_OFF, kind);
    let mut flags = 0;
    if root {
        flags |= BTNODE_ROOT;
    }
    if leaf {
        flags |= BTNODE_LEAF;
    }
    if fixed {
        flags |= BTNODE_FIXED_KV_SIZE;
    }
    put16(&mut b, BTN_FLAGS_OFF, flags);
    put32(&mut b, BTN_NKEYS_OFF, kv.len() as u32);
    let entry = if fixed { 4 } else { 8 };
    let toc_len = kv.len() * entry;
    put16(&mut b, BTN_TABLE_SPACE_OFF, 0);
    put16(&mut b, BTN_TABLE_SPACE_OFF + 2, toc_len as u16);
    let toc = BTN_DATA_OFF;
    let key_start = toc + toc_len;
    let value_end = if root { BS - BTREE_INFO_SIZE } else { BS };
    let (mut k, mut v) = (0usize, 0usize);
    for (i, (key, val)) in kv.iter().enumerate() {
        b[key_start + k..key_start + k + key.len()].copy_from_slice(key);
        v += val.len();
        b[value_end - v..value_end - v + val.len()].copy_from_slice(val);
        let t = toc + i * entry;
        if fixed {
            put16(&mut b, t, k as u16);
            put16(&mut b, t + 2, v as u16);
        } else {
            put16(&mut b, t, k as u16);
            put16(&mut b, t + 2, key.len() as u16);
            put16(&mut b, t + 4, v as u16);
            put16(&mut b, t + 6, val.len() as u16);
        }
        k += key.len();
        assert!(key_start + k <= value_end - v, "the node is full");
    }
    set_checksum(&mut b);
    b
}

/// A file-system record key and value, with its sort key.
struct Rec {
    sort: (u64, u64, u32, String),
    key: Vec<u8>,
    val: Vec<u8>,
}

fn records(entries: &[Entry]) -> Vec<Rec> {
    // Folders first by path, so a parent always has its inode number.
    let mut all: Vec<Entry> = Vec::new();
    for e in entries {
        let parts: Vec<&str> = e.path.split('/').filter(|p| !p.is_empty()).collect();
        for n in 1..parts.len() {
            let p = parts[..n].join("/");
            if !all.iter().any(|x| x.path == p) && !entries.iter().any(|x| x.path == p) {
                all.push(Entry {
                    path: p,
                    dir: true,
                    uid: 0,
                    gid: 0,
                });
            }
        }
    }
    all.extend(entries.iter().cloned());
    all.sort_by_key(|e| e.path.matches('/').count());
    let mut ids: Vec<(String, u64)> = vec![(String::new(), ROOT_DIR_INODE)];
    let mut out = Vec::new();
    let inode = |ino: u64, parent: u64, uid: u32, gid: u32, dir: bool| {
        let mut key = vec![0u8; 8];
        put64(&mut key, 0, (TYPE_INODE << OBJ_TYPE_SHIFT) | ino);
        let mut val = vec![0u8; 92];
        put64(&mut val, 0, parent);
        put64(&mut val, INODE_PRIVATE_ID_OFF, ino);
        put32(&mut val, INODE_OWNER_OFF, uid);
        put32(&mut val, INODE_GROUP_OFF, gid);
        put16(&mut val, 80, if dir { 0o040755 } else { 0o100644 });
        Rec {
            sort: (ino, TYPE_INODE, 0, String::new()),
            key,
            val,
        }
    };
    out.push(inode(ROOT_DIR_INODE, 1, 0, 0, true));
    for (ino, e) in (16u64..).zip(all.iter()) {
        let (parent_path, name) = match e.path.rsplit_once('/') {
            Some((p, n)) => (p.to_string(), n.to_string()),
            None => (String::new(), e.path.clone()),
        };
        let parent = ids
            .iter()
            .find(|(p, _)| *p == parent_path)
            .map(|(_, i)| *i)
            .expect("parent first");
        ids.push((e.path.clone(), ino));
        out.push(inode(ino, parent, e.uid, e.gid, e.dir));
        let hash = name_hash(&name);
        let mut key = vec![0u8; 12];
        put64(&mut key, 0, (TYPE_DIR_REC << OBJ_TYPE_SHIFT) | parent);
        put32(&mut key, 8, (hash << 10) | (name.len() as u32 + 1));
        key.extend_from_slice(name.as_bytes());
        key.push(0);
        let mut val = vec![0u8; 18];
        put64(&mut val, 0, ino);
        out.push(Rec {
            sort: (parent, TYPE_DIR_REC, hash, name.clone()),
            key,
            val,
        });
    }
    out.sort_by(|a, b| a.sort.cmp(&b.sort));
    out
}

fn omap_key(oid: u64, xid: u64) -> Vec<u8> {
    let mut k = vec![0u8; 16];
    put64(&mut k, 0, oid);
    put64(&mut k, 8, xid);
    k
}

fn omap_val(paddr: u64) -> Vec<u8> {
    let mut v = vec![0u8; 16];
    put32(&mut v, 4, BS as u32);
    put64(&mut v, 8, paddr);
    v
}

/// An object map tree at blocks `first..`: leaves of `cap` entries and a
/// root above them when one leaf is not enough. Returns the blocks and the
/// root address.
fn omap_tree(first: u64, mut map: Vec<(u64, u64, u64)>, cap: usize) -> (Vec<Vec<u8>>, u64) {
    map.sort();
    let chunks: Vec<&[(u64, u64, u64)]> = map.chunks(cap.max(1)).collect();
    let kv = |c: &[(u64, u64, u64)]| -> Vec<(Vec<u8>, Vec<u8>)> {
        c.iter()
            .map(|(o, x, p)| (omap_key(*o, *x), omap_val(*p)))
            .collect()
    };
    if chunks.len() == 1 {
        return (vec![node(first, true, true, true, &kv(chunks[0]))], first);
    }
    let mut blocks = Vec::new();
    let mut index = Vec::new();
    for (i, c) in chunks.iter().enumerate() {
        let addr = first + 1 + i as u64;
        blocks.push(node(addr, false, true, true, &kv(c)));
        let mut child = vec![0u8; 8];
        put64(&mut child, 0, addr);
        index.push((omap_key(c[0].0, c[0].1), child));
    }
    let mut out = vec![node(first, true, false, true, &index)];
    out.extend(blocks);
    (out, first)
}

/// The image bytes.
pub fn build(f: &Fixture) -> Vec<u8> {
    let recs = records(&f.entries);
    // Block plan: 0 superblock, 1 checkpoint copy, 2 container omap,
    // 3.. container omap tree (one leaf), then the volumes.
    let mut blocks: Vec<Vec<u8>> = vec![vec![0u8; BS]; 4];
    let system_vol = 4u64;
    let data_vol = 5u64;
    let vol_omap = 6u64;
    let fs_first = 7u64;
    // The file-system tree: leaves of `leaf_cap` records, a root above
    // them when needed. Virtual ids from FS_ROOT_OID.
    let chunks: Vec<&[Rec]> = recs.chunks(f.leaf_cap.max(1)).collect();
    let mut fs_blocks = Vec::new();
    let mut omap_map = Vec::new();
    if chunks.len() == 1 {
        let kv: Vec<_> = chunks[0]
            .iter()
            .map(|r| (r.key.clone(), r.val.clone()))
            .collect();
        fs_blocks.push(node(FS_ROOT_OID, true, true, false, &kv));
        omap_map.push((FS_ROOT_OID, XID, fs_first));
    } else {
        let mut index = Vec::new();
        for (i, c) in chunks.iter().enumerate() {
            let oid = FS_ROOT_OID + 1 + i as u64;
            let kv: Vec<_> = c.iter().map(|r| (r.key.clone(), r.val.clone())).collect();
            fs_blocks.push(node(oid, false, true, false, &kv));
            omap_map.push((oid, XID, fs_first + 1 + i as u64));
            let mut child = vec![0u8; 8];
            put64(&mut child, 0, oid);
            index.push((c[0].key.clone(), child));
        }
        fs_blocks.insert(0, node(FS_ROOT_OID, true, false, false, &index));
        omap_map.push((FS_ROOT_OID, XID, fs_first));
    }
    // An older version of the root at a block that holds garbage, and a
    // newer one than the volume knows: the lookup must take neither.
    let garbage = fs_first + fs_blocks.len() as u64;
    omap_map.push((FS_ROOT_OID, XID - 1, garbage));
    omap_map.push((FS_ROOT_OID, XID + 5, garbage));
    let omap_tree_first = garbage + 1;
    let (omap_blocks, omap_root) = omap_tree(omap_tree_first, omap_map, f.omap_cap);

    // The container superblock and its checkpoint copy.
    let mut sb = vec![0u8; BS];
    put64(&mut sb, OBJ_OID_OFF, 1);
    put64(&mut sb, OBJ_XID_OFF, XID);
    put32(&mut sb, OBJ_TYPE_OFF, OBJECT_TYPE_NX_SUPERBLOCK);
    put32(&mut sb, NX_MAGIC_OFF, NX_MAGIC);
    put32(&mut sb, NX_BLOCK_SIZE_OFF, BS as u32);
    put32(&mut sb, NX_XP_DESC_BLOCKS_OFF, 1);
    put64(&mut sb, NX_XP_DESC_BASE_OFF, 1);
    put64(&mut sb, NX_OMAP_OID_OFF, 2);
    put64(&mut sb, NX_FS_OID_OFF, SYSTEM_VOL_OID);
    put64(&mut sb, NX_FS_OID_OFF + 8, DATA_VOL_OID);
    set_checksum(&mut sb);
    blocks[0] = sb.clone();
    blocks[1] = sb;
    let mut omap = vec![0u8; BS];
    put32(&mut omap, OBJ_TYPE_OFF, 0x0B);
    put64(&mut omap, OMAP_TREE_OID_OFF, 3);
    set_checksum(&mut omap);
    blocks[2] = omap;
    blocks[3] = node(
        3,
        true,
        true,
        true,
        &[
            (omap_key(SYSTEM_VOL_OID, XID), omap_val(system_vol)),
            (omap_key(DATA_VOL_OID, XID), omap_val(data_vol)),
        ],
    );
    let volume = |role: u16, omap: u64| {
        let mut v = vec![0u8; BS];
        put64(&mut v, OBJ_XID_OFF, XID);
        put32(&mut v, OBJ_TYPE_OFF, 0x0D);
        put32(&mut v, APFS_MAGIC_OFF, APFS_MAGIC);
        put64(&mut v, APFS_OMAP_OID_OFF, omap);
        put64(&mut v, APFS_ROOT_TREE_OID_OFF, FS_ROOT_OID);
        put16(&mut v, APFS_ROLE_OFF, role);
        set_checksum(&mut v);
        v
    };
    // The System volume points at an object map that does not exist: a
    // walk that picks it by mistake fails.
    blocks.push(volume(0x0001, 9999));
    blocks.push(volume(ROLE_DATA, vol_omap));
    let mut vomap = vec![0u8; BS];
    put32(&mut vomap, OBJ_TYPE_OFF, 0x0B);
    put64(&mut vomap, OMAP_TREE_OID_OFF, omap_root);
    set_checksum(&mut vomap);
    blocks.push(vomap);
    blocks.extend(fs_blocks);
    blocks.push(vec![0xA5; BS]);
    blocks.extend(omap_blocks);
    let container: Vec<u8> = blocks.concat();
    if !f.gpt {
        return container;
    }
    let mut disk = vec![0u8; GPT_START_LBA as usize * 512];
    disk[512..520].copy_from_slice(b"EFI PART");
    put64(&mut disk, 512 + 72, 2);
    put32(&mut disk, 512 + 80, 4);
    put32(&mut disk, 512 + 84, 128);
    // Entry 0: another partition type (the iBoot system container).
    let e0 = 1024;
    disk[e0..e0 + 16].copy_from_slice(&[
        0x61, 0x69, 0x64, 0x69, 0x67, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC,
        0xAC,
    ]);
    put64(&mut disk, e0 + 32, 2);
    let e1 = e0 + 128;
    disk[e1..e1 + 16].copy_from_slice(&APFS_PART_GUID);
    put64(&mut disk, e1 + 32, GPT_START_LBA);
    disk.extend(container);
    disk
}

/// Write the image of `f` to `path`.
pub fn write(f: &Fixture, path: &std::path::Path) {
    std::fs::write(path, build(f)).unwrap();
}

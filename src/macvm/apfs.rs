// SPDX-FileCopyrightText: Copyright The Lima Authors
// SPDX-FileCopyrightText: Copyright the paddock authors
// SPDX-License-Identifier: Apache-2.0
//
// Ported from lima-vm/lima, pkg/apfs (chown.go, fletcher64.go, types.go),
// under the Apache License 2.0; see NOTICE at the top of the repository.
// Changes from the original: Rust instead of Go; an owner reader next to
// the owner writer; an inode that already has the wanted owner is left as
// it is, so a second run is a no-op; the errors go through the text module.

//! The owners of a few files on an unmounted APFS disk image,
//! platform-neutral.
//!
//! A plain user writes files into the Data volume of a guest disk through a
//! mount without owners (`hdiutil attach -owners off`). APFS then stores
//! the owner 99:99 for each new inode. launchd wants root as the owner of a
//! LaunchDaemon plist, and a plain user cannot `chown`. So, after the
//! detach, this module walks the on-disk structures of the image file (the
//! GPT, the container superblock, the object maps, the file-system B-tree)
//! to the inode records of the named paths, sets their owner and group
//! fields and writes the block back with a new Fletcher-64 checksum. It
//! reads only the structures that it needs, at the byte offsets of Apple's
//! "Apple File System Reference".
//!
//! Every count and offset read from the image is checked against the block
//! before use: a damaged or crafted image is an error, never a panic or a
//! write outside the block.

use super::text;
use anyhow::Result;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::Path;

/// The role of the Data volume (`APFS_VOL_ROLE_DATA`).
pub const ROLE_DATA: u16 = 0x0040;
/// The owner and group that APFS stores for a file made through a mount
/// without owners.
pub const NOOWNERS_ID: u32 = 99;

pub(crate) const NX_MAGIC: u32 = 0x4253_584E; // "NXSB"
pub(crate) const APFS_MAGIC: u32 = 0x4253_5041; // "APSB"
pub(crate) const OBJECT_TYPE_NX_SUPERBLOCK: u32 = 0x01;
pub(crate) const OBJECT_TYPE_BTREE: u32 = 0x02;
pub(crate) const OBJECT_TYPE_BTREE_NODE: u32 = 0x03;
const OBJ_TYPE_MASK: u32 = 0x0000_FFFF;

pub(crate) const BTNODE_ROOT: u16 = 0x0001;
pub(crate) const BTNODE_LEAF: u16 = 0x0002;
pub(crate) const BTNODE_FIXED_KV_SIZE: u16 = 0x0004;

pub(crate) const TYPE_INODE: u64 = 3;
pub(crate) const TYPE_DIR_REC: u64 = 9;
pub(crate) const OBJ_ID_MASK: u64 = 0x0FFF_FFFF_FFFF_FFFF;
pub(crate) const OBJ_TYPE_SHIFT: u32 = 60;
pub(crate) const ROOT_DIR_INODE: u64 = 2;

/// `NX_MAXIMUM_BLOCK_SIZE`: a bogus block size never sizes a buffer.
const MAX_BLOCK_SIZE: u32 = 65536;

// nx_superblock_t
pub(crate) const NX_MAGIC_OFF: usize = 32;
pub(crate) const NX_BLOCK_SIZE_OFF: usize = 36;
pub(crate) const NX_XP_DESC_BLOCKS_OFF: usize = 104;
pub(crate) const NX_XP_DESC_BASE_OFF: usize = 112;
pub(crate) const NX_OMAP_OID_OFF: usize = 160;
pub(crate) const NX_FS_OID_OFF: usize = 184;
const NX_MAX_FILE_SYSTEMS: usize = 100;
const NX_XP_DESC_BLOCKS_MASK: u32 = 0x7FFF_FFFF;

// obj_phys_t
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const OBJ_OID_OFF: usize = 8;
pub(crate) const OBJ_XID_OFF: usize = 16;
pub(crate) const OBJ_TYPE_OFF: usize = 24;

// apfs_superblock_t
pub(crate) const APFS_MAGIC_OFF: usize = 32;
pub(crate) const APFS_OMAP_OID_OFF: usize = 128;
pub(crate) const APFS_ROOT_TREE_OID_OFF: usize = 136;
pub(crate) const APFS_ROLE_OFF: usize = 964;

// omap_phys_t
pub(crate) const OMAP_TREE_OID_OFF: usize = 48;

// btree_node_phys_t
pub(crate) const BTN_FLAGS_OFF: usize = 32;
pub(crate) const BTN_NKEYS_OFF: usize = 36;
pub(crate) const BTN_TABLE_SPACE_OFF: usize = 40;
pub(crate) const BTN_DATA_OFF: usize = 56;
/// `btree_info_t` at the end of a root node.
pub(crate) const BTREE_INFO_SIZE: usize = 40;

// j_inode_val_t
pub(crate) const INODE_PRIVATE_ID_OFF: usize = 8;
pub(crate) const INODE_OWNER_OFF: usize = 72;
pub(crate) const INODE_GROUP_OFF: usize = 76;

// GPT
const GPT_SECTOR: u64 = 512;
const GPT_MIN_ENTRY: u32 = 128;
const GPT_MAX_ENTRY: u32 = 4096;
const GPT_MAX_TABLE: u64 = 4 << 20;
/// 7C3457EF-0000-11AA-AA11-00306543ECAC as stored on disk: the APFS
/// container. The iBoot system container and the recovery container of a
/// Mac disk have other type GUIDs, so the first match is the main one.
pub(crate) const APFS_PART_GUID: [u8; 16] = [
    0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC,
];

fn bad(what: &str) -> anyhow::Error {
    anyhow::Error::msg(text::apfs_damaged(what))
}

fn bad_n(what: &str, n: u64) -> anyhow::Error {
    anyhow::Error::msg(text::apfs_damaged_at(what, n))
}

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().expect("four bytes"))
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().expect("eight bytes"))
}

fn put_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// The APFS Fletcher-64 checksum of a block, over `block[8..]`.
pub fn fletcher64(block: &[u8]) -> u64 {
    const MOD: u64 = 0xFFFF_FFFF;
    let (mut s1, mut s2) = (0u64, 0u64);
    for chunk in block[8..].as_chunks::<4>().0 {
        s1 = (s1 + u64::from(u32::from_le_bytes(*chunk))) % MOD;
        s2 = (s2 + s1) % MOD;
    }
    let low = MOD - ((s1 + s2) % MOD);
    let high = MOD - ((s1 + low) % MOD);
    (high << 32) | low
}

/// True when the stored checksum of a block is right.
pub fn checksum_ok(block: &[u8]) -> bool {
    block.len() >= 8 && u64_at(block, 0) == fletcher64(block)
}

/// Store a new checksum into a block.
pub fn set_checksum(block: &mut [u8]) {
    let c = fletcher64(block);
    block[..8].copy_from_slice(&c.to_le_bytes());
}

/// The owner and group of each path on the volume of `role`, in order.
/// The paths are relative to the volume root, such as
/// `Library/LaunchDaemons/x.plist`.
pub fn owners(image: &Path, role: u16, paths: &[&str]) -> Result<Vec<(u32, u32)>> {
    let c = Container::open(image, false)?;
    let vol = c.volume(role)?;
    let root = c.omap_lookup(vol.omap_tree, vol.root_tree_oid, vol.xid)?;
    let mut out = Vec::new();
    for p in paths {
        let ino = c.resolve(root, &vol, p)?;
        let (_, blk, val) = c.find_inode(root, &vol, ino)?;
        out.push((
            u32_at(&blk, val + INODE_OWNER_OFF),
            u32_at(&blk, val + INODE_GROUP_OFF),
        ));
    }
    Ok(out)
}

/// Set the owner and group of each path on the volume of `role` to
/// `uid:gid`. An inode must have the owner of a mount without owners
/// ([`NOOWNERS_ID`]) or already `uid:gid`; any other owner is an error
/// before the write, so the call never changes a file that paddock did
/// not make. Returns the number of inodes written.
pub fn chown(image: &Path, role: u16, uid: u32, gid: u32, paths: &[&str]) -> Result<usize> {
    let c = Container::open(image, true)?;
    let vol = c.volume(role)?;
    let root = c.omap_lookup(vol.omap_tree, vol.root_tree_oid, vol.xid)?;
    // Every path resolves and every owner is checked before the first
    // write: a missing path leaves the image as it was.
    let mut found = Vec::new();
    for p in paths {
        let ino = c.resolve(root, &vol, p)?;
        let (addr, blk, val) = c.find_inode(root, &vol, ino)?;
        let now = (
            u32_at(&blk, val + INODE_OWNER_OFF),
            u32_at(&blk, val + INODE_GROUP_OFF),
        );
        if now == (uid, gid) {
            continue;
        }
        if now != (NOOWNERS_ID, NOOWNERS_ID) {
            return Err(anyhow::Error::msg(text::apfs_unexpected_owner(
                p, now.0, now.1,
            )));
        }
        found.push((ino, addr));
    }
    let mut written = 0;
    for (ino, addr) in found {
        // Read the block again: two inodes can share a leaf.
        let mut blk = c.read_block(addr)?;
        let node = Node::parse(&blk, c.block_size)?;
        let val = node
            .inode_value(&blk, ino)?
            .ok_or_else(|| bad_n("inode gone from its leaf", ino))?;
        put_u32(&mut blk, val + INODE_OWNER_OFF, uid);
        put_u32(&mut blk, val + INODE_GROUP_OFF, gid);
        set_checksum(&mut blk);
        c.write_block(addr, &blk)?;
        written += 1;
    }
    c.f.sync_all().map_err(|e| bad(&e.to_string()))?;
    Ok(written)
}

/// An open container: the file, where the container starts in it, and the
/// block size.
pub(crate) struct Container {
    f: File,
    base: u64,
    block_size: u32,
}

/// What the walk needs of a volume.
pub(crate) struct Volume {
    omap_tree: u64,
    root_tree_oid: u64,
    xid: u64,
}

impl Container {
    fn open(path: &Path, write: bool) -> Result<Container> {
        let f = OpenOptions::new()
            .read(true)
            .write(write)
            .open(path)
            .map_err(|e| anyhow::Error::new(e).context(text::cannot_open(path)))?;
        let mut hdr = vec![0u8; 4096];
        f.read_exact_at(&mut hdr, 0)
            .map_err(|_| bad("the image is shorter than one block"))?;
        let base = if u32_at(&hdr, NX_MAGIC_OFF) == NX_MAGIC {
            0
        } else {
            let off = gpt_apfs_offset(&f)?;
            f.read_exact_at(&mut hdr, off)
                .map_err(|_| bad("the APFS partition is past the end"))?;
            if u32_at(&hdr, NX_MAGIC_OFF) != NX_MAGIC {
                return Err(bad("the APFS partition has no container superblock"));
            }
            off
        };
        let block_size = u32_at(&hdr, NX_BLOCK_SIZE_OFF);
        if !(4096..=MAX_BLOCK_SIZE).contains(&block_size) {
            return Err(bad_n("invalid block size", u64::from(block_size)));
        }
        Ok(Container {
            f,
            base,
            block_size,
        })
    }

    fn offset(&self, addr: u64) -> Result<u64> {
        addr.checked_mul(u64::from(self.block_size))
            .and_then(|o| o.checked_add(self.base))
            .ok_or_else(|| bad_n("block address out of range", addr))
    }

    fn read_block(&self, addr: u64) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; self.block_size as usize];
        self.f
            .read_exact_at(&mut buf, self.offset(addr)?)
            .map_err(|_| bad_n("cannot read block", addr))?;
        Ok(buf)
    }

    fn write_block(&self, addr: u64, data: &[u8]) -> Result<()> {
        self.f
            .write_all_at(data, self.offset(addr)?)
            .map_err(|e| anyhow::Error::msg(text::apfs_write_failed(addr, &e.to_string())))
    }

    /// The container superblock with the highest transaction id in the
    /// checkpoint descriptor area.
    fn latest_superblock(&self) -> Result<Vec<u8>> {
        let b0 = self.read_block(0)?;
        if !checksum_ok(&b0) {
            return Err(bad("block 0 has a wrong checksum"));
        }
        let base = u64_at(&b0, NX_XP_DESC_BASE_OFF);
        let count = u32_at(&b0, NX_XP_DESC_BLOCKS_OFF) & NX_XP_DESC_BLOCKS_MASK;
        let mut best: Option<(u64, Vec<u8>)> = None;
        for i in 0..u64::from(count) {
            let Ok(blk) = self.read_block(base.saturating_add(i)) else {
                continue;
            };
            if !checksum_ok(&blk)
                || u32_at(&blk, OBJ_TYPE_OFF) & OBJ_TYPE_MASK != OBJECT_TYPE_NX_SUPERBLOCK
                || u32_at(&blk, NX_MAGIC_OFF) != NX_MAGIC
            {
                continue;
            }
            let xid = u64_at(&blk, OBJ_XID_OFF);
            if best.as_ref().is_none_or(|(b, _)| xid > *b) {
                best = Some((xid, blk));
            }
        }
        best.map(|(_, b)| b)
            .ok_or_else(|| bad("no valid container superblock in the checkpoint area"))
    }

    /// The volume with `role`.
    fn volume(&self, role: u16) -> Result<Volume> {
        let sb = self.latest_superblock()?;
        let omap = self.read_block(u64_at(&sb, NX_OMAP_OID_OFF))?;
        let tree = u64_at(&omap, OMAP_TREE_OID_OFF);
        let xid = u64_at(&sb, OBJ_XID_OFF);
        for i in 0..NX_MAX_FILE_SYSTEMS {
            let oid = u64_at(&sb, NX_FS_OID_OFF + i * 8);
            if oid == 0 {
                continue;
            }
            let Ok(phys) = self.omap_lookup(tree, oid, xid) else {
                continue;
            };
            let Ok(vb) = self.read_block(phys) else {
                continue;
            };
            if !checksum_ok(&vb) || u32_at(&vb, APFS_MAGIC_OFF) != APFS_MAGIC {
                continue;
            }
            if u16_at(&vb, APFS_ROLE_OFF) != role {
                continue;
            }
            let vomap = self.read_block(u64_at(&vb, APFS_OMAP_OID_OFF))?;
            return Ok(Volume {
                omap_tree: u64_at(&vomap, OMAP_TREE_OID_OFF),
                root_tree_oid: u64_at(&vb, APFS_ROOT_TREE_OID_OFF),
                xid: u64_at(&vb, OBJ_XID_OFF),
            });
        }
        Err(bad_n("no volume with the role", u64::from(role)))
    }

    /// The physical address of virtual object `oid`: the entry of the
    /// object map with the highest xid at or below `max_xid`.
    fn omap_lookup(&self, tree: u64, oid: u64, max_xid: u64) -> Result<u64> {
        let mut blk = self.read_block(tree)?;
        for _depth in 0..MAX_DEPTH {
            if !checksum_ok(&blk) {
                return Err(bad("an object map node has a wrong checksum"));
            }
            let node = Node::parse(&blk, self.block_size)?;
            if node.leaf {
                let mut best: Option<(u64, u64)> = None;
                for i in 0..node.nkeys {
                    let (k, v) = node.toc(&blk, i);
                    let ks = node
                        .key_at(k, 16)
                        .ok_or_else(|| bad("an object map key is out of range"))?;
                    let (eoid, exid) = (u64_at(&blk, ks), u64_at(&blk, ks + 8));
                    if eoid == oid && exid <= max_xid {
                        let vs = node
                            .value_at(v, 16)
                            .ok_or_else(|| bad("an object map value is out of range"))?;
                        if best.is_none_or(|(x, _)| exid > x) {
                            best = Some((exid, u64_at(&blk, vs + 8)));
                        }
                    }
                }
                return best
                    .map(|(_, p)| p)
                    .ok_or_else(|| bad_n("no object map entry for the object", oid));
            }
            let mut child = 0;
            for i in 0..node.nkeys {
                let (k, _) = node.toc(&blk, i);
                let ks = node
                    .key_at(k, 16)
                    .ok_or_else(|| bad("an object map key is out of range"))?;
                if (u64_at(&blk, ks), u64_at(&blk, ks + 8)) <= (oid, max_xid) {
                    child = i;
                } else {
                    break;
                }
            }
            let (_, v) = node.toc(&blk, child);
            let vs = node
                .value_at(v, 8)
                .ok_or_else(|| bad("an object map child is out of range"))?;
            blk = self.read_block(u64_at(&blk, vs))?;
        }
        Err(bad("the object map is deeper than any real one"))
    }

    /// The inode number of `path` from the root folder.
    fn resolve(&self, root: u64, vol: &Volume, path: &str) -> Result<u64> {
        let mut cnid = ROOT_DIR_INODE;
        for name in path.split('/').filter(|n| !n.is_empty()) {
            cnid = self
                .lookup(root, vol, cnid, name)?
                .ok_or_else(|| anyhow::Error::msg(text::apfs_path_missing(path, name)))?;
        }
        Ok(cnid)
    }

    /// Walk from the root node to the leaf that holds `target`, choosing
    /// at each internal node the last key that `before_or_at` accepts.
    fn descend(
        &self,
        root: u64,
        vol: &Volume,
        mut before_or_at: impl FnMut(&Node, &[u8], usize) -> Result<bool>,
    ) -> Result<(u64, Vec<u8>, Node)> {
        let mut addr = root;
        let mut blk = self.read_block(addr)?;
        for _depth in 0..MAX_DEPTH {
            if !checksum_ok(&blk) {
                return Err(bad("a file-system node has a wrong checksum"));
            }
            let node = Node::parse(&blk, self.block_size)?;
            if node.leaf {
                return Ok((addr, blk, node));
            }
            let mut child = 0;
            for i in 0..node.nkeys {
                let (k, _) = node.toc(&blk, i);
                if before_or_at(&node, &blk, k)? {
                    child = i;
                } else {
                    break;
                }
            }
            let (_, v) = node.toc(&blk, child);
            let vs = node
                .value_at(v, 8)
                .ok_or_else(|| bad("a file-system child is out of range"))?;
            addr = self.omap_lookup(vol.omap_tree, u64_at(&blk, vs), vol.xid)?;
            blk = self.read_block(addr)?;
        }
        Err(bad("the file-system tree is deeper than any real one"))
    }

    /// The file id of `name` in the folder `parent`.
    fn lookup(&self, root: u64, vol: &Volume, parent: u64, name: &str) -> Result<Option<u64>> {
        let header = (TYPE_DIR_REC << OBJ_TYPE_SHIFT) | (parent & OBJ_ID_MASK);
        let hash = name_hash(name);
        let (_, blk, node) = self.descend(root, vol, |n, b, k| {
            Ok(n.compare_drec(b, k, header, name, hash)
                .ok_or_else(|| bad("a file-system key is out of range"))?
                .is_le())
        })?;
        for i in 0..node.nkeys {
            let (k, v) = node.toc(&blk, i);
            let ks = node
                .key_at(k, 8)
                .ok_or_else(|| bad("a file-system key is out of range"))?;
            if u64_at(&blk, ks) != header || node.drec_name(&blk, ks + 8) != name {
                continue;
            }
            let vs = node
                .value_at(v, 8)
                .ok_or_else(|| bad("a file-system value is out of range"))?;
            return Ok(Some(u64_at(&blk, vs)));
        }
        Ok(None)
    }

    /// The leaf of inode `ino`: its address, its bytes and the offset of
    /// the inode value in it.
    fn find_inode(&self, root: u64, vol: &Volume, ino: u64) -> Result<(u64, Vec<u8>, usize)> {
        let header = (TYPE_INODE << OBJ_TYPE_SHIFT) | (ino & OBJ_ID_MASK);
        let (addr, blk, node) = self.descend(root, vol, |n, b, k| {
            let ks = n
                .key_at(k, 8)
                .ok_or_else(|| bad("a file-system key is out of range"))?;
            Ok(compare_fs_header(u64_at(b, ks), header).is_le())
        })?;
        let val = node
            .inode_value(&blk, ino)?
            .ok_or_else(|| bad_n("inode not found in the file-system tree", ino))?;
        Ok((addr, blk, val))
    }
}

/// No real tree is this deep; a loop in a damaged image ends here.
const MAX_DEPTH: usize = 64;

/// The byte offset of the first APFS container partition in a GPT.
fn gpt_apfs_offset(f: &File) -> Result<u64> {
    let mut hdr = [0u8; 512];
    f.read_exact_at(&mut hdr, GPT_SECTOR)
        .map_err(|_| bad("no GPT header"))?;
    if &hdr[0..8] != b"EFI PART" {
        return Err(bad("neither an APFS container nor a GPT disk"));
    }
    let entries_lba = u64_at(&hdr, 72);
    let count = u32_at(&hdr, 80);
    let size = u32_at(&hdr, 84);
    if !(GPT_MIN_ENTRY..=GPT_MAX_ENTRY).contains(&size) {
        return Err(bad_n("GPT entry size out of range", u64::from(size)));
    }
    if count == 0 || u64::from(count) * u64::from(size) > GPT_MAX_TABLE {
        return Err(bad_n("GPT entry count out of range", u64::from(count)));
    }
    let mut entry = vec![0u8; size as usize];
    for i in 0..u64::from(count) {
        let off = entries_lba
            .checked_mul(GPT_SECTOR)
            .and_then(|o| o.checked_add(i * u64::from(size)))
            .ok_or_else(|| bad("GPT table out of range"))?;
        f.read_exact_at(&mut entry, off)
            .map_err(|_| bad_n("cannot read GPT entry", i))?;
        if entry[0..16] == APFS_PART_GUID {
            return u64_at(&entry, 32)
                .checked_mul(GPT_SECTOR)
                .ok_or_else(|| bad("GPT partition start out of range"));
        }
        if entry[0..16].iter().all(|b| *b == 0) {
            break;
        }
    }
    Err(bad("no APFS partition in the GPT"))
}

/// The order of file-system keys: the object id, then the type.
pub(crate) fn compare_fs_header(a: u64, b: u64) -> std::cmp::Ordering {
    ((a & OBJ_ID_MASK), a >> OBJ_TYPE_SHIFT).cmp(&((b & OBJ_ID_MASK), b >> OBJ_TYPE_SHIFT))
}

/// The 22-bit hash of a folder entry name: CRC-32C of the lower-case name
/// as UTF-32LE, complemented. The names of paddock are ASCII, where lower
/// case is the case folding of APFS.
pub fn name_hash(name: &str) -> u32 {
    let utf32: Vec<u8> = name
        .to_lowercase()
        .chars()
        .flat_map(|c| (c as u32).to_le_bytes())
        .collect();
    crc32c_raw(&utf32) & 0x3F_FFFF
}

/// CRC-32C (Castagnoli) with the start value `!0` and no final
/// complement: the complement of the standard check value.
pub fn crc32c_raw(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// A bounds-checked view of a B-tree node: every offset that it gives
/// lies inside the block.
pub(crate) struct Node {
    nkeys: u32,
    toc_start: usize,
    key_start: usize,
    value_end: usize,
    leaf: bool,
    fixed: bool,
}

impl Node {
    fn parse(blk: &[u8], block_size: u32) -> Result<Node> {
        let kind = u32_at(blk, OBJ_TYPE_OFF) & OBJ_TYPE_MASK;
        if kind != OBJECT_TYPE_BTREE && kind != OBJECT_TYPE_BTREE_NODE {
            return Err(bad_n("not a B-tree node, type", u64::from(kind)));
        }
        let flags = u16_at(blk, BTN_FLAGS_OFF);
        let mut value_end = block_size as usize;
        if flags & BTNODE_ROOT != 0 {
            value_end -= BTREE_INFO_SIZE;
        }
        let toc_off = usize::from(u16_at(blk, BTN_TABLE_SPACE_OFF));
        let toc_len = usize::from(u16_at(blk, BTN_TABLE_SPACE_OFF + 2));
        let fixed = flags & BTNODE_FIXED_KV_SIZE != 0;
        let entry = if fixed { 4 } else { 8 };
        let nkeys = u32_at(blk, BTN_NKEYS_OFF);
        let toc_start = BTN_DATA_OFF + toc_off;
        let key_start = toc_start + toc_len;
        if key_start > value_end || nkeys as usize > toc_len / entry {
            return Err(bad("a B-tree table of contents is out of range"));
        }
        let leaf = flags & BTNODE_LEAF != 0;
        if !leaf && nkeys == 0 {
            return Err(bad("a B-tree internal node has no keys"));
        }
        Ok(Node {
            nkeys,
            toc_start,
            key_start,
            value_end,
            leaf,
            fixed,
        })
    }

    /// The key and value offsets of entry `i < nkeys`.
    fn toc(&self, blk: &[u8], i: u32) -> (usize, usize) {
        let i = i as usize;
        if self.fixed {
            let o = self.toc_start + i * 4;
            (usize::from(u16_at(blk, o)), usize::from(u16_at(blk, o + 2)))
        } else {
            let o = self.toc_start + i * 8;
            (usize::from(u16_at(blk, o)), usize::from(u16_at(blk, o + 4)))
        }
    }

    fn key_at(&self, k: usize, size: usize) -> Option<usize> {
        let start = self.key_start + k;
        (start + size <= self.value_end).then_some(start)
    }

    fn value_at(&self, v: usize, size: usize) -> Option<usize> {
        let start = self.value_end.checked_sub(v)?;
        (start >= self.key_start && start + size <= self.value_end).then_some(start)
    }

    /// The offset of the value of inode `ino` in this leaf, checked: the
    /// value holds the owner fields and its private id is `ino`.
    fn inode_value(&self, blk: &[u8], ino: u64) -> Result<Option<usize>> {
        let header = (TYPE_INODE << OBJ_TYPE_SHIFT) | (ino & OBJ_ID_MASK);
        for i in 0..self.nkeys {
            let (k, v) = self.toc(blk, i);
            let ks = self
                .key_at(k, 8)
                .ok_or_else(|| bad("a file-system key is out of range"))?;
            if u64_at(blk, ks) != header {
                continue;
            }
            let vs = self
                .value_at(v, INODE_GROUP_OFF + 4)
                .ok_or_else(|| bad_n("the value of the inode is out of range", ino))?;
            if u64_at(blk, vs + INODE_PRIVATE_ID_OFF) != ino {
                return Err(bad_n("the inode has another private id", ino));
            }
            return Ok(Some(vs));
        }
        Ok(None)
    }

    /// Compare the folder-entry key at `k` with (`header`, `name`).
    fn compare_drec(
        &self,
        blk: &[u8],
        k: usize,
        header: u64,
        name: &str,
        hash: u32,
    ) -> Option<std::cmp::Ordering> {
        let ks = self.key_at(k, 12)?;
        let c = compare_fs_header(u64_at(blk, ks), header);
        if c.is_ne() {
            return Some(c);
        }
        let v = u32_at(blk, ks + 8);
        let entry = self.drec_name(blk, ks + 8);
        if v & 0xFFFF_FC00 != 0 {
            let c = ((v >> 10) & 0x3F_FFFF).cmp(&hash);
            if c.is_ne() {
                return Some(c);
            }
            return Some(entry.to_lowercase().cmp(&name.to_lowercase()));
        }
        Some(entry.as_str().cmp(name))
    }

    /// The name of a folder-entry key whose length field is at `at`: the
    /// hashed form (`name_len_and_hash`, four bytes) or the plain one (two
    /// bytes). An out-of-range length gives an empty name.
    fn drec_name(&self, blk: &[u8], at: usize) -> String {
        if at + 4 > self.value_end {
            return String::new();
        }
        let v = u32_at(blk, at);
        let (len, start) = if v & 0xFFFF_FC00 != 0 {
            ((v & 0x3FF) as usize, at + 4)
        } else {
            (usize::from(u16_at(blk, at)), at + 2)
        };
        if len <= 1 || start + len - 1 > self.value_end {
            return String::new();
        }
        String::from_utf8_lossy(&blk[start..start + len - 1]).into_owned()
    }
}

#[cfg(test)]
#[path = "apfs_tests.rs"]
mod tests;

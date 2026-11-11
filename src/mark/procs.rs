//! The process table on macOS, for the marker and for the start of a
//! terminal run.

use super::ProcTable;

/// Each process and its parent, from the kernel.
pub fn proc_table() -> ProcTable {
    let mut out = ProcTable::new();
    // SAFETY: proc_listallpids fills at most the given number of bytes.
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return out;
    }
    let mut pids = vec![0 as libc::pid_t; n as usize + 64];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    for &pid in pids.iter().take(n.max(0) as usize) {
        if pid <= 0 {
            continue;
        }
        // SAFETY: proc_bsdinfo is plain data, and the size matches.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let r = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if r == size {
            out.insert(pid as u32, info.pbi_ppid);
        }
    }
    out
}

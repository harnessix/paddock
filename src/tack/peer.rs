//! Who is on the other end of a tackd connection, from the kernel.
//!
//! - Linux: `SO_PEERCRED` gives the uid and pid at `connect()`. A pid can
//!   be reused once its process ends, so tackd also takes a pidfd of the
//!   peer (`SO_PEERPIDFD`, Linux 6.5, or `pidfd_open` on older kernels)
//!   and checks after reading the process table that the process is still
//!   alive: a live process keeps its pid, so what tackd read was that
//!   process.
//! - macOS: `getpeereid` gives the uid, `LOCAL_PEERPID` the pid.

use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

/// The kernel's view of a peer.
#[derive(Debug)]
pub struct Cred {
    pub uid: u32,
    pub pid: u32,
    #[cfg(target_os = "linux")]
    pidfd: Option<std::os::fd::OwnedFd>,
}

impl Cred {
    pub fn peer(&self) -> super::policy::Peer {
        super::policy::Peer {
            uid: self.uid,
            pid: self.pid,
        }
    }

    /// Whether the peer process is still alive (so its pid still names
    /// it). Where the kernel cannot tell, true.
    pub fn alive(&self) -> bool {
        #[cfg(target_os = "linux")]
        if let Some(fd) = &self.pidfd {
            use std::os::fd::AsRawFd;
            // SAFETY: signal 0 checks the process without sending anything.
            let r = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    0,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            };
            if r == 0 {
                return true;
            }
            // EPERM: alive, but another user's (cannot happen as root).
            return std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        }
        true
    }
}

/// SO_PEERPIDFD where the value is the generic one.
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "x86",
        target_arch = "arm",
        target_arch = "riscv64"
    )
))]
const SO_PEERPIDFD: libc::c_int = 77;

#[cfg(target_os = "linux")]
pub fn of(s: &UnixStream) -> std::io::Result<Cred> {
    use std::os::fd::{FromRawFd, OwnedFd};
    let fd = s.as_raw_fd();
    let mut c: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: c and len describe a ucred buffer.
    let r = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut c as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if c.pid <= 0 {
        return Err(std::io::Error::other(
            "the kernel reports no pid for the peer (it is in a pid namespace that tackd cannot see)",
        ));
    }
    let mut pidfd: Option<OwnedFd> = None;
    #[cfg(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "x86",
        target_arch = "arm",
        target_arch = "riscv64"
    ))]
    {
        let mut pfd: libc::c_int = -1;
        let mut plen = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: pfd and plen describe an int buffer.
        let r = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                SO_PEERPIDFD,
                &mut pfd as *mut _ as *mut libc::c_void,
                &mut plen,
            )
        };
        if r == 0 && pfd >= 0 {
            // SAFETY: the kernel gave us this new descriptor.
            pidfd = Some(unsafe { OwnedFd::from_raw_fd(pfd) });
        }
    }
    if pidfd.is_none() {
        // SAFETY: pidfd_open takes a pid and flags.
        let r = unsafe { libc::syscall(libc::SYS_pidfd_open, c.pid, 0) };
        if r >= 0 {
            // SAFETY: a new descriptor from the kernel.
            pidfd = Some(unsafe { OwnedFd::from_raw_fd(r as libc::c_int) });
        }
    }
    Ok(Cred {
        uid: c.uid,
        pid: c.pid as u32,
        pidfd,
    })
}

#[cfg(target_os = "macos")]
pub fn of(s: &UnixStream) -> std::io::Result<Cred> {
    let fd = s.as_raw_fd();
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: two out-parameters of the right types.
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: pid and len describe a pid_t buffer.
    let r = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            &mut pid as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if pid <= 0 {
        return Err(std::io::Error::other(
            "the kernel reports no pid for the peer",
        ));
    }
    Ok(Cred {
        uid,
        pid: pid as u32,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn of(_s: &UnixStream) -> std::io::Result<Cred> {
    Err(std::io::Error::other("tackd runs on Linux and macOS"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_peer_of_a_socket_pair_is_this_process() {
        let (a, _b) = UnixStream::pair().unwrap();
        let c = of(&a).unwrap();
        assert_eq!(c.uid, unsafe { libc::geteuid() });
        assert_eq!(c.pid, std::process::id());
        assert!(c.alive());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_peer_that_ended_is_not_alive() {
        // A child connects, then exits; its pidfd reports it gone.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s");
        let l = std::os::unix::net::UnixListener::bind(&path).unwrap();
        // The address is built before fork: the child only makes
        // async-signal-safe calls.
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (i, b) in path.as_os_str().as_encoded_bytes().iter().enumerate() {
            addr.sun_path[i] = *b as libc::c_char;
        }
        let alen = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
        // SAFETY: the child calls socket, connect and _exit only.
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            unsafe {
                let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
                libc::connect(fd, &addr as *const _ as *const libc::sockaddr, alen);
                libc::_exit(0);
            }
        }
        let (conn, _) = l.accept().unwrap();
        let c = of(&conn).unwrap();
        assert_eq!(c.pid, pid as u32);
        let mut st = 0;
        unsafe { libc::waitpid(pid, &mut st, 0) };
        // Every kernel since 5.3 has pidfd_open.
        assert!(c.pidfd.is_some(), "no pidfd for the peer");
        assert!(!c.alive());
    }
}

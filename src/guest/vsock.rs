//! `AF_VSOCK` connections from the guest to the host (CID 2), with a
//! connect timeout. The socket is a plain Unix domain descriptor after
//! the connect, so the rest of the agent reads and writes a `UnixStream`.

use crate::macvm::text;
use std::io;
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// Connect to `port` of `cid` within `timeout`.
pub fn connect(cid: u32, port: u32, timeout: Duration) -> io::Result<UnixStream> {
    // SAFETY: a plain socket call.
    let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is ours; from_raw_fd takes its ownership, so an early
    // return closes it.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    // SAFETY: plain fcntl calls on our descriptor.
    unsafe {
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    stream.set_nonblocking(true)?;
    // SAFETY: sockaddr_vm is plain data; the length and family are set.
    let mut addr: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
    addr.svm_len = std::mem::size_of::<libc::sockaddr_vm>() as u8;
    addr.svm_family = libc::AF_VSOCK as libc::sa_family_t;
    addr.svm_port = port;
    addr.svm_cid = cid;
    // SAFETY: the address is valid and its length matches.
    let r = unsafe {
        libc::connect(
            fd,
            (&addr as *const libc::sockaddr_vm).cast(),
            std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
        )
    };
    if r != 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(e);
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        // SAFETY: one valid pollfd.
        let n = unsafe { libc::poll(&mut pfd, 1, ms) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                text::vsock_no_answer(port, timeout),
            ));
        }
        let mut err: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: the out buffer is an int of the given length.
        let r = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut err as *mut libc::c_int).cast(),
                &mut len,
            )
        };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        if err != 0 {
            return Err(io::Error::from_raw_os_error(err));
        }
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

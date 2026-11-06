//! File descriptors over a Unix socket (SCM_RIGHTS): the stand-in hands
//! its standard streams to tackd with the bytes of its start request, so
//! the program that tackd starts reads and writes the caller's terminal or
//! pipes, with no copy in between.
//!
//! The descriptors travel on the first byte of a `sendmsg`. The receiver
//! reads that byte with `recvmsg` and takes at most `max` descriptors; more,
//! or a control message that the kernel had to cut (MSG_CTRUNC), is an
//! error, and every descriptor that came is closed. Received descriptors
//! are close-on-exec.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// The most descriptors that one message carries.
pub const MAX_FDS: usize = 3;

/// Send `bytes`, with `fds` on the first byte. The rest of `bytes`, if the
/// kernel took only a part, follows as plain bytes.
pub fn send(sock: &UnixStream, bytes: &[u8], fds: &[RawFd]) -> io::Result<()> {
    if fds.len() > MAX_FDS {
        return Err(io::Error::other("too many descriptors"));
    }
    if bytes.is_empty() {
        return Err(io::Error::other("descriptors need at least one byte"));
    }
    let space = unsafe { libc::CMSG_SPACE((MAX_FDS * std::mem::size_of::<RawFd>()) as u32) };
    let mut control = vec![0u8; space as usize];
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut libc::c_void,
        iov_len: bytes.len(),
    };
    // SAFETY: a zeroed msghdr is valid; every pointer below points into
    // buffers that live until sendmsg returns.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if !fds.is_empty() {
        let len = std::mem::size_of_val(fds);
        msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = unsafe { libc::CMSG_SPACE(len as u32) } as _;
        // SAFETY: the control buffer holds one header and `len` bytes.
        unsafe {
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(len as u32) as _;
            std::ptr::copy_nonoverlapping(fds.as_ptr() as *const u8, libc::CMSG_DATA(c), len);
        }
    }
    let sent = loop {
        // SAFETY: msg is complete; MSG_NOSIGNAL is not on macOS, where
        // the stand-in ignores SIGPIPE instead.
        let r = unsafe { libc::sendmsg(sock.as_raw_fd(), &msg, flags_send()) };
        if r >= 0 {
            break r as usize;
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    };
    if sent < bytes.len() {
        use std::io::Write;
        let mut w = sock;
        w.write_all(&bytes[sent..])?;
    }
    Ok(())
}

fn flags_send() -> libc::c_int {
    #[cfg(target_os = "linux")]
    {
        libc::MSG_NOSIGNAL
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// Read up to `buf.len()` bytes and the descriptors that come with them.
/// Returns the number of bytes; 0 is the end of the stream.
pub fn recv(sock: &UnixStream, buf: &mut [u8], max: usize) -> io::Result<(usize, Vec<OwnedFd>)> {
    let max = max.min(MAX_FDS);
    // Room for more than `max`, so that too many is seen, not cut.
    let space = unsafe { libc::CMSG_SPACE(((MAX_FDS + 1) * std::mem::size_of::<RawFd>()) as u32) };
    let mut control = vec![0u8; space as usize];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr() as *mut libc::c_void,
        iov_len: buf.len(),
    };
    // SAFETY: as in send.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = space as _;
    let n = loop {
        // SAFETY: msg describes buffers that live until recvmsg returns.
        let r = unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg, flags_recv()) };
        if r >= 0 {
            break r as usize;
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    };
    let mut fds = Vec::new();
    // SAFETY: the kernel filled msg_control with msg_controllen bytes of
    // control messages; CMSG_NXTHDR stays inside them.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c);
                let len = (*c).cmsg_len as usize - (data as usize - c as usize);
                let count = len / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    let mut fd: RawFd = -1;
                    std::ptr::copy_nonoverlapping(
                        data.add(i * std::mem::size_of::<RawFd>()),
                        &mut fd as *mut RawFd as *mut u8,
                        std::mem::size_of::<RawFd>(),
                    );
                    if fd >= 0 {
                        fds.push(OwnedFd::from_raw_fd(fd));
                    }
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    #[cfg(not(target_os = "linux"))]
    for f in &fds {
        // SAFETY: f is an open descriptor that we own.
        unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    if msg.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(io::Error::other(
            "the descriptors did not fit (the message was cut)",
        ));
    }
    if fds.len() > max {
        return Err(io::Error::other(format!(
            "{} descriptors came; at most {max} are expected",
            fds.len()
        )));
    }
    Ok((n, fds))
}

fn flags_recv() -> libc::c_int {
    #[cfg(target_os = "linux")]
    {
        libc::MSG_CMSG_CLOEXEC
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn descriptors_travel_with_the_first_byte() {
        let (a, b) = UnixStream::pair().unwrap();
        let (mut pr, mut pw) = UnixStream::pair().unwrap();
        send(&a, b"hello", &[pw.as_raw_fd()]).unwrap();
        let mut buf = [0u8; 2];
        let (n, fds) = recv(&b, &mut buf, 3).unwrap();
        assert_eq!((n, &buf[..n]), (2, &b"he"[..]));
        assert_eq!(fds.len(), 1);
        // The rest of the bytes come without descriptors.
        let mut rest = [0u8; 3];
        let (n, more) = recv(&b, &mut rest, 3).unwrap();
        assert_eq!((&rest[..n], more.len()), (&b"llo"[..], 0));
        // The received descriptor is the same pipe end: close-on-exec.
        let got = UnixStream::from(fds.into_iter().next().unwrap());
        let flags = unsafe { libc::fcntl(got.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        (&got).write_all(b"x").unwrap();
        drop(got);
        pw.flush().unwrap();
        drop(pw);
        let mut s = String::new();
        pr.read_to_string(&mut s).unwrap();
        assert_eq!(s, "x");
    }

    #[test]
    fn more_descriptors_than_expected_are_refused_and_closed() {
        let (a, b) = UnixStream::pair().unwrap();
        let (x, _y) = UnixStream::pair().unwrap();
        let f = x.as_raw_fd();
        send(&a, b"z", &[f, f, f]).unwrap();
        let mut buf = [0u8; 1];
        let e = recv(&b, &mut buf, 2).unwrap_err();
        assert!(e.to_string().contains("at most 2"), "{e}");
        assert!(send(&a, b"z", &[f, f, f, f]).is_err());
        assert!(send(&a, b"", &[f]).is_err());
    }
}

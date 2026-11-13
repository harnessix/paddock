use super::*;

pub(super) enum Forward {
    Data,
    Idle,
    End,
}

/// Read what is already buffered.
pub(super) fn forward_out(
    master: RawFd,
    caller_out: RawFd,
    caller_ok: &mut bool,
    log: &impl Fn(&[u8]),
    buf: &mut [u8],
) -> Forward {
    match read_fd(master, buf) {
        Ok(0) => Forward::End,
        Ok(n) => {
            let data = &buf[..n];
            if *caller_ok && write_all_fd(caller_out, data).is_err() {
                *caller_ok = false;
            }
            log(data);
            Forward::Data
        }
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Forward::Idle,
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Forward::Idle,
        // The slave is gone. A pty master then returns EIO.
        Err(e) if e.raw_os_error() == Some(libc::EIO) => Forward::End,
        Err(_) => Forward::End,
    }
}

pub(super) fn drain_out(
    master: RawFd,
    caller_out: RawFd,
    caller_ok: &mut bool,
    log: &impl Fn(&[u8]),
    buf: &mut [u8],
) {
    while let Forward::Data = forward_out(master, caller_out, caller_ok, log, buf) {}
}

//! Who connects to the guest proxy: the process behind a loopback connection,
//! its executable, its code signature and the outermost app of its parent
//! chain.
//!
//! The pure parts have Linux tests: the parent chain walk, the cache, the
//! `netstat` line parser of the fallback. The macOS parts read
//! `net.inet.tcp.pcblist_n` (`so_e_pid` names the process of a loopback
//! connection; `so_last_pid` is 0 there), fall back to
//! `netstat -anv`, read the path with `proc_pidpath`, and ask the Security
//! framework for the signature.

#[cfg(target_os = "macos")]
use crate::macvm::proto::Identity;
use crate::mark::ProcTable;
use std::collections::{HashMap, VecDeque};

/// The cache keeps this many entries, by `(pid, path)`. The oldest goes
/// first.
pub const CACHE_MAX: usize = 1024;
/// The parent chain is followed this far at most.
const CHAIN_MAX: usize = 64;

/// The signature of a process: its bundle identifier, its team and its
/// executable path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signing {
    pub bundle: Option<String>,
    pub team: Option<String>,
    pub path: Option<String>,
}

/// The outermost app above `pid` in the process table: the bundle of the
/// highest ancestor that has one, so a helper (a Chromium network process,
/// a Go sidecar, a `curl` from an app's shell) is named after its app.
/// `None` when no ancestor has a bundle.
pub fn outer_app(
    pid: u32,
    table: &ProcTable,
    mut bundle_of: impl FnMut(u32) -> Option<String>,
) -> Option<String> {
    let mut outer = None;
    let mut cur = pid;
    for _ in 0..CHAIN_MAX {
        let Some(&parent) = table.get(&cur) else {
            break;
        };
        if parent == 0 || parent == 1 || parent == cur {
            break;
        }
        if let Some(b) = bundle_of(parent) {
            outer = Some(b);
        }
        cur = parent;
    }
    outer
}

/// Signatures by `(pid, path)`: a pid is reused after an exit, so the
/// path is part of the key.
#[derive(Default)]
pub struct Cache {
    map: HashMap<(u32, String), Signing>,
    order: VecDeque<(u32, String)>,
}

impl Cache {
    pub fn get(&self, pid: u32, path: &str) -> Option<&Signing> {
        self.map.get(&(pid, path.to_string()))
    }

    pub fn put(&mut self, pid: u32, path: &str, s: Signing) {
        let key = (pid, path.to_string());
        if self.map.insert(key.clone(), s).is_none() {
            self.order.push_back(key);
        }
        while self.map.len() > CACHE_MAX {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The pid column of `netstat -anv -p tcp` for the TCP socket whose local
/// port is `port`. The columns on macOS: proto, recv-q, send-q, local
/// address (`127.0.0.1.3128`), foreign address, state, rhiwat, shiwat,
/// pid, epid, and more. The `epid` names the process of a loopback
/// connection when `pid` is 0, as `so_e_pid` does.
pub fn parse_netstat(output: &str, port: u16) -> Option<u32> {
    let suffix = format!(".{port}");
    for line in output.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || !f[0].starts_with("tcp") || !f[3].ends_with(&suffix) {
            continue;
        }
        let pid: u32 = f[8].parse().ok()?;
        let epid: u32 = f[9].parse().unwrap_or(0);
        return Some(if pid != 0 { pid } else { epid }).filter(|p| *p != 0);
    }
    None
}

/// The search over the buffer, apart from the sysctl.
pub fn pcblist_find(buf: &[u8], port: u16) -> Option<u32> {
    let u32_at = |o: usize| {
        buf.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let i32_at = |o: usize| u32_at(o).map(|v| v as i32);
    let u16_be = |o: usize| buf.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let mut off = u32_at(0)? as usize;
    let mut lport: Option<u16> = None;
    while off + 8 <= buf.len() {
        let rlen = u32_at(off)? as usize;
        let kind = u32_at(off + 4)?;
        if rlen < 8 || off + rlen > buf.len() {
            break;
        }
        match kind {
            0x10 => lport = u16_be(off + 18),
            0x01 if lport.take() == Some(port) => {
                let e_pid = i32_at(off + 76)?;
                let last = i32_at(off + 72)?;
                let pid = if e_pid > 0 { e_pid } else { last };
                return (pid > 0).then_some(pid as u32);
            }
            0x01 => lport = None,
            _ => {}
        }
        off += rlen;
    }
    None
}

/// The identity of the process behind a loopback connection from
/// `local_port` of the client side.
#[cfg(target_os = "macos")]
pub fn lookup(local_port: u16) -> Identity {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let Some(pid) = peer_pid(local_port) else {
        return Identity::default();
    };
    let path = macos::path_of(pid);
    let key_path = path.clone().unwrap_or_default();
    let signing = {
        let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let cache = guard.get_or_insert_with(Cache::default);
        match cache.get(pid, &key_path) {
            Some(s) => s.clone(),
            None => {
                let s = macos::signing(pid, path.clone());
                cache.put(pid, &key_path, s.clone());
                s
            }
        }
    };
    let table = crate::mark::procs::proc_table();
    let parent_app = outer_app(pid, &table, |p| macos::signing(p, macos::path_of(p)).bundle);
    Identity {
        pid: Some(pid),
        app: signing.bundle,
        team: signing.team,
        path: signing.path,
        parent_app,
    }
}

/// The pid behind the local port of a TCP connection: `pcblist_n` first,
/// `netstat` second.
#[cfg(target_os = "macos")]
pub fn peer_pid(local_port: u16) -> Option<u32> {
    macos::pcblist_pid(local_port).or_else(|| {
        let out = std::process::Command::new("/usr/sbin/netstat")
            .args(["-anv", "-p", "tcp"])
            .output()
            .ok()?;
        parse_netstat(&String::from_utf8_lossy(&out.stdout), local_port)
    })
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Signing;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSNumber, NSString};
    use std::ffi::{c_void, CString};

    /// The sysctl buffer: records of `xi_len`, `xi_kind`, then the
    /// structure. Kind 0x10 is `xinpcb_n` with the foreign port at 16
    /// and the local port at 18 (network order). Kind 0x01 is
    /// `xsocket_n` with `so_uid` at 68, `so_last_pid` at 72 and
    /// `so_e_pid` at 76. The inpcb record comes right before its socket.
    pub fn pcblist_pid(port: u16) -> Option<u32> {
        let name = CString::new("net.inet.tcp.pcblist_n").ok()?;
        let mut len: libc::size_t = 0;
        // SAFETY: a size query with a null buffer.
        if unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        } != 0
        {
            return None;
        }
        let mut buf = vec![0u8; len + 4096];
        let mut len2: libc::size_t = buf.len();
        // SAFETY: the buffer is as long as len2 says.
        if unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buf.as_mut_ptr().cast(),
                &mut len2,
                std::ptr::null_mut(),
                0,
            )
        } != 0
        {
            return None;
        }
        buf.truncate(len2);
        super::pcblist_find(&buf, port)
    }

    /// The executable path of a process.
    pub fn path_of(pid: u32) -> Option<String> {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer is as long as the size says.
        let n =
            unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        String::from_utf8(buf).ok()
    }

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecGuestAttributePid: *const NSString;
        static kSecCodeInfoIdentifier: *const NSString;
        static kSecCodeInfoTeamIdentifier: *const NSString;
        fn SecCodeCopyGuestWithAttributes(
            host: *const c_void,
            attributes: *const NSDictionary<NSString, NSNumber>,
            flags: u32,
            guest: *mut *const c_void,
        ) -> i32;
        fn SecCodeCopySigningInformation(
            code: *const c_void,
            flags: u32,
            information: *mut *const NSDictionary<NSString, AnyObject>,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    /// `kSecCSSigningInformation`.
    const SIGNING_INFORMATION: u32 = 1 << 1;

    /// The bundle identifier and team of the running code of `pid`, from
    /// its signature. A process with no signature, or one that the
    /// framework will not name, gets `None` for both.
    pub fn signing(pid: u32, path: Option<String>) -> Signing {
        let mut out = Signing {
            bundle: None,
            team: None,
            path,
        };
        // SAFETY: the Security constants are CFStrings, toll-free NSString.
        let pid_key: &NSString = unsafe { &*kSecGuestAttributePid };
        let attrs =
            NSDictionary::from_retained_objects(&[pid_key], &[NSNumber::new_i32(pid as i32)]);
        let mut code: *const c_void = std::ptr::null();
        // SAFETY: a null host means the system, the dictionary lives for the call.
        let status = unsafe {
            SecCodeCopyGuestWithAttributes(
                std::ptr::null(),
                &*attrs as *const NSDictionary<NSString, NSNumber>,
                0,
                &mut code,
            )
        };
        if status != 0 || code.is_null() {
            return out;
        }
        let mut info: *const NSDictionary<NSString, AnyObject> = std::ptr::null();
        // SAFETY: code is a valid SecCodeRef that we own.
        let status = unsafe { SecCodeCopySigningInformation(code, SIGNING_INFORMATION, &mut info) };
        // SAFETY: the Copy rule: we own `code`.
        unsafe { CFRelease(code) };
        if status != 0 || info.is_null() {
            return out;
        }
        // SAFETY: the Copy rule: we own `info`, and it is a CFDictionary,
        // toll-free NSDictionary.
        let info: Retained<NSDictionary<NSString, AnyObject>> =
            unsafe { Retained::from_raw(info as *mut _) }.expect("a dictionary");
        let text = |key: *const NSString| -> Option<String> {
            // SAFETY: the Security constants are CFStrings.
            let key: &NSString = unsafe { &*key };
            let value = info.objectForKey(key)?;
            let s: Retained<NSString> = value.downcast::<NSString>().ok()?;
            let s = s.to_string();
            (!s.is_empty()).then_some(s)
        };
        // SAFETY: the constants of Security.framework.
        out.bundle = text(unsafe { kSecCodeInfoIdentifier });
        out.team = text(unsafe { kSecCodeInfoTeamIdentifier });
        out
    }
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;

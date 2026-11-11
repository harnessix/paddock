//! The horse tree of the device file system in the state that a run gives
//! it, through the FUSE protocol. The test speaks the kernel side over a
//! socket pair, so it needs no `/dev/fuse` and no mount.
use super::*;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

const LOOKUP: u32 = 1;
pub(super) const OPEN: u32 = 14;
const READ: u32 = 15;
const INIT: u32 = 26;

/// The kernel end of a FUSE session.
pub(super) struct Kernel {
    fd: OwnedFd,
    unique: u64,
}

impl Kernel {
    pub(super) fn send(&mut self, opcode: u32, nodeid: u64, body: &[u8]) -> (i32, Vec<u8>) {
        self.unique += 1;
        let mut m = Vec::new();
        m.extend(((40 + body.len()) as u32).to_ne_bytes());
        m.extend(opcode.to_ne_bytes());
        m.extend(self.unique.to_ne_bytes());
        m.extend(nodeid.to_ne_bytes());
        m.extend(unsafe { libc::geteuid() }.to_ne_bytes());
        m.extend(unsafe { libc::getegid() }.to_ne_bytes());
        m.extend(std::process::id().to_ne_bytes());
        m.extend(0u32.to_ne_bytes());
        m.extend(body);
        let n = unsafe { libc::write(self.fd.as_raw_fd(), m.as_ptr().cast(), m.len()) };
        assert_eq!(n, m.len() as isize);
        self.reply()
    }

    pub(super) fn reply(&mut self) -> (i32, Vec<u8>) {
        let mut buf = vec![0u8; 1 << 20];
        let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        assert!(n >= 16, "no reply");
        let error = i32::from_ne_bytes(buf[4..8].try_into().unwrap());
        let unique = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
        assert_eq!(unique, self.unique);
        (error, buf[16..n as usize].to_vec())
    }

    /// The node id of `name` in the directory `parent`, or the error.
    pub(super) fn lookup(&mut self, parent: u64, name: &str) -> Result<u64, i32> {
        let mut body = name.as_bytes().to_vec();
        body.push(0);
        match self.send(LOOKUP, parent, &body) {
            (0, out) => Ok(u64::from_ne_bytes(out[..8].try_into().unwrap())),
            (e, _) => Err(-e),
        }
    }

    /// The node id of a path below the root, one lookup per part.
    pub(super) fn walk(&mut self, path: &str) -> Result<u64, String> {
        let mut node = 1;
        for part in path.split('/') {
            node = self
                .lookup(node, part)
                .map_err(|e| format!("lookup of {part} in {path}: errno {e}"))?;
        }
        Ok(node)
    }

    /// The bytes of a file: an open and one read.
    pub(super) fn read(&mut self, path: &str) -> Result<Vec<u8>, String> {
        let node = self.walk(path)?;
        let (e, out) = self.send(OPEN, node, &[0u8; 8]);
        if e != 0 {
            return Err(format!("open of {path}: errno {}", -e));
        }
        let fh = u64::from_ne_bytes(out[..8].try_into().unwrap());
        let mut body = Vec::new();
        body.extend(fh.to_ne_bytes());
        body.extend(0u64.to_ne_bytes());
        body.extend((1u32 << 16).to_ne_bytes());
        body.extend([0u8; 20]);
        match self.send(READ, node, &body) {
            (0, out) => Ok(out),
            (e, _) => Err(format!("read of {path}: errno {}", -e)),
        }
    }
}

/// Serve `fs` on a socket pair and do the INIT handshake.
pub(super) fn serve(fs: DeviceFs) -> (Kernel, fuser::BackgroundSession) {
    let mut fds = [0; 2];
    assert_eq!(
        unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0, fds.as_mut_ptr()) },
        0
    );
    let (kernel, server) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let mut k = Kernel {
        fd: kernel,
        unique: 0,
    };
    // INIT: ABI 7.31, then the rest of fuse_init_in.
    let mut init = Vec::new();
    for v in [7u32, 31, 1 << 17, 0, 0] {
        init.extend(v.to_ne_bytes());
    }
    init.extend([0u8; 44]);
    k.unique += 1;
    let mut m = Vec::new();
    m.extend(((40 + init.len()) as u32).to_ne_bytes());
    m.extend(INIT.to_ne_bytes());
    m.extend(k.unique.to_ne_bytes());
    m.extend([0u8; 24]);
    m.extend(&init);
    assert_eq!(
        unsafe { libc::write(k.fd.as_raw_fd(), m.as_ptr().cast(), m.len()) },
        m.len() as isize
    );
    let session = fuser::Session::from_fd(
        fs,
        server,
        fuser::SessionACL::Owner,
        fuser::Config::default(),
    )
    .unwrap();
    assert_eq!(k.reply().0, 0, "INIT");
    (k, session.spawn().unwrap())
}

#[test]
fn a_fresh_run_session_exposes_an_encrypted_binding_in_the_device() {
    use crate::horse::{self as hs, run};
    let tmp = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        tmp.path().into(),
        "demo".into(),
        unsafe { libc::getuid() },
        unsafe { libc::getgid() },
        Default::default(),
    );
    let store = hs::Store::open_host(&env).unwrap();
    let rh = run::begin(&store, hs::dsh::HARNESS, hs::dsh::VERSION, None, None).unwrap();
    run::record_event(&store, &rh, "run.begin", "test", serde_json::json!({})).unwrap();
    store
        .lock()
        .unwrap()
        .bind_native(&rh.session, "session-demo")
        .unwrap();
    let target = store.new_default_horse().unwrap();
    let target_session = store
        .new_session(
            &target.id,
            hs::Binding {
                harness: "test".into(),
                native_id: None,
                version: "1".into(),
                container: "test".into(),
            },
        )
        .unwrap();
    let canary = b"trainer-fuse-canary-18942";
    let origin = hs::store::Origin {
        harness: "test",
        container: "test",
        version: "1",
    };
    let hash = store
        .for_horse(&target.id)
        .unwrap()
        .put_native(&origin, None, "seq:0", canary)
        .unwrap();
    store
        .update_session(&target_session.id, |s| s.head = Some(hash))
        .unwrap();
    hs::device::peek::grant::issue_trainer(
        &store,
        "test",
        "g-1",
        &target.id,
        &target_session.id,
        hs::now_ms() + 60_000,
        1024 * 1024,
    )
    .unwrap();
    let setup = crate::horse::device::Access {
        store: store.clone(),
        horse: rh.horse.clone(),
        session: rh.session.clone(),
    };
    let ctx = DeviceCtx {
        env: env.clone(),
        sandbox: "test".into(),
        harness: "dsh".into(),
        project: tmp.path().into(),
        started_by: vec![],
        approver: Arc::new(crate::broker::HostApprover),
        instances: crate::devices::sandbox::Instances::new(env.clone()),
    };
    let fs = DeviceFs::new("test", vec![]).with_horse(Some(setup), ctx);
    let (mut k, _session) = serve(fs);
    let binding = k
        .read(&format!("horse/self/sessions/{}/binding", rh.session))
        .unwrap();
    assert!(!binding
        .windows(b"session-demo".len())
        .any(|w| w == b"session-demo"));
    let target_file = format!("horse/peek/g-1/000000-native-{}", canary.len());
    let ciphertext = k.read(&target_file).unwrap();
    assert!(!ciphertext.windows(canary.len()).any(|w| w == canary));
    hs::device::peek::revoke(&store, "test").unwrap();
    assert!(k.walk(&target_file).is_err());
    // The walk is real: a session that the run does not own is not there.
    assert_eq!(
        k.walk("horse/self/sessions/s-000000000000/binding"),
        Err(
            "lookup of s-000000000000 in horse/self/sessions/s-000000000000/binding: errno 2"
                .into()
        )
    );
}

#[test]
fn a_horse_run_without_the_device_mount_says_why() {
    use crate::horse::{self as hs, run};
    let tmp = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        tmp.path().join("home"),
        "demo".into(),
        unsafe { libc::getuid() },
        unsafe { libc::getgid() },
        Default::default(),
    );
    let cfg = env.paddock_config();
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), "[devices]\nmount = false\n").unwrap();
    let store = hs::Store::open(&tmp.path().join("store")).unwrap();
    let rh = run::begin(&store, hs::dsh::HARNESS, hs::dsh::VERSION, None, None).unwrap();
    let access = crate::horse::device::Access {
        store,
        horse: rh.horse,
        session: rh.session,
    };
    let spec = || {
        crate::spec::SandboxSpec::new(
            "sb-1".into(),
            "dsh",
            std::path::Path::new("/home/demo/app"),
            env.home.join("sb"),
            &env.home,
            1000,
            1000,
            "demo",
        )
    };
    let secrets = || crate::credentials::secret::SecretFiles::new(None, None);
    // Without horse mode an off mount stays quiet.
    let mut plain = spec();
    assert!(start(
        &env,
        &mut plain,
        "bwrap",
        false,
        &GrantCtx::none(),
        &[],
        None,
        secrets(),
        None
    )
    .is_none());
    assert!(plain.notices.is_empty(), "{:?}", plain.notices);
    let mut horse = spec();
    let devices = start(
        &env,
        &mut horse,
        "bwrap",
        false,
        &GrantCtx::none(),
        &[],
        None,
        secrets(),
        Some(access),
    );
    assert!(devices.is_none());
    assert_eq!(
        horse.notices,
        [format!("{HORSE_OFF}: [devices] mount is false")]
    );
}

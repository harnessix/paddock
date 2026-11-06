use super::*;
use std::path::Path;
use std::time::{Duration, Instant};

#[test]
fn config_is_off_by_default_and_names_known_devices() {
    let c = DevicesCfg::parse("").unwrap();
    assert!(c.mount);
    assert!(c.enabled.is_empty());
    assert!(enabled_names(&c, false, &GrantCtx::none()).is_empty());
    assert_eq!(enabled_names(&c, true, &GrantCtx::none()), ["echo"]);
    let c = DevicesCfg::parse("[devices]\necho = true\n").unwrap();
    assert_eq!(c.enabled, ["echo"]);
    let c = DevicesCfg::parse("[devices]\nmount = false\necho = false\n").unwrap();
    assert!(!c.mount && c.enabled.is_empty());
    assert!(DevicesCfg::parse("[devices]\nsearch = true\n").is_err());
    assert!(DevicesCfg::parse("[devices]\necho = \"yes\"\n").is_err());
}

#[test]
fn the_project_file_cannot_turn_devices_on() {
    let r: Result<crate::manifest::Manifest, _> = toml::from_str("[devices]\necho = true\n");
    assert!(r.is_err());
}

#[test]
fn catalog_lists_every_device_with_its_state() {
    let off: serde_json::Value = serde_json::from_slice(&catalog_on(false, &[], &[])).unwrap();
    let names: Vec<&str> = off["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["echo", "ask", "wait"]);
    assert!(off["protocol"]
        .as_str()
        .unwrap()
        .contains("ETIME (still waiting)"));
    assert_eq!(off["devices"][0]["name"], "echo");
    assert_eq!(off["devices"][0]["state"], "off");
    assert!(off["devices"][0]["how_to_enable"]
        .as_str()
        .unwrap()
        .contains("config.toml"));
    let on: serde_json::Value =
        serde_json::from_slice(&catalog_on(false, &[Arc::new(Echo)], &[])).unwrap();
    assert_eq!(on["devices"][0]["state"], "on");
    assert_eq!(on["devices"][0]["path"], "/dev/paddock/echo");
    assert_eq!(on["devices"][0]["limits"]["max_request"], 65536);
    assert!(on["protocol"]
        .as_str()
        .unwrap()
        .contains("One open is one request"));
}

#[test]
fn catalog_lists_secret_files_by_name_only() {
    let v: serde_json::Value =
        serde_json::from_slice(&catalog_on(false, &[], &["GH_TOKEN".to_string()])).unwrap();
    assert_eq!(v["secrets"][0]["name"], "GH_TOKEN");
    assert_eq!(v["secrets"][0]["path"], "/dev/paddock/secret/GH_TOKEN");
    assert!(v["secrets_how"].as_str().unwrap().contains("cat "));
    let none: serde_json::Value = serde_json::from_slice(&catalog(&[])).unwrap();
    assert!(none.get("secrets").is_none());
}

#[test]
fn the_catalog_on_macos_names_the_command_and_the_fifo() {
    let v: serde_json::Value = serde_json::from_slice(&catalog_on(
        true,
        &[Arc::new(Echo)],
        &["GH_TOKEN".to_string()],
    ))
    .unwrap();
    assert_eq!(v["devices"][0]["invoke"], "paddock-host device echo");
    assert!(v["devices"][0].get("path").is_none());
    assert!(v["protocol"]
        .as_str()
        .unwrap()
        .contains("paddock-host device NAME"));
    assert_eq!(v["secrets"][0]["path"], "$PADDOCK_DEVICES/secret/GH_TOKEN");
    let how = v["secrets_how"].as_str().unwrap();
    assert!(
        how.contains("cat \"$PADDOCK_DEVICES/secret/GH_TOKEN\""),
        "{how}"
    );
    assert!(!how.contains("/dev/paddock"), "{how}");
}

/// A host for `start`: the user configuration turns on `devices`.
fn host(dir: &Path, devices: &str) -> HostEnv {
    let vars = [
        ("XDG_RUNTIME_DIR", dir.join("rt")),
        ("XDG_STATE_HOME", dir.join("st")),
        ("XDG_CONFIG_HOME", dir.join("cf")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.display().to_string()))
    .collect();
    let env = HostEnv::from_parts(dir.join("home"), "demo".into(), 1000, 1000, vars);
    crate::privfs::write(
        &env.paddock_config().join("config.toml"),
        format!("[devices]\n{devices}\n"),
    )
    .unwrap();
    env
}

fn sandbox(dir: &Path) -> crate::spec::SandboxSpec {
    crate::spec::SandboxSpec::new(
        "sb1".into(),
        "fake",
        &dir.join("project"),
        dir.join("home-src"),
        &dir.join("home"),
        1000,
        1000,
        "demo",
    )
}

#[test]
fn seatbelt_gets_a_socket_and_fifos_without_fuse() {
    use crate::credentials::secret::{Secret, SecretFiles};
    let dir = tempfile::tempdir().unwrap();
    let env = host(dir.path(), "echo = true");
    let mut spec = sandbox(dir.path());
    let secrets = SecretFiles::new(None, None);
    let d = start(
        &env,
        &mut spec,
        "seatbelt",
        false,
        &GrantCtx::none(),
        &[],
        None,
        secrets.clone(),
    )
    .expect("the devices of a Seatbelt run");
    assert!(spec.notices.is_empty(), "{:?}", spec.notices);
    let sock = spec.device_socket.clone().unwrap();
    assert_eq!(
        sock,
        crate::sockpath::named(&env, "sb1", SOCKET_NAME).unwrap()
    );
    assert_eq!(spec.devices, Some(socket_dir(&env, "sb1")));
    use socket::{request_at, Request};
    assert_eq!(
        request_at(&sock, &Request::call("echo", 2), b"hi").unwrap(),
        b"hi"
    );
    secrets.insert("T", Secret::from("tok"));
    let fifo = socket_dir(&env, "sb1").join(SECRET_DIR).join("T");
    let start = Instant::now();
    while !fifo.exists() {
        assert!(start.elapsed() < Duration::from_secs(2), "no FIFO");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read(&fifo).unwrap(), b"tok");
    drop(d);
    assert!(!sock.exists());
    assert!(!socket_dir(&env, "sb1").exists());
}

#[test]
fn the_vm_gets_a_notice_not_a_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let env = host(dir.path(), "echo = true");
    let mut spec = sandbox(dir.path());
    let secrets = crate::credentials::secret::SecretFiles::new(None, None);
    assert!(start(
        &env,
        &mut spec,
        "vm",
        false,
        &GrantCtx::none(),
        &[],
        None,
        secrets
    )
    .is_none());
    assert!(spec.devices.is_none() && spec.device_socket.is_none());
    assert_eq!(spec.notices.len(), 1);
    assert!(spec.notices[0].contains("do not reach the VM yet"));
    assert!(backend_support(&env, "seatbelt").is_ok());
}

#[test]
fn doctor_on_macos_names_the_socket_and_not_fuse() {
    let dir = tempfile::tempdir().unwrap();
    let env = host(dir.path(), "wait = true");
    let lines = doctor_lines_on(&env, true).join("\n");
    assert!(
        lines.contains("devices on seatbelt: over a socket, `paddock-host device`"),
        "{lines}"
    );
    assert!(
        lines.contains("device sandboxes on seatbelt: run = \"sandbox\" under sandbox-exec; no mem or cpus limit, and `paddock freeze` does not reach them"),
        "{lines}"
    );
    assert!(lines.contains("devices on vm:"), "{lines}");
    assert!(!lines.contains("FUSE"), "{lines}");
    assert!(lines.contains("devices: wait"), "{lines}");
}

/// Test devices that stream, for the mount, the socket and the client.
pub(crate) mod streaming {
    use crate::devices::{Call, Collect, DevError, Device, Info, Limits, Sink};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn info(response: &str, max_response: usize) -> Info {
        Info {
            shape: "builtin",
            stream: true,
            request: "any".into(),
            response: response.into(),
            limits: Limits {
                max_request: 1024,
                max_response,
                deadline_seconds: 30,
            },
        }
    }

    /// What `Ab` does after "a".
    #[derive(Clone, Copy)]
    pub enum After {
        /// "b", then the end.
        B,
        /// The device fails: bytes, then EIO.
        Fail,
        /// The device says still waiting: bytes, then ETIME.
        Wait,
    }

    /// Writes "a", waits 200 ms, then `then`. Records when it wrote "a".
    pub struct Ab {
        pub name: &'static str,
        pub then: After,
        pub wrote_a: Arc<Mutex<Option<Instant>>>,
    }

    impl Ab {
        pub fn new(name: &'static str, then: After) -> Self {
            Self {
                name,
                then,
                wrote_a: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl Device for Ab {
        fn name(&self) -> &str {
            self.name
        }
        fn info(&self) -> Info {
            info("a, then b", 16)
        }
        fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
            let mut c = Collect::default();
            self.call_stream(call, request, &mut c)?;
            Ok(c.0)
        }
        fn call_stream(
            &self,
            _call: &Call,
            _request: &[u8],
            sink: &mut dyn Sink,
        ) -> Result<(), DevError> {
            *self.wrote_a.lock().unwrap() = Some(Instant::now());
            sink.push(b"a");
            std::thread::sleep(Duration::from_millis(200));
            match self.then {
                After::B => {
                    sink.push(b"b");
                    Ok(())
                }
                After::Fail => Err(DevError::Failed),
                After::Wait => Err(DevError::Waiting),
            }
        }
    }

    /// The size of `Big`.
    #[cfg(target_os = "linux")]
    pub const BIG: usize = 64 << 20;
    /// The pieces of `Big`.
    #[cfg(target_os = "linux")]
    pub const PIECE: usize = 64 << 10;

    /// The byte of `Big` at `pos`.
    #[cfg(target_os = "linux")]
    pub fn pattern(pos: usize) -> u8 {
        pos as u8
    }

    /// Writes `BIG` bytes of `pattern` in pieces of `PIECE`. The mount
    /// test of Linux reads it.
    #[cfg(target_os = "linux")]
    pub struct Big;

    #[cfg(target_os = "linux")]
    impl Device for Big {
        fn name(&self) -> &str {
            "big"
        }
        fn info(&self) -> Info {
            info("64 MiB of a pattern", BIG)
        }
        fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
            let mut c = Collect::default();
            self.call_stream(call, request, &mut c)?;
            Ok(c.0)
        }
        fn call_stream(
            &self,
            call: &Call,
            _request: &[u8],
            sink: &mut dyn Sink,
        ) -> Result<(), DevError> {
            // Every piece is the same: PIECE is a multiple of 256.
            let piece: Vec<u8> = (0..PIECE).map(pattern).collect();
            for _ in 0..BIG / PIECE {
                if call.cancelled() {
                    return Err(DevError::Cancelled);
                }
                sink.push(&piece);
            }
            Ok(())
        }
    }

    /// Writes pieces of `piece` bytes until it is cancelled, up to `max`
    /// bytes of response. Records when it saw the cancel.
    pub struct Forever {
        pub name: &'static str,
        pub piece: usize,
        pub max: usize,
        pub saw_cancel: Arc<Mutex<Option<Instant>>>,
    }

    impl Forever {
        pub fn new(name: &'static str, piece: usize, max: usize) -> Self {
            Self {
                name,
                piece,
                max,
                saw_cancel: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl Device for Forever {
        fn name(&self) -> &str {
            self.name
        }
        fn info(&self) -> Info {
            info("bytes until cancelled", self.max)
        }
        fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
            let mut c = Collect::default();
            self.call_stream(call, request, &mut c)?;
            Ok(c.0)
        }
        fn call_stream(
            &self,
            call: &Call,
            _request: &[u8],
            sink: &mut dyn Sink,
        ) -> Result<(), DevError> {
            let piece = vec![b'z'; self.piece];
            while !call.cancelled() {
                sink.push(&piece);
            }
            *self.saw_cancel.lock().unwrap() = Some(Instant::now());
            Err(DevError::Cancelled)
        }
    }

    /// Writes `pieces` pieces of `piece` bytes with a short gap, then ends.
    pub struct Pieces {
        pub piece: usize,
        pub pieces: usize,
    }

    impl Device for Pieces {
        fn name(&self) -> &str {
            "pieces"
        }
        fn info(&self) -> Info {
            info("pieces", 1 << 20)
        }
        fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
            let mut c = Collect::default();
            self.call_stream(call, request, &mut c)?;
            Ok(c.0)
        }
        fn call_stream(
            &self,
            _call: &Call,
            _request: &[u8],
            sink: &mut dyn Sink,
        ) -> Result<(), DevError> {
            for i in 0..self.pieces {
                sink.push(&vec![b'0' + i as u8; self.piece]);
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(())
        }
    }
}

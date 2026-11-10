//! Host-side peer session. Transport settings never enter a request.
use super::snapshot::{Manifest, Snapshot};
use super::{
    protocol::{self, Request, Response},
    Peers,
};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Child;

#[derive(Debug)]
pub struct PeerError(pub String);
impl std::fmt::Display for PeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for PeerError {}

#[derive(Debug)]
pub struct Unreachable(pub String);
impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for Unreachable {}

pub struct Session {
    child: Child,
    input: std::process::ChildStdin,
    output: BufReader<std::process::ChildStdout>,
    gpu_supported: bool,
}

impl Session {
    pub fn connect(peers: &Peers, peer: &str, project: String) -> Result<Self> {
        let mut child = peers
            .connect(peer)
            .map_err(|e| Unreachable(e.to_string()))?;
        let mut input = child
            .stdin
            .take()
            .context("exclave transport has no input")?;
        let output = child
            .stdout
            .take()
            .context("exclave transport has no output")?;
        let pid = child.id() as i32;
        let (cancel, wait) = std::sync::mpsc::channel::<()>();
        let mut output = BufReader::new(output);
        let result = std::thread::scope(|scope| {
            let watchdog = scope.spawn(move || {
                if wait
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .is_err()
                {
                    // The transport is started in a separate process group.
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
            });
            let result = (|| -> Result<bool> {
                let sent = protocol::write(
                    &mut input,
                    &Request::Hello {
                        version: 1,
                        project,
                    },
                );
                match protocol::read(&mut output).map_err(|e| {
                    if e.chain().any(|cause| cause.is::<std::io::Error>()) {
                        anyhow::Error::new(Unreachable(
                            "transport connection failed while reading hello".into(),
                        ))
                    } else {
                        e
                    }
                })? {
                    Some(Response::Ready {
                        version: 1,
                        gpu_supported,
                    }) if sent.is_ok() => Ok(gpu_supported),
                    Some(Response::Error { message }) => Err(PeerError(message).into()),
                    None if sent.is_err() => Err(Unreachable(
                        "transport connection failed while sending hello".into(),
                    )
                    .into()),
                    None => Err(Unreachable("transport closed before peer hello".into()).into()),
                    _ => bail!("peer did not accept exclave protocol"),
                }
            })();
            let _ = cancel.send(());
            let _ = watchdog.join();
            result
        });
        let gpu_supported = match result {
            Ok(value) => value,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self {
            child,
            input,
            output,
            gpu_supported,
        })
    }

    /// Reserve peer capacity before dispatch. A busy peer has started no command.
    pub fn reserve(&mut self) -> Result<bool> {
        protocol::write(&mut self.input, &Request::Reserve)?;
        match protocol::read(&mut self.output)? {
            Some(Response::Accepted) => Ok(true),
            Some(Response::Busy) => Ok(false),
            Some(Response::Error { message }) => Err(PeerError(message).into()),
            _ => bail!("peer reservation failed"),
        }
    }

    /// Send a snapshot manifest and only the blobs the peer requests.
    pub fn upload(&mut self, snapshot: &Snapshot) -> Result<String> {
        let id = snapshot.manifest.hash()?;
        for entry in snapshot.manifest.files.values() {
            let bytes = snapshot
                .blobs
                .get(&entry.hash)
                .context("missing local blob")?;
            if bytes.len() as u64 != entry.size
                || blake3::hash(bytes).to_hex().as_str() != entry.hash
            {
                bail!("local blob integrity mismatch");
            }
        }
        protocol::write(
            &mut self.input,
            &Request::Manifest {
                manifest: snapshot.manifest.clone(),
            },
        )?;
        let hashes = match protocol::read(&mut self.output)? {
            Some(Response::Missing { hashes }) => hashes,
            Some(Response::Error { message }) => return Err(PeerError(message).into()),
            _ => bail!("peer did not request missing blobs"),
        };
        let allowed: BTreeSet<_> = snapshot
            .manifest
            .files
            .values()
            .map(|e| e.hash.as_str())
            .collect();
        let mut seen = BTreeSet::new();
        for hash in hashes {
            if !allowed.contains(hash.as_str()) || !seen.insert(hash.clone()) {
                bail!("peer requested unexpected blob");
            }
            let bytes = &snapshot.blobs[&hash];
            if bytes.is_empty() {
                protocol::write(
                    &mut self.input,
                    &Request::BlobChunk {
                        hash,
                        offset: 0,
                        data: String::new(),
                        final_chunk: true,
                    },
                )?;
            } else {
                for (i, chunk) in bytes.chunks(protocol::CHUNK_SIZE).enumerate() {
                    let offset = i * protocol::CHUNK_SIZE;
                    protocol::write(
                        &mut self.input,
                        &Request::BlobChunk {
                            hash: hash.clone(),
                            offset: offset as u64,
                            data: STANDARD.encode(chunk),
                            final_chunk: offset + chunk.len() == bytes.len(),
                        },
                    )?;
                }
            }
            match protocol::read(&mut self.output)? {
                Some(Response::Accepted) => {}
                Some(Response::Error { message }) => return Err(PeerError(message).into()),
                _ => bail!("peer did not accept blob"),
            }
        }
        Ok(id)
    }

    /// Run and install only declared, verified output files under project.
    #[allow(clippy::too_many_arguments)]
    pub fn run_with_outputs(
        &mut self,
        snapshot: String,
        network: String,
        gpu: bool,
        command: Vec<String>,
        outputs: Vec<String>,
        project: &Path,
        hidden: &[PathBuf],
        stdout: &mut impl Write,
        stderr: &mut impl Write,
    ) -> Result<i32> {
        if gpu && !self.gpu_supported {
            bail!("peer does not support GPU requests");
        }
        let mut declared = BTreeSet::new();
        for name in &outputs {
            if !super::snapshot::permitted(name, hidden)? || !declared.insert(name.clone()) {
                bail!("invalid declared output");
            }
        }
        protocol::write(
            &mut self.input,
            &Request::Run {
                snapshot,
                network,
                gpu,
                command,
                outputs,
            },
        )?;
        let mut manifest: Option<Manifest> = None;
        let mut blobs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut complete = BTreeSet::new();
        loop {
            match protocol::read(&mut self.output)? {
                Some(Response::Output { stream, data }) if manifest.is_none() => {
                    let bytes = STANDARD
                        .decode(data)
                        .map_err(|_| anyhow::anyhow!("invalid peer output"))?;
                    match stream.as_str() {
                        "stdout" => {
                            stdout.write_all(&bytes)?;
                            stdout.flush()?;
                        }
                        "stderr" => {
                            stderr.write_all(&bytes)?;
                            stderr.flush()?;
                        }
                        _ => bail!("invalid peer output stream"),
                    }
                }
                Some(Response::OutputManifest { manifest: m }) if manifest.is_none() => {
                    m.validate()?;
                    if !m.links.is_empty() {
                        bail!("symlink outputs are not allowed");
                    }
                    if !m.files.keys().all(|name| declared.contains(name)) {
                        bail!("unexpected output paths");
                    }
                    for name in m.files.keys() {
                        if !super::snapshot::permitted(name, hidden)? {
                            bail!("hidden output path");
                        }
                    }
                    manifest = Some(m);
                }
                Some(Response::OutputChunk {
                    hash,
                    offset,
                    data,
                    final_chunk,
                }) => {
                    let m = manifest.as_ref().context("output manifest required")?;
                    let entry = m
                        .files
                        .values()
                        .find(|e| e.hash == hash)
                        .context("unexpected output blob")?;
                    if complete.contains(&hash) || data.len() > protocol::CHUNK_SIZE.div_ceil(3) * 4
                    {
                        bail!("invalid output chunk");
                    }
                    let bytes = STANDARD
                        .decode(data)
                        .map_err(|_| anyhow::anyhow!("invalid output chunk"))?;
                    let buf = blobs.entry(hash.clone()).or_default();
                    if bytes.len() > protocol::CHUNK_SIZE
                        || offset != buf.len() as u64
                        || (!final_chunk && bytes.is_empty())
                        || (buf.len() as u64)
                            .checked_add(bytes.len() as u64)
                            .is_none_or(|n| n > entry.size)
                    {
                        bail!("invalid output chunk");
                    }
                    buf.extend_from_slice(&bytes);
                    if final_chunk {
                        if buf.len() as u64 != entry.size
                            || blake3::hash(buf).to_hex().as_str() != hash
                        {
                            bail!("output blob integrity mismatch");
                        }
                        complete.insert(hash);
                    }
                }
                Some(Response::Exit { code }) => {
                    let m = manifest.context("output manifest required")?;
                    if code == 0 && m.files.keys().cloned().collect::<BTreeSet<_>>() != declared {
                        bail!("declared output missing");
                    }
                    if m.files.values().any(|e| !complete.contains(&e.hash)) {
                        bail!("missing output blob");
                    }
                    let root = crate::safefs::Root::open(project)?;
                    // Verify every destination before writing any file. Directory walks never follow symlinks.
                    for name in m.files.keys() {
                        let p = Path::new(name);
                        let dir = root.dir(p.parent().unwrap_or(Path::new("")), true)?;
                        if !matches!(
                            dir.kind(p.file_name().unwrap())?,
                            None | Some(crate::safefs::Kind::File)
                        ) {
                            bail!("unsafe output destination");
                        }
                    }
                    for (name, entry) in &m.files {
                        root.write(
                            Path::new(name),
                            &blobs[&entry.hash],
                            if entry.executable { 0o700 } else { 0o600 },
                        )?;
                    }
                    return Ok(code);
                }
                Some(Response::Error { message }) => return Err(PeerError(message).into()),
                _ => bail!("unexpected peer response"),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        snapshot: String,
        network: String,
        gpu: bool,
        command: Vec<String>,
        outputs: Vec<String>,
        stdout: &mut impl Write,
        stderr: &mut impl Write,
    ) -> Result<i32> {
        if gpu && !self.gpu_supported {
            bail!("peer does not support GPU requests");
        }
        protocol::write(
            &mut self.input,
            &Request::Run {
                snapshot,
                network,
                gpu,
                command,
                outputs,
            },
        )?;
        loop {
            match protocol::read(&mut self.output)? {
                Some(Response::Output { stream, data }) => {
                    let bytes = STANDARD
                        .decode(data)
                        .map_err(|_| anyhow::anyhow!("invalid peer output"))?;
                    match stream.as_str() {
                        "stdout" => {
                            stdout.write_all(&bytes)?;
                            stdout.flush()?;
                        }
                        "stderr" => {
                            stderr.write_all(&bytes)?;
                            stderr.flush()?;
                        }
                        _ => bail!("invalid peer output stream"),
                    }
                }
                Some(Response::Exit { code }) => return Ok(code),
                Some(Response::Error { message }) => return Err(PeerError(message).into()),
                _ => bail!("peer connection ended unexpectedly"),
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // End the transport and any helpers that inherited the pipe.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::HostEnv;
    use std::collections::BTreeMap;

    #[test]
    fn handshake_preserves_peer_error_and_classifies_closed_transport() {
        let dir = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(
            dir.path().to_path_buf(),
            "test".into(),
            1,
            1,
            BTreeMap::new(),
        );
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        let script = dir.path().join("peer.sh");
        std::fs::write(&script, "read hello\nprintf '%s\\n' '{\"status\":\"done\",\"result\":{\"type\":\"error\",\"message\":\"No space left on device\"}}'\n").unwrap();
        std::fs::write(
            env.paddock_config().join("config.toml"),
            format!(
                "[exclave.peers.test]\ntransport = ['/bin/sh', '{}']\n",
                script.display()
            ),
        )
        .unwrap();
        let peers = Peers::load(&env).unwrap();
        let error = Session::connect(&peers, "test", "a".repeat(64))
            .err()
            .unwrap();
        assert!(error.is::<PeerError>());
        assert_eq!(error.to_string(), "No space left on device");
        std::fs::write(&script, "exit 0\n").unwrap();
        let error = Session::connect(&peers, "test", "a".repeat(64))
            .err()
            .unwrap();
        assert!(error.is::<Unreachable>());
    }

    #[test]
    fn streams_peer_output_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(
            dir.path().to_path_buf(),
            "test".into(),
            1,
            1,
            BTreeMap::new(),
        );
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        let script = dir.path().join("peer.sh");
        std::fs::write(&script, "read hello\nprintf '%s\\n' '{\"status\":\"done\",\"result\":{\"type\":\"ready\",\"version\":1}}'\nread command\nprintf '%s\\n' '{\"status\":\"done\",\"result\":{\"type\":\"output\",\"stream\":\"stdout\",\"data\":\"b2s=\"}}' '{\"status\":\"done\",\"result\":{\"type\":\"exit\",\"code\":7}}'\n").unwrap();
        let config = format!(
            "[exclave.peers.test]\ntransport = ['/bin/sh', '{}']\n",
            script.display()
        );
        std::fs::write(env.paddock_config().join("config.toml"), config).unwrap();
        let mut session =
            Session::connect(&Peers::load(&env).unwrap(), "test", "a".repeat(64)).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            session
                .run(
                    "b".repeat(64),
                    "none".into(),
                    false,
                    vec!["test".into()],
                    vec![],
                    &mut out,
                    &mut err
                )
                .unwrap(),
            7
        );
        assert_eq!(out, b"ok");
        assert!(err.is_empty());
    }
}

#[cfg(test)]
mod transfer_tests {
    use super::super::snapshot::Entry;
    use super::*;
    use crate::env::HostEnv;
    use std::os::unix::fs::symlink;

    fn peer(dir: &Path, script: &str) -> Session {
        let env = HostEnv::from_parts(dir.to_path_buf(), "test".into(), 1, 1, BTreeMap::new());
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        let file = dir.join("peer.sh");
        std::fs::write(&file, script).unwrap();
        std::fs::write(
            env.paddock_config().join("config.toml"),
            format!(
                "[exclave.peers.test]\ntransport = ['/bin/sh', '{}']\n",
                file.display()
            ),
        )
        .unwrap();
        Session::connect(&Peers::load(&env).unwrap(), "test", "a".repeat(64)).unwrap()
    }
    fn frame(response: Response) -> String {
        let mut wire = Vec::new();
        protocol::write(&mut wire, &response).unwrap();
        String::from_utf8(wire).unwrap()
    }
    fn entry(bytes: &[u8]) -> Entry {
        Entry {
            hash: blake3::hash(bytes).to_hex().to_string(),
            size: bytes.len() as u64,
            executable: false,
            modified_ns: 0,
        }
    }
    #[test]
    fn old_peer_cannot_silently_ignore_gpu_request() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = peer(dir.path(), "read hello\nprintf '%s\\n' '{\"status\":\"done\",\"result\":{\"type\":\"ready\",\"version\":1}}'\nread run\n");
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert!(session
            .run(
                "b".repeat(64),
                "none".into(),
                true,
                vec!["true".into()],
                vec![],
                &mut out,
                &mut err
            )
            .is_err());
    }

    fn run_peer(dir: &Path, responses: &[Response], outputs: Vec<String>) -> Result<i32> {
        let wire: String = responses
            .iter()
            .map(|r| {
                let mut b = Vec::new();
                protocol::write(&mut b, r).unwrap();
                String::from_utf8(b).unwrap()
            })
            .collect();
        let reply = dir.join("reply");
        std::fs::write(&reply, wire).unwrap();
        let script = format!("read hello\nprintf '%s\\n' '{{\"status\":\"done\",\"result\":{{\"type\":\"ready\",\"version\":1}}}}'\nread run\ncat '{}'\n", reply.display());
        peer(dir, &script).run_with_outputs(
            "b".repeat(64),
            "none".into(),
            false,
            vec!["test".into()],
            outputs,
            dir,
            &[],
            &mut Vec::new(),
            &mut Vec::new(),
        )
    }
    #[test]
    fn rejects_tampering_undeclared_and_symlink_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let e = entry(b"good");
        let manifest = Manifest {
            files: [("out".into(), e.clone())].into(),
            ..Manifest::default()
        };
        let responses = || {
            vec![
                Response::OutputManifest {
                    manifest: manifest.clone(),
                },
                Response::OutputChunk {
                    hash: e.hash.clone(),
                    offset: 0,
                    data: STANDARD.encode(b"evil"),
                    final_chunk: true,
                },
                Response::Exit { code: 0 },
            ]
        };
        assert!(run_peer(dir.path(), &responses(), vec!["out".into()]).is_err());
        assert!(!dir.path().join("out").exists());
        assert!(run_peer(dir.path(), &responses(), vec!["other".into()]).is_err());
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("target"), b"untouched").unwrap();
        symlink(outside.path().join("target"), dir.path().join("out")).unwrap();
        let valid = vec![
            Response::OutputManifest { manifest },
            Response::OutputChunk {
                hash: e.hash,
                offset: 0,
                data: STANDARD.encode(b"good"),
                final_chunk: true,
            },
            Response::Exit { code: 0 },
        ];
        assert!(run_peer(dir.path(), &valid, vec!["out".into()]).is_err());
        assert_eq!(
            std::fs::read(outside.path().join("target")).unwrap(),
            b"untouched"
        );
    }
    #[test]
    fn rejects_symlink_output_entries() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest {
            links: [("out".into(), "target".into())].into(),
            ..Manifest::default()
        };
        let responses = vec![
            Response::OutputManifest { manifest },
            Response::Exit { code: 0 },
        ];
        assert!(run_peer(dir.path(), &responses, vec!["out".into()]).is_err());
        assert!(!dir.path().join("out").exists());
    }

    #[test]
    fn installs_verified_output_and_rejects_unsafe_paths() {
        let dir = tempfile::tempdir().unwrap();
        let e = entry(b"content");
        let manifest = Manifest {
            files: [("nested/out".into(), e.clone())].into(),
            ..Manifest::default()
        };
        let valid = vec![
            Response::OutputManifest { manifest },
            Response::OutputChunk {
                hash: e.hash,
                offset: 0,
                data: STANDARD.encode(b"content"),
                final_chunk: true,
            },
            Response::Exit { code: 7 },
        ];
        assert_eq!(
            run_peer(dir.path(), &valid, vec!["nested/out".into()]).unwrap(),
            7
        );
        assert_eq!(
            std::fs::read(dir.path().join("nested/out")).unwrap(),
            b"content"
        );
        for name in ["../escape", ".git/config", ".env"] {
            assert!(run_peer(dir.path(), &[], vec![name.into()]).is_err());
        }
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), dir.path().join("link")).unwrap();
        let e = entry(b"content");
        let responses = vec![
            Response::OutputManifest {
                manifest: Manifest {
                    files: [("link/out".into(), e.clone())].into(),
                    ..Manifest::default()
                },
            },
            Response::OutputChunk {
                hash: e.hash,
                offset: 0,
                data: STANDARD.encode(b"content"),
                final_chunk: true,
            },
            Response::Exit { code: 0 },
        ];
        assert!(run_peer(dir.path(), &responses, vec!["link/out".into()]).is_err());
        assert!(!outside.path().join("out").exists());
    }
    #[test]
    fn rejects_unrequested_upload_hash() {
        let dir = tempfile::tempdir().unwrap();
        let ready = frame(Response::Ready {
            version: 1,
            gpu_supported: false,
        });
        let missing = frame(Response::Missing {
            hashes: vec!["a".repeat(64)],
        });
        let script = format!(
            "read hello\nprintf '%s' '{}'\nread manifest\nprintf '%s' '{}'\n",
            ready, missing
        );
        let mut session = peer(dir.path(), &script);
        let e = entry(b"data");
        let snapshot = Snapshot {
            manifest: Manifest {
                files: [("file".into(), e.clone())].into(),
                ..Manifest::default()
            },
            blobs: [(e.hash, b"data".to_vec())].into(),
        };
        assert!(session.upload(&snapshot).is_err());
    }
    #[test]
    fn uploads_only_requested_chunks_on_second_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let a = vec![42; protocol::MAX_FRAME as usize + 20];
        let b = b"changed".to_vec();
        let ea = entry(&a);
        let eb = entry(&b);
        let first = Snapshot {
            manifest: Manifest {
                files: [("a".into(), ea.clone())].into(),
                ..Manifest::default()
            },
            blobs: [(ea.hash.clone(), a)].into(),
        };
        let second = Snapshot {
            manifest: Manifest {
                files: [("a".into(), ea.clone()), ("b".into(), eb.clone())].into(),
                ..Manifest::default()
            },
            blobs: [
                (ea.hash.clone(), first.blobs[&ea.hash].clone()),
                (eb.hash.clone(), b),
            ]
            .into(),
        };
        let ready = frame(Response::Ready {
            version: 1,
            gpu_supported: false,
        });
        let missing1 = frame(Response::Missing {
            hashes: vec![ea.hash.clone()],
        });
        let missing2 = frame(Response::Missing {
            hashes: vec![eb.hash.clone()],
        });
        let accepted = frame(Response::Accepted);
        let log = dir.path().join("wire");
        let count = first.blobs[&ea.hash].len().div_ceil(protocol::CHUNK_SIZE);
        let script = format!("read hello\nprintf '%s' '{}'\nread manifest\nprintf '%s' '{}'\ni=0\nwhile [ $i -lt {} ]; do read chunk; printf '%s\\n' \"$chunk\" >> '{}'; i=$((i+1)); done\nprintf '%s' '{}'\nread manifest\nprintf '%s' '{}'\nread chunk\nprintf '%s\\n' \"$chunk\" >> '{}'\nprintf '%s' '{}'\n", ready, missing1, count, log.display(), accepted, missing2, log.display(), accepted);
        let mut session = peer(dir.path(), &script);
        session.upload(&first).unwrap();
        session.upload(&second).unwrap();
        let lines = std::fs::read_to_string(log).unwrap();
        let mut input = lines.as_bytes();
        let mut hashes = Vec::new();
        while let Some(Request::BlobChunk { hash, .. }) = protocol::read(&mut input).unwrap() {
            hashes.push(hash);
        }
        assert_eq!(hashes.len(), count + 1);
        assert!(hashes[..count].iter().all(|h| h == &ea.hash));
        assert_eq!(hashes[count], eb.hash);
    }
}

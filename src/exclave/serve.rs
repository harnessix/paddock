//! Peer stdio service. The peer never accepts local paths from the caller.

use super::{
    config::ServeConfig,
    protocol::{self, Request, Response},
    sandbox,
    snapshot::{Manifest, Store},
};
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

struct Remove(PathBuf);
impl Drop for Remove {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct RemoveTree(PathBuf);
impl Drop for RemoveTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn admission(root: &Path) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.join("run.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("peer is busy");
    }
    Ok(lock)
}

fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn directory(base: &Path, name: &str) -> Result<PathBuf> {
    if !digest(name) {
        bail!("invalid project or snapshot identifier");
    }
    let path = base.join(name);
    let metadata =
        std::fs::symlink_metadata(&path).context("project or snapshot does not exist")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("invalid project or snapshot directory");
    }
    Ok(path)
}

struct Upload {
    file: std::fs::File,
    path: PathBuf,
    hash: String,
    size: u64,
    written: u64,
    hasher: blake3::Hasher,
}
impl Drop for Upload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
impl Upload {
    fn new(store: &Store, hash: String, size: u64) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = store.directory().join(format!(
            ".upload-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        Ok(Self {
            file,
            path,
            hash,
            size,
            written: 0,
            hasher: blake3::Hasher::new(),
        })
    }
    fn chunk(&mut self, offset: u64, bytes: &[u8], final_chunk: bool, store: &Store) -> Result<()> {
        if bytes.len() > protocol::CHUNK_SIZE
            || offset != self.written
            || self
                .written
                .checked_add(bytes.len() as u64)
                .is_none_or(|n| n > self.size)
            || (!final_chunk && bytes.is_empty())
        {
            bail!("invalid blob chunk");
        }
        self.file.write_all(bytes)?;
        self.hasher.update(bytes);
        self.written += bytes.len() as u64;
        if final_chunk {
            if self.written != self.size || self.hasher.finalize().to_hex().as_str() != self.hash {
                bail!("blob integrity mismatch");
            }
            self.file.sync_all()?;
            // A complete, verified file is made visible in one atomic operation.
            std::fs::rename(&self.path, store.directory().join(&self.hash))?;
        }
        Ok(())
    }
}

pub fn serve(
    env: &HostEnv,
    input: &mut impl BufRead,
    output: &mut (impl Write + Send),
) -> Result<()> {
    let config = ServeConfig::load(env)?;
    let root = config.cache_root.clone();
    crate::privfs::dir(&root)?;
    let Some(Request::Hello {
        version: 1,
        project,
    }) = protocol::read(input)?
    else {
        bail!("expected exclave version 1 hello");
    };
    if !digest(&project) {
        bail!("invalid project identifier");
    }
    use std::os::unix::fs::MetadataExt;
    if !config.allow_shared_disk && std::fs::metadata(&root)?.dev() == std::fs::metadata("/")?.dev()
    {
        protocol::write(
            output,
            &Response::Error {
                message: super::cache::SHARED_DISK_ERROR.into(),
            },
        )?;
        return Ok(());
    }
    let project_id = project.clone();
    let projects = root.join("projects");
    crate::privfs::dir(&projects)?;
    let project_dir = projects.join(&project);
    crate::privfs::dir(&project_dir)?;
    let project_dir = directory(&projects, &project)?;
    // A fresh private socket exposes only the peer-owned registry allowlist.
    let socket = crate::sockpath::host_socket(
        &root,
        env.uid,
        "",
        &format!("proxy-{}.sock", std::process::id()),
    )?;
    let (list, bad) = crate::proxy::allow::Allowlist::parse(&config.registries);
    if !bad.is_empty() {
        bail!("invalid registry list");
    }
    let (http_policy, bad_http) =
        crate::proxy::allow::RegistryHttpPolicy::parse(&config.http_registries);
    if !bad_http.is_empty() {
        bail!("invalid HTTP registry list");
    }
    let (private_policy, bad_private) =
        crate::proxy::allow::RegistryHttpPolicy::parse(&config.private_registries);
    if !bad_private.is_empty() {
        bail!("invalid private registry list");
    }
    let proxy = crate::proxy::HostProxy {
        registry_private: Some(private_policy),
        registry_http: Some(http_policy),
        #[cfg(feature = "exclave-fake-registry-test")]
        allow_loopback: vec![38473],
        api: Some(crate::proxy::allow::ApiPolicy {
            list,
            refusals: None,
            allowlist: false,
        }),
        ..Default::default()
    };
    crate::proxy::serve(&socket, proxy)?;
    let _socket_cleanup = Remove(socket.clone());
    let resolv = root.join(format!("resolv-{}", std::process::id()));
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&resolv)?;
    let _resolv_cleanup = Remove(resolv.clone());
    file.write_all(b"nameserver 169.254.0.53\n")?;
    let executable = std::env::current_exe()?;
    let store = Store::open(&project_dir.join("blobs"))?;
    let mut manifest: Option<Manifest> = None;
    let mut upload: Option<Upload> = None;
    let mut reservation: Option<std::fs::File> = None;
    protocol::write(
        output,
        &Response::Ready {
            version: 1,
            gpu_supported: true,
        },
    )?;
    while let Some(request) = protocol::read(input)? {
        match request {
            Request::Reserve => {
                if reservation.is_some() {
                    protocol::write(output, &Response::Accepted)?;
                } else {
                    match admission(&root) {
                        Ok(lock) => {
                            reservation = Some(lock);
                            protocol::write(output, &Response::Accepted)?;
                        }
                        Err(_) => protocol::write(output, &Response::Busy)?,
                    }
                }
            }
            Request::Manifest { manifest: next } => {
                if upload.is_some() {
                    bail!("upload in progress");
                }
                next.validate()?;
                let missing = store.missing(&next)?;
                manifest = Some(next);
                protocol::write(
                    output,
                    &Response::Missing {
                        hashes: missing.into_iter().collect(),
                    },
                )?;
            }
            Request::BlobChunk {
                hash,
                offset,
                data,
                final_chunk,
            } => {
                let current = manifest.as_ref().context("manifest required")?;
                let size = current
                    .files
                    .values()
                    .find(|e| e.hash == hash)
                    .context("unrequested blob")?
                    .size;
                if data.len() > protocol::CHUNK_SIZE.div_ceil(3) * 4 {
                    bail!("oversized blob chunk");
                }
                let bytes = STANDARD
                    .decode(data)
                    .map_err(|_| anyhow::anyhow!("invalid blob chunk"))?;
                if upload.is_none() {
                    upload = Some(Upload::new(&store, hash.clone(), size)?);
                }
                let pending = upload.as_mut().unwrap();
                if pending.hash != hash {
                    bail!("interleaved blob upload");
                }
                pending.chunk(offset, &bytes, final_chunk, &store)?;
                if final_chunk {
                    upload = None;
                    protocol::write(output, &Response::Accepted)?;
                }
            }
            Request::Run {
                snapshot,
                network,
                gpu,
                command,
                outputs,
            } => {
                if upload.is_some() {
                    bail!("upload in progress");
                }
                let result = (|| -> Result<()> {
                    let _admission = reservation.take().context("peer reservation required")?;
                    if gpu && !config.gpu {
                        bail!("GPU is not enabled by the peer");
                    }
                    let current = manifest.as_ref().context("manifest required")?;
                    if current.hash()? != snapshot {
                        bail!("snapshot hash mismatch");
                    }
                    let runs = project_dir.join("runs");
                    crate::privfs::dir(&runs)?;
                    static NEXT: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(0);
                    let project = runs.join(format!(
                        "{}-{}",
                        std::process::id(),
                        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    ));
                    std::fs::create_dir(&project)?;
                    let _cleanup = RemoveTree(project.clone());
                    store.materialize(current, &project)?;
                    let cache = super::cache::ProjectCache::open(&root, project_id.as_bytes())?;
                    cache.enforce_cap(config.cache_bytes)?;
                    for path in &config.ro_paths {
                        super::config::validate_ro_path(env, path)?;
                    }
                    let mut process = match network.as_str() {
                        "none" => sandbox::offline(
                            &project,
                            cache.target_dir(),
                            cache.cargo_dir(),
                            cache.python_dir(),
                            &command,
                            &config.limits,
                            &config.env,
                            gpu,
                            &config.ro_paths,
                            config.rustup.then_some(env.home.as_path()),
                        )?,
                        "registries" => sandbox::registries(
                            &project,
                            cache.target_dir(),
                            cache.cargo_dir(),
                            cache.python_dir(),
                            &command,
                            &config.limits,
                            &socket,
                            &executable,
                            &config.env,
                            gpu,
                            &config.ro_paths,
                            config.rustup.then_some(env.home.as_path()),
                        )?,
                        "egress" if config.allow_egress => sandbox::egress(
                            env,
                            &project,
                            cache.target_dir(),
                            cache.cargo_dir(),
                            cache.python_dir(),
                            &command,
                            &config.limits,
                            &resolv,
                            &config.env,
                            gpu,
                            &config.ro_paths,
                            config.rustup,
                        )?,
                        _ => bail!("peer network mode is not allowed"),
                    };
                    let mut child = process
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()?;
                    let stdout = child.stdout.take().context("missing stdout")?;
                    let stderr = child.stderr.take().context("missing stderr")?;
                    let output = Mutex::new(&mut *output);
                    std::thread::scope(|scope| -> Result<()> {
                        let pump = |mut source: std::process::ChildStdout| -> Result<()> {
                            let mut buf = [0u8; 16384];
                            loop {
                                let n = source.read(&mut buf)?;
                                if n == 0 {
                                    return Ok(());
                                }
                                protocol::write(
                                    &mut **output
                                        .lock()
                                        .map_err(|_| anyhow::anyhow!("output lock failed"))?,
                                    &Response::Output {
                                        stream: "stdout".into(),
                                        data: STANDARD.encode(&buf[..n]),
                                    },
                                )?;
                            }
                        };
                        let out = scope.spawn(move || pump(stdout));
                        let mut err = stderr;
                        let output_ref = &output;
                        let e = scope.spawn(move || -> Result<()> {
                            let mut buf = [0u8; 16384];
                            loop {
                                let n = err.read(&mut buf)?;
                                if n == 0 {
                                    return Ok(());
                                }
                                protocol::write(
                                    &mut **output_ref
                                        .lock()
                                        .map_err(|_| anyhow::anyhow!("output lock failed"))?,
                                    &Response::Output {
                                        stream: "stderr".into(),
                                        data: STANDARD.encode(&buf[..n]),
                                    },
                                )?;
                            }
                        });
                        out.join()
                            .map_err(|_| anyhow::anyhow!("output thread failed"))??;
                        e.join()
                            .map_err(|_| anyhow::anyhow!("output thread failed"))??;
                        Ok(())
                    })?;
                    let status = child.wait()?;
                    cache.record_size()?;
                    if !status.success() && super::cache::volume_full(&root)? {
                        protocol::write(
                            &mut **output
                                .lock()
                                .map_err(|_| anyhow::anyhow!("output lock failed"))?,
                            &Response::Output {
                                stream: "stderr".into(),
                                data: STANDARD.encode(b"exclave cache volume full\n"),
                            },
                        )?;
                    }
                    use std::os::unix::process::ExitStatusExt;
                    let code = status
                        .code()
                        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
                    let mut present = Vec::new();
                    for name in &outputs {
                        if !super::snapshot::permitted(name, &[])? {
                            bail!("invalid output path");
                        }
                        match std::fs::symlink_metadata(project.join(name)) {
                            Ok(_) => present.push(name.clone()),
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound && code != 0 => {}
                            Err(e) => return Err(e.into()),
                        }
                    }
                    let returned = super::snapshot::collect_outputs(&project, &present, &[])?;
                    let mut sink = output
                        .lock()
                        .map_err(|_| anyhow::anyhow!("output lock failed"))?;
                    protocol::write(
                        &mut **sink,
                        &Response::OutputManifest {
                            manifest: returned.manifest,
                        },
                    )?;
                    for (hash, bytes) in returned.blobs {
                        if bytes.is_empty() {
                            protocol::write(
                                &mut **sink,
                                &Response::OutputChunk {
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
                                    &mut **sink,
                                    &Response::OutputChunk {
                                        hash: hash.clone(),
                                        offset: offset as u64,
                                        data: STANDARD.encode(chunk),
                                        final_chunk: offset + chunk.len() == bytes.len(),
                                    },
                                )?;
                            }
                        }
                    }
                    protocol::write(&mut **sink, &Response::Exit { code })?;
                    Ok(())
                })();
                if let Err(error) = result {
                    let full = error.chain().any(|e| {
                        e.downcast_ref::<std::io::Error>()
                            .is_some_and(|io| io.raw_os_error() == Some(libc::ENOSPC))
                    });
                    let message = if full {
                        "exclave cache volume full"
                    } else if error.to_string() == "GPU is not enabled by the peer" {
                        "GPU is not enabled by the peer"
                    } else if error.to_string() == "no NVIDIA devices available" {
                        "no NVIDIA devices available"
                    } else if error.to_string() == "exclave cache exceeds cache_bytes; active or in-use caches cannot be evicted" {
                        "exclave cache exceeds cache_bytes; active or in-use caches cannot be evicted"
                    } else {
                        "peer execution failed"
                    };
                    if message != "peer execution failed" {
                        protocol::write(
                            output,
                            &Response::Output {
                                stream: "stderr".into(),
                                data: STANDARD.encode(format!("{message}\n")),
                            },
                        )?;
                    }
                    protocol::write(
                        output,
                        &Response::Error {
                            message: message.into(),
                        },
                    )?;
                }
            }
            _ => protocol::write(
                output,
                &Response::Error {
                    message: "unsupported request".into(),
                },
            )?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn shared_root_refusal_is_reported_before_upload() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        if std::fs::metadata(temp.path()).unwrap().dev() != std::fs::metadata("/").unwrap().dev() {
            return;
        }
        let mut vars = BTreeMap::new();
        vars.insert(
            "XDG_DATA_HOME".into(),
            temp.path().join("data").display().to_string(),
        );
        vars.insert(
            "XDG_CONFIG_HOME".into(),
            temp.path().join("config").display().to_string(),
        );
        let env = crate::env::HostEnv::from_parts(
            temp.path().to_path_buf(),
            "test".into(),
            unsafe { libc::getuid() },
            unsafe { libc::getgid() },
            vars,
        );
        let mut input = Vec::new();
        protocol::write(
            &mut input,
            &Request::Hello {
                version: 1,
                project: "a".repeat(64),
            },
        )
        .unwrap();
        let mut output = Vec::new();
        serve(&env, &mut input.as_slice(), &mut output).unwrap();
        assert!(
            matches!(protocol::read::<Response>(&mut output.as_slice()).unwrap(), Some(Response::Error { message }) if message == super::super::cache::SHARED_DISK_ERROR)
        );
    }

    #[test]
    fn uploads_are_bounded_verified_and_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let data = vec![42; protocol::CHUNK_SIZE + 17];
        let hash = blake3::hash(&data).to_hex().to_string();
        let mut upload = Upload::new(&store, hash.clone(), data.len() as u64).unwrap();
        assert!(upload.chunk(1, &data[..10], false, &store).is_err());
        assert!(upload.chunk(0, &data, true, &store).is_err());
        upload
            .chunk(0, &data[..protocol::CHUNK_SIZE], false, &store)
            .unwrap();
        assert!(store.get(&hash).unwrap().is_none());
        upload
            .chunk(
                protocol::CHUNK_SIZE as u64,
                &data[protocol::CHUNK_SIZE..],
                true,
                &store,
            )
            .unwrap();
        assert_eq!(store.get(&hash).unwrap().unwrap(), data);
        let bad = "a".repeat(64);
        let mut upload = Upload::new(&store, bad.clone(), 3).unwrap();
        assert!(upload.chunk(0, b"bad", true, &store).is_err());
        drop(upload);
        assert!(store.get(&bad).unwrap().is_none());
    }

    #[test]
    fn local_framed_transfer_only_requests_changed_blob() {
        use super::super::snapshot::Entry;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".config/paddock")).unwrap();
        std::fs::write(
            dir.path().join(".config/paddock/config.toml"),
            "[exclave.serve]\nallow_shared_disk = true\n",
        )
        .unwrap();
        let env = HostEnv::from_parts(
            dir.path().to_path_buf(),
            "test".into(),
            1,
            1,
            BTreeMap::new(),
        );
        let a = b"one";
        let b = b"two";
        let c = b"changed";
        let entry = |bytes: &[u8]| Entry {
            hash: blake3::hash(bytes).to_hex().to_string(),
            size: bytes.len() as u64,
            executable: false,
            modified_ns: 0,
        };
        let first = Manifest {
            files: [("a".into(), entry(a)), ("b".into(), entry(b))].into(),
            ..Manifest::default()
        };
        let second = Manifest {
            files: [("a".into(), entry(a)), ("b".into(), entry(c))].into(),
            ..Manifest::default()
        };
        let mut wire = Vec::new();
        protocol::write(
            &mut wire,
            &Request::Hello {
                version: 1,
                project: "a".repeat(64),
            },
        )
        .unwrap();
        protocol::write(&mut wire, &Request::Manifest { manifest: first }).unwrap();
        for bytes in [a.as_slice(), b.as_slice()] {
            protocol::write(
                &mut wire,
                &Request::BlobChunk {
                    hash: entry(bytes).hash,
                    offset: 0,
                    data: STANDARD.encode(bytes),
                    final_chunk: true,
                },
            )
            .unwrap();
        }
        protocol::write(&mut wire, &Request::Manifest { manifest: second }).unwrap();
        let mut responses = Vec::new();
        serve(&env, &mut wire.as_slice(), &mut responses).unwrap();
        let mut input = responses.as_slice();
        assert!(matches!(
            protocol::read::<Response>(&mut input).unwrap(),
            Some(Response::Ready { .. })
        ));
        assert!(
            matches!(protocol::read::<Response>(&mut input).unwrap(), Some(Response::Missing { hashes }) if hashes.len() == 2)
        );
        assert!(matches!(
            protocol::read::<Response>(&mut input).unwrap(),
            Some(Response::Accepted)
        ));
        assert!(matches!(
            protocol::read::<Response>(&mut input).unwrap(),
            Some(Response::Accepted)
        ));
        assert!(
            matches!(protocol::read::<Response>(&mut input).unwrap(), Some(Response::Missing { hashes }) if hashes == vec![entry(c).hash])
        );
    }

    #[test]
    fn accepts_only_opaque_project_ids() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".config/paddock")).unwrap();
        std::fs::write(
            dir.path().join(".config/paddock/config.toml"),
            "[exclave.serve]\nallow_shared_disk = true\n",
        )
        .unwrap();
        let env = HostEnv::from_parts(
            dir.path().to_path_buf(),
            "test".into(),
            1,
            1,
            BTreeMap::new(),
        );
        let mut input = std::io::Cursor::new(
            format!(
                "{{\"v\":1,\"cap\":\"exclave\",\"args\":{{\"type\":\"hello\",\"version\":1,\"project\":\"{}\"}}}}\n",
                "a".repeat(64)
            )
            .into_bytes(),
        );
        let mut output = Vec::new();
        serve(&env, &mut input, &mut output).unwrap();
        assert!(String::from_utf8(output).unwrap().contains("ready"));
        let mut input = std::io::Cursor::new(
            b"{\"v\":1,\"cap\":\"exclave\",\"args\":{\"type\":\"hello\",\"version\":1,\"project\":\"../escape\"}}\n".to_vec(),
        );
        assert!(serve(&env, &mut input, &mut Vec::new()).is_err());
    }
}

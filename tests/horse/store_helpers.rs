use std::path::Path;

pub(super) fn open(path: &Path) -> anyhow::Result<paddock::horse::Store> {
    let root = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("invalid test store path"))?;
    let vars = [("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state")]
        .into_iter()
        .map(|(key, value)| (key.to_string(), root.join(value).display().to_string()))
        .collect();
    let env = paddock::env::HostEnv::from_parts(root.join("home"), "test".into(), 0, 0, vars);
    paddock::horse::Store::open_host(&env)
}

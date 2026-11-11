//! `paddock device add --example mac`: the worked example of the README,
//! written into the user configuration with its script, and the steps
//! for the Mac printed.

use super::{config_file, AddArgs};
use anyhow::{bail, Context as _, Result};
use paddock::devices::config::ParseEnv;
use paddock::devices::DevicesCfg;
use paddock::env::HostEnv;
use std::os::unix::fs::PermissionsExt;

/// `paddock device add --example mac`: the three tables of the worked
/// example and the script, then the Mac steps. `keyscan` reads the host
/// key of the Mac (`ssh-keyscan`); the line is shown, never written.
/// Nothing is written before every part passed its checks, and the
/// script is written before the configuration, so a failure leaves no
/// table that names a missing script.
pub(crate) fn add_example(
    env: &HostEnv,
    a: &AddArgs,
    keyscan: impl Fn(&str, u16) -> std::result::Result<String, String>,
) -> Result<String> {
    use paddock::devices::examples::{MacExample, BUILD_SH, CREDENTIAL, DEVICE};
    let host = a.host.clone().unwrap_or_else(|| "mac.local".into());
    let user = a.user.clone().unwrap_or_else(|| env.user.clone());
    if host.is_empty() || host.contains(|c: char| c.is_whitespace() || c == ':') {
        bail!("--host {host:?}: give the name or the address of the Mac, for example mac.local");
    }
    if user.is_empty()
        || user.contains(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.')
    {
        bail!("--user {user:?}: give the account name on the Mac, letters, digits, dash, underscore or dot");
    }
    let mut ex = MacExample::new(&host, &user, &env.home);
    if let Some(p) = a.port {
        ex.port = p;
        ex.connect_port = p;
    }
    let file = config_file(env);
    let cfg = DevicesCfg::load(env)?;
    if cfg.user.iter().any(|u| u.name == DEVICE) {
        bail!(
            "a [devices.{DEVICE}] table exists in {}. Remove it, or edit it there",
            file.display()
        );
    }
    let creds = paddock::credentials::load(env)?;
    if creds.contains_key(CREDENTIAL) {
        bail!(
            "a [credentials.{CREDENTIAL}] table exists in {}. Remove it, or edit it there",
            file.display()
        );
    }
    let script = ex.script_path();
    if script.exists() {
        bail!("{} exists. Remove it, or edit it there", script.display());
    }
    let penv = ParseEnv::host(&env.home);
    let text = ex.tables();
    // The round trip: what goes into the file parses to the device and
    // the credential of the example.
    let back = DevicesCfg::parse_with(&text, penv)
        .with_context(|| format!("the example tables do not parse:\n{text}"))?;
    if back.user.len() != 1 || back.user[0].name != DEVICE {
        bail!("the example tables do not parse to one device {DEVICE}:\n{text}");
    }
    let c = paddock::credentials::parse(&text)
        .with_context(|| format!("the example credential does not parse:\n{text}"))?;
    if !c.contains_key(CREDENTIAL) {
        bail!("the example tables have no [credentials.{CREDENTIAL}]:\n{text}");
    }
    if let Some(d) = script.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    std::fs::write(&script, BUILD_SH)
        .with_context(|| format!("cannot write {}", script.display()))?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("cannot make {} executable", script.display()))?;
    let mut existing = std::fs::read_to_string(&file).unwrap_or_default();
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str("\n# added by paddock device add --example mac\n");
    existing.push_str(&text);
    paddock::privfs::write(&file, existing)
        .with_context(|| format!("cannot write {}", file.display()))?;
    DevicesCfg::load(env).with_context(|| format!("{} after the add", file.display()))?;
    paddock::credentials::load(env).with_context(|| format!("{} after the add", file.display()))?;
    let scan = keyscan(&host, ex.port);
    let mut out = format!(
        "# added to {}\n{text}\n# wrote {} (mode 0755)\n\n",
        file.display(),
        script.display()
    );
    out.push_str(&ex.steps(scan.as_deref().map_err(String::as_str)));
    let kh = ex.known_hosts_path();
    if kh.exists() {
        out.push_str(&format!(
            "\n{} exists already: step 3 adds a line to it.\n",
            kh.display()
        ));
    }
    if ex.key_path().exists() {
        out.push_str(&format!(
            "\n{} exists already: step 1 is done if your agent holds it (`ssh-add -l`).\n",
            ex.key_path().display()
        ));
    }
    Ok(out)
}

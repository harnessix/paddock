use super::*;

pub(super) fn host_ipv4_addrs() -> Vec<String> {
    let out = Command::new("ip")
        .args(["-4", "-o", "addr", "show"])
        .output();
    let Ok(out) = out else { return vec![] };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().skip_while(|w| *w != "inet").nth(1))
        .filter_map(|cidr| cidr.split('/').next())
        .map(str::to_string)
        .collect()
}

pub(super) fn s_set_approvals(built: &mut build::Built, env: &HostEnv) {
    let dir = crate::broker::approvals_dir(env);
    built.spec.set_env(
        "PADDOCK_PROBE_APPROVALS",
        dir.display().to_string(),
        "doctor",
    );
}

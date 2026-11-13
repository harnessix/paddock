use super::*;

#[test]
fn sizes_for_people() {
    assert_eq!(size(512), "512 B");
    assert_eq!(size(154_786), "151 KB");
    assert_eq!(size(5 << 30), "5.0 GB");
    assert_eq!(size(3 << 20), "3.0 MB");
}

fn argv(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

fn tool_of(s: &str) -> Option<&'static str> {
    tunnel_in(&argv(s)).map(|t| t.tool)
}

#[test]
fn each_tool_matches_by_name() {
    for (cmd, tool) in [
        ("/usr/local/bin/ngrok http 3000", "ngrok"),
        ("cloudflared tunnel run", "cloudflared"),
        ("frpc -c frpc.toml", "frpc"),
        ("frps", "frps"),
        ("bore local 8000 --to example.org", "bore"),
        ("chisel client example.org R:80", "chisel"),
        ("lt --port 3000", "localtunnel"),
        ("localtunnel --port 3000", "localtunnel"),
        (
            "node /usr/lib/node_modules/.bin/lt --port 3000",
            "localtunnel",
        ),
        ("tailscale funnel 443", "tailscale funnel"),
    ] {
        assert_eq!(tool_of(cmd), Some(tool), "{cmd}");
    }
}

#[test]
fn ssh_matches_tunnel_flags_only() {
    let t = tunnel_in(&argv("ssh -R 8080:localhost:80 host")).unwrap();
    assert_eq!((t.tool, t.flags), ("ssh", vec!["-R".to_string()]));
    assert_eq!(
        tunnel_in(&argv("/usr/bin/ssh -L8080:a:80 host"))
            .unwrap()
            .flags,
        ["-L"]
    );
    assert_eq!(tunnel_in(&argv("ssh -D 1080 host")).unwrap().flags, ["-D"]);
    assert_eq!(
        tunnel_in(&argv("ssh -NR 9000:a:22 host")).unwrap().flags,
        ["-R"]
    );
    assert_eq!(
        tunnel_in(&argv("ssh -fNL 1:a:2 -D 3 host")).unwrap().flags,
        ["-L", "-D"]
    );
    // Options after the host count too.
    assert_eq!(tunnel_in(&argv("ssh host -R 1:a:2")).unwrap().flags, ["-R"]);
    assert!(tunnel_in(&argv("ssh -oRemoteForward=1:a:2 host")).is_some());
    assert_eq!(
        tunnel_in(&argv("ssh -o RemoteForward=1:a:2 host"))
            .unwrap()
            .flags,
        ["-R"]
    );
    assert_eq!(
        tunnel_in(&argv("ssh -o LocalForward 1:a:2 host"))
            .unwrap()
            .flags,
        ["-L"]
    );
    assert_eq!(
        tunnel_in(&argv("ssh -o DynamicForward=1080 host"))
            .unwrap()
            .flags,
        ["-D"]
    );
    assert_eq!(
        tunnel_in(&argv("ssh -W host:443 jump")).unwrap().flags,
        ["-W"]
    );
    assert_eq!(tunnel_in(&argv("ssh -w 0:0 host")).unwrap().flags, ["-w"]);
}

#[test]
fn normal_commands_do_not_match() {
    for cmd in [
        "ssh host",
        "ssh -p 22 -i key host",
        // The argument of -l, and the remote command, are not flags.
        "ssh -l R host",
        "ssh -o ProxyJump=x host",
        "ssh host ls -R /",
        "ssh -- host -R",
        "git push",
        "cargo build -L",
        "tailscale status",
        "sleep 10",
        "bash -c ls",
        "",
    ] {
        assert_eq!(tool_of(cmd), None, "{cmd}");
    }
}

#[test]
fn the_config_switches() {
    assert_eq!(
        MonitorCfg::parse(""),
        MonitorCfg {
            tunnels: true,
            notify: true,
            traffic: true,
            upload_per_hour: 5 << 30,
            factor: 10.0,
            credentials: true,
            egress: true,
        }
    );
    let off = MonitorCfg::parse(
        "[monitor]\ntunnels = false\nnotify = false\ntraffic = false\nupload_per_hour = \"300M\"\nfactor = 4\ncredentials = false\negress = false\n",
    );
    assert!(!off.tunnels && !off.notify && !off.traffic && !off.credentials && !off.egress);
    assert_eq!((off.upload_per_hour, off.factor), (300 << 20, 4.0));
    let num = MonitorCfg::parse("[monitor]\nupload_per_hour = 1000\nfactor = 2.5\n");
    assert_eq!((num.upload_per_hour, num.factor), (1000, 2.5));
    // Bad values fall back to the defaults.
    let bad = MonitorCfg::parse("[monitor]\nupload_per_hour = \"lots\"\nfactor = 0\n");
    assert_eq!((bad.upload_per_hour, bad.factor), (5 << 30, 10.0));
}

#[test]
fn sizes() {
    assert_eq!(parse_size("5G"), Some(5 << 30));
    assert_eq!(parse_size("300m"), Some(300 << 20));
    assert_eq!(parse_size("64K"), Some(64 << 10));
    assert_eq!(parse_size(" 1000 "), Some(1000));
    for bad in ["", "G", "5T", "-1G", "1.5G", "lots"] {
        assert_eq!(parse_size(bad), None, "{bad}");
    }
    assert_eq!(parse_size("99999999999G"), None);
}

const NET_DEV: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
lo: 9000000      10    0    0    0     0          0         0  9000000      10    0    0    0     0       0          0
  tap0:    1500       5    0    0    0     0          0         0      700       4    0    0    0     0       0          0
  eth1:     500       1    0    0    0     0          0         0      300       2    0    0    0     0       0          0
";

#[test]
fn net_dev_skips_lo() {
    assert_eq!(parse_net_dev(NET_DEV), (2000, 1000));
    assert_eq!(parse_net_dev(""), (0, 0));
    assert_eq!(parse_net_dev("garbage: 1 2\n"), (0, 0));
}

#[test]
fn a_counter_never_steps_back() {
    let mut c = Counter::default();
    assert_eq!(c.step((100, 50)), (100, 50));
    assert_eq!(c.step((150, 80)), (50, 30));
    assert_eq!(c.step((150, 80)), (0, 0));
    // A new namespace starts from zero: the reading is the step.
    assert_eq!(c.step((10, 5)), (10, 5));
    assert_eq!(c.step((20, 5)), (10, 0));
}

#[test]
fn the_hourly_window() {
    let h = 500_000u64;
    let mut t = Traffic::default();
    assert_eq!(t.add(h * 3600 + 10, 7, 100), None);
    assert_eq!(t.add(h * 3600 + 3599, 3, 50), None);
    assert_eq!((t.hour, t.hour_sent, t.sent, t.received), (h, 150, 150, 10));
    // The next hour starts at zero and hands back the hour that ended.
    assert_eq!(t.add((h + 1) * 3600, 0, 20), Some((h, 150)));
    assert_eq!((t.hour, t.hour_sent, t.sent), (h + 1, 20, 170));
    // An hour without sent bytes hands back nothing.
    assert_eq!(t.add((h + 2) * 3600, 5, 0), Some((h + 1, 20)));
    assert_eq!(t.add((h + 3) * 3600, 5, 0), None);
}

#[test]
fn the_baseline_median() {
    let mut b = Baseline::default();
    assert_eq!(b.median(), None);
    let now = 1000;
    for (h, sent) in [(990, 10), (991, 0), (992, 30), (993, 20)] {
        b.add(h, sent, now);
    }
    assert_eq!(b.busy(), 3);
    assert_eq!(b.median(), Some(20));
    // Two sandboxes in the same hour add up.
    b.add(993, 20, now);
    assert_eq!(b.hours, [(990, 10), (992, 30), (993, 40)]);
    b.add(994, 50, now);
    assert_eq!(b.median(), Some(35));
    // Older than 30 days goes.
    b.add(1700, 1, 991 + 30 * 24);
    assert_eq!(b.hours.first(), Some(&(992, 30)));
    // The file holds numbers only, in a temp folder.
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("monitor/h-1.json");
    b.save(&file).unwrap();
    assert_eq!(Baseline::load(&file), b);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text
        .chars()
        .all(|c| c.is_ascii_digit() || "{}[],:\"hours".contains(c)));
}

fn base_of(hours: &[u64]) -> Baseline {
    Baseline {
        hours: hours
            .iter()
            .enumerate()
            .map(|(i, b)| (i as u64, *b))
            .collect(),
    }
}

#[test]
fn both_alert_rules() {
    let cfg = MonitorCfg::parse("[monitor]\nupload_per_hour = \"1M\"\nfactor = 10\n");
    let none = Baseline::default();
    assert!(reasons(1 << 20, &cfg, &none).is_empty());
    assert_eq!(reasons((1 << 20) + 1, &cfg, &none), ["upload_per_hour"]);
    // Four hours are not a baseline yet.
    let four = base_of(&[100, 100, 100, 100]);
    assert!(reasons(5000, &cfg, &four).is_empty());
    let five = base_of(&[100, 100, 100, 100, 100]);
    assert!(reasons(1000, &cfg, &five).is_empty());
    assert_eq!(reasons(1001, &cfg, &five), ["factor"]);
    assert_eq!(reasons(2 << 20, &cfg, &five), ["upload_per_hour", "factor"]);
}

#[test]
fn an_alert_counts_once_per_sandbox_and_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("monitor.json");
    let mut seen = Seen::open(file.clone());
    assert_eq!(seen.fresh_alerts(vec!["factor"], 7, 100), ["factor"]);
    assert!(seen.fresh_alerts(vec!["factor"], 7, 100).is_empty());
    assert_eq!(
        seen.fresh_alerts(vec!["factor", "upload_per_hour"], 7, 100),
        ["upload_per_hour"]
    );
    let mut again = Seen::open(file);
    assert!(again
        .fresh_alerts(vec!["upload_per_hour", "factor"], 7, 100)
        .is_empty());
    let mut other = Seen::open(tmp.path().join("other.json"));
    assert_eq!(other.fresh_alerts(vec!["factor"], 7, 100), ["factor"]);
}

#[test]
fn traffic_goes_to_the_run_folder_and_the_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = MonitorCfg::parse("[monitor]\nupload_per_hour = \"1K\"\n");
    let mut live = Live {
        seen: Seen::open(tmp.path().join("run/monitor.json")),
        counter: Counter::default(),
        base_file: tmp.path().join("state/monitor/h-1.json"),
        done: false,
        creds: None,
        creds_done: false,
    };
    let (new, sent, _) = live.count((300, 200), &cfg);
    assert!(new.is_empty());
    assert_eq!(sent, 200);
    let (new, sent, _) = live.count((400, 2000), &cfg);
    assert_eq!((new, sent), (vec!["upload_per_hour"], 2000));
    let (new, _, _) = live.count((500, 3000), &cfg);
    assert!(new.is_empty());
    let rec = load_record(&tmp.path().join("run/monitor.json"));
    let t = rec.traffic.unwrap();
    assert_eq!((t.sent, t.received), (3000, 500));
    // The alert flag and the bytes of its hour, for the panel.
    assert!(rec.alerts.contains("upload_per_hour"));
    assert_eq!(rec.alert_sent, 3000);
    assert_eq!(rec.alert_hour, t.hour);
    live.finish();
    live.finish();
    let b = Baseline::load(&live.base_file);
    assert_eq!(b.hours.iter().map(|h| h.1).sum::<u64>(), 3000);
}

#[test]
fn a_tool_counts_once_per_sandbox() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("monitor.json");
    let ngrok = || tunnel_in(&argv("ngrok http 80")).unwrap();
    let ssh = || tunnel_in(&argv("ssh -R 1:a:2 h")).unwrap();
    let mut seen = Seen::open(file.clone());
    assert_eq!(seen.fresh(vec![ngrok(), ngrok()]).len(), 1);
    assert!(seen.fresh(vec![ngrok()]).is_empty());
    assert_eq!(seen.fresh(vec![ssh(), ngrok()]).len(), 1);
    // A new watch reads what the run folder holds.
    let mut again = Seen::open(file.clone());
    assert!(again.fresh(vec![ngrok(), ssh()]).is_empty());
    assert_eq!(
        load_seen(&file).into_iter().collect::<Vec<_>>(),
        ["ngrok", "ssh"]
    );
    // Another sandbox has a file of its own.
    let mut other = Seen::open(tmp.path().join("other.json"));
    assert_eq!(other.fresh(vec![ngrok()]).len(), 1);
}

fn put(home: &Path, rel: &str, body: &str) {
    let f = home.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, body).unwrap();
}

fn paths(files: &[CredFile]) -> Vec<&str> {
    files.iter().map(|f| f.path.as_str()).collect()
}

#[test]
fn credential_names() {
    for yes in [
        ".ssh/id_ed25519",
        ".ssh/authorized_keys",
        ".ssh/config",
        ".netrc",
        ".git-credentials",
        ".config/gh/hosts.yml",
        ".docker/config.json",
        ".aws/credentials",
        ".kube/config",
        "work/server.pem",
        ".config/x/GitHub_Token.txt",
        "notes/apikey",
    ] {
        assert!(is_credential(yes), "{yes}");
    }
    for no in [
        ".bashrc",
        ".config/x/config.json",
        ".gitconfig",
        "go/pkg/mod/golang.org/x/oauth2@v0.21.0/token.go",
        "token.go",
        "src/token.rs",
        "lib/secret.py",
    ] {
        assert!(!is_credential(no), "{no}");
    }
}

#[test]
fn new_credential_files_are_found_and_login_refreshes_counted() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    put(&home, ".bashrc", "x");
    put(&home, ".fake/auth.json", "login-1");
    put(&home, ".fake/keys/a", "k");
    put(&home, ".ssh/known_hosts", "h");
    let mut c = Creds::new(
        home.clone(),
        vec![".fake/auth.json".into(), ".fake/keys/".into()],
    );
    assert_eq!(c.check(), (vec![], 0));
    // The harness refreshes its token and adds a key to its login folder.
    put(&home, ".fake/auth.json", "login-2-longer");
    put(&home, ".fake/keys/b", "k");
    assert_eq!(c.check(), (vec![], 2));
    // A planted key, an authorized key and a file in a cache.
    put(&home, ".ssh/id_ed25519", "PRIVATE");
    put(&home, ".ssh/authorized_keys", "ssh-ed25519 AAA");
    put(&home, ".cache/x/token", "t");
    put(&home, "proj/node_modules/m/secret.js", "s");
    put(
        &home,
        "go/pkg/mod/golang.org/x/oauth2@v0.21.0/token.go",
        "package oauth2",
    );
    put(&home, ".cargo/git/checkouts/pkg/token.txt", "token");
    put(&home, ".m2/repository/pkg/secret.txt", "secret");
    put(&home, ".gradle/caches/secret.txt", "secret");
    let (found, n) = c.check();
    assert_eq!(n, 0);
    assert_eq!(paths(&found), [".ssh/authorized_keys", ".ssh/id_ed25519"]);
    assert_eq!(found[1].size, 7);
    // A change to a credential file that is not a login is news.
    put(&home, ".ssh/known_hosts", "h2");
    assert_eq!(paths(&c.check().0), [".ssh/known_hosts"]);
    assert_eq!(c.check(), (vec![], 0));
}

#[test]
fn a_symlink_is_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let outside = tmp.path().join("outside");
    put(&outside, ".ssh/id_rsa", "k");
    put(&outside, "token", "t");
    std::fs::create_dir_all(&home).unwrap();
    let mut c = Creds::new(home.clone(), vec![]);
    std::os::unix::fs::symlink(outside.join(".ssh"), home.join(".ssh")).unwrap();
    std::os::unix::fs::symlink(outside.join("token"), home.join("token")).unwrap();
    assert_eq!(paths(&c.check().0), ["token"]);
}

#[test]
fn a_folder_swapped_for_a_symlink_during_the_scan_is_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let outside = tmp.path().join("outside");
    put(&outside, "token", "t");
    put(&home, "x/a", "a");
    put(&home, "trigger", "x");
    // The folder `x` is listed before it is entered. The sandbox swaps it
    // for a symlink in between.
    let swap = || {
        std::fs::rename(home.join("x"), home.join("x.old")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("x")).unwrap();
    };
    let keep = |r: &str| {
        if r == "trigger" {
            swap();
        }
        is_credential(r)
    };
    let found = scan(&home, &keep);
    assert!(found.keys().all(|k| !k.starts_with("x/")), "{found:?}");
}

#[test]
fn a_credential_file_counts_once_per_sandbox() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("monitor.json");
    let key = |size| CredFile {
        path: ".ssh/id_ed25519".into(),
        size,
    };
    let mut seen = Seen::open(file.clone());
    assert_eq!(seen.fresh_creds(vec![key(7)], 0), [key(7)]);
    assert!(seen.fresh_creds(vec![key(9)], 2).is_empty());
    let mut again = Seen::open(file.clone());
    assert!(again.fresh_creds(vec![key(9)], 1).is_empty());
    let rec = load_record(&file);
    assert_eq!(rec.credential_files, [key(7)]);
    assert_eq!(rec.login_refreshes, 3);
}

#[test]
fn credential_file_names_in_notifications_are_cleaned() {
    let dirty = CredFile {
        path: "bad\nname\u{202e}.token\u{1b}[2J".into(),
        size: 10,
    };
    let body = cred_notification_body("s1", &[dirty]);
    assert!(!body.contains('\n'));
    assert!(!body.contains('\u{202e}'));
    assert!(!body.contains('\u{1b}'));
    assert!(body.contains("~/bad name.token[2J"));
}

#[test]
fn the_audit_line_has_path_and_size_but_no_content() {
    let tmp = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        tmp.path().join("host"),
        "u".into(),
        1,
        1,
        Default::default(),
    );
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let id = Identity {
        sandbox: "s-1".into(),
        harness: "fake".into(),
        project: tmp.path().join("p"),
        pasture: None,
        parent: None,
        chain: Vec::new(),
        tree: None,
    };
    let cfg = MonitorCfg {
        notify: false,
        ..MonitorCfg::parse("")
    };
    let w = Watch {
        live: Arc::new(Mutex::new(Live {
            seen: Seen::open(tmp.path().join("run/monitor.json")),
            counter: Counter::default(),
            base_file: tmp.path().join("base.json"),
            done: false,
            creds: None,
            creds_done: false,
        })),
        env: env.clone(),
        id,
        cfg,
    };
    w.credentials(home.clone(), vec![]);
    let secret = "SECRET-KEY-MATERIAL-1234";
    put(&home, ".ssh/id_ed25519", secret);
    w.finish();
    w.finish();
    let log = std::fs::read_to_string(crate::broker::audit_file(&env)).unwrap();
    assert_eq!(log.lines().count(), 1);
    let line: serde_json::Value = serde_json::from_str(log.trim()).unwrap();
    assert_eq!(line["capability"], CAPABILITY);
    assert_eq!(line["path"], ".ssh/id_ed25519");
    assert_eq!(line["size"], secret.len());
    assert!(!log.contains(secret));
}

#[test]
fn the_alert_keeps_the_bytes_of_its_hour() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("monitor.json");
    let mut seen = Seen::open(file.clone());
    assert_eq!(
        seen.fresh_alerts(vec!["upload_per_hour"], 10, 6 << 30),
        ["upload_per_hour"]
    );
    // Later in the same hour the bytes follow.
    assert!(seen
        .fresh_alerts(vec!["upload_per_hour"], 10, 7 << 30)
        .is_empty());
    // The next hour does not change them.
    seen.fresh_alerts(vec![], 11, 1 << 30);
    let text = std::fs::read_to_string(&file).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["alerts"], serde_json::json!(["upload_per_hour"]));
    assert_eq!(v["alert_sent"], 7u64 << 30);
    assert_eq!(v["alert_hour"], 10);
}

fn test_env(tmp: &Path) -> HostEnv {
    let mut vars = BTreeMap::new();
    vars.insert(
        "XDG_RUNTIME_DIR".to_string(),
        tmp.join("run").display().to_string(),
    );
    HostEnv::from_parts(tmp.join("home"), "demo".into(), 1000, 1000, vars)
}

#[test]
fn ack_clears_the_mark_until_a_new_finding() {
    let tmp = tempfile::tempdir().unwrap();
    let env = test_env(tmp.path());
    let project = Path::new("/work/app");
    let mut seen = Seen::open(seen_file(&env, "s1"));
    seen.fresh(vec![tunnel_in(&argv("ngrok http 80")).unwrap()]);
    seen.fresh_alerts(vec!["upload_per_hour"], 3, 6 << 30);
    seen.fresh_creds(
        vec![CredFile {
            path: ".ssh/id_ed25519".into(),
            size: 400,
        }],
        0,
    );
    let m = marks(&env, "s1", project);
    let texts: Vec<String> = m.iter().map(Finding::text).collect();
    assert_eq!(
        texts,
        [
            "tunnel: ngrok",
            "upload: 6.0 GB this hour",
            "new credential file: ~/.ssh/id_ed25519"
        ]
    );
    assert_eq!(ack(&env, "s1").unwrap(), 3);
    assert!(marks(&env, "s1", project).is_empty());
    // The findings stay, marked as acknowledged.
    assert!(findings(&env, "s1").iter().all(|f| f.acked));
    // A new finding marks the sandbox again, alone.
    seen.fresh(vec![tunnel_in(&argv("cloudflared tunnel run")).unwrap()]);
    let m = marks(&env, "s1", project);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].text(), "tunnel: cloudflared");
    // Another sandbox has its own acknowledgement.
    let mut other = Seen::open(seen_file(&env, "s2"));
    other.fresh(vec![tunnel_in(&argv("ngrok http 80")).unwrap()]);
    assert_eq!(marks(&env, "s2", project).len(), 1);
}

#[test]
fn egress_findings_reach_the_marks_and_the_ack() {
    let tmp = tempfile::tempdir().unwrap();
    let env = test_env(tmp.path());
    let project = Path::new("/work/app");
    let file = egress::found_file(&env, "s1");
    crate::privfs::dir(file.parent().unwrap()).unwrap();
    std::fs::write(&file, r#"{"new_host":"new.example","new_hosts":"11"}"#).unwrap();
    let texts: Vec<String> = marks(&env, "s1", project)
        .iter()
        .map(Finding::text)
        .collect();
    assert_eq!(
        texts,
        ["new host: new.example", "new hosts: 11 in 10 minutes"]
    );
    assert_eq!(ack(&env, "s1").unwrap(), 2);
    assert!(marks(&env, "s1", project).is_empty());
}

#[test]
fn mute_matches_the_project_and_its_folders() {
    let mute = parse_mute("[monitor]\nmute = [\"/work/noisy\", \"relative/path\", \"/srv/x/\"]\n");
    assert_eq!(
        mute,
        [PathBuf::from("/work/noisy"), PathBuf::from("/srv/x/")]
    );
    assert!(is_muted(&mute, Path::new("/work/noisy")));
    assert!(is_muted(&mute, Path::new("/work/noisy/sub/dir")));
    assert!(is_muted(&mute, Path::new("/srv/x")));
    assert!(!is_muted(&mute, Path::new("/work/noisy2")));
    assert!(!is_muted(&mute, Path::new("/work")));
    assert!(!is_muted(&mute, Path::new("/elsewhere/relative/path")));
    assert!(parse_mute("").is_empty());
    assert!(parse_mute("[monitor]\nmute = \"/work\"\n").is_empty());
}

#[test]
fn a_muted_project_has_no_mark() {
    let tmp = tempfile::tempdir().unwrap();
    let env = test_env(tmp.path());
    let mut seen = Seen::open(seen_file(&env, "s1"));
    seen.fresh(vec![tunnel_in(&argv("ngrok http 80")).unwrap()]);
    let project = Path::new("/work/noisy/app");
    assert_eq!(marks(&env, "s1", project).len(), 1);
    edit_mute(&env, Path::new("/work/noisy"), true).unwrap();
    assert!(marks(&env, "s1", project).is_empty());
    assert!(!notify_now(&env, project, true));
    assert!(notify_now(&env, Path::new("/work/other"), true));
    // The findings stay for the list.
    assert_eq!(findings(&env, "s1").len(), 1);
    edit_mute(&env, Path::new("/work/noisy/"), false).unwrap();
    assert_eq!(marks(&env, "s1", project).len(), 1);
}

#[test]
fn mute_edits_keep_the_rest_of_the_file() {
    let text = "# my settings\n[monitor]\n# quiet here\nnotify = true\n\n[env]\nhide = [\"A\"]\n";
    let (out, list) = edit_mute_text(text, Path::new("/work/a/"), true).unwrap();
    assert_eq!(list, ["/work/a"]);
    assert!(out.contains("# my settings"));
    assert!(out.contains("# quiet here"));
    assert!(out.contains("hide = [\"A\"]"));
    assert_eq!(parse_mute(&out), [PathBuf::from("/work/a")]);
    // Twice is once.
    let (out, list) = edit_mute_text(&out, Path::new("/work/a"), true).unwrap();
    assert_eq!(list, ["/work/a"]);
    let (out, list) = edit_mute_text(&out, Path::new("/work/b"), true).unwrap();
    assert_eq!(list, ["/work/a", "/work/b"]);
    let (out, list) = edit_mute_text(&out, Path::new("/work/a"), false).unwrap();
    assert_eq!(list, ["/work/b"]);
    assert!(out.contains("# quiet here"));
    assert!(edit_mute_text("", Path::new("rel"), true).is_err());
    assert!(edit_mute_text("[monitor]\nmute = 3\n", Path::new("/a"), true).is_err());
    assert!(edit_mute_text("not toml [", Path::new("/a"), true).is_err());
}

#[test]
fn credential_scan_walks_breadth_first_and_does_not_drop_entries_on_budget_exhaustion() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");

    // In LIFO, descending into a large directory exhausts the budget before
    // shallow credential folders like .ssh are reached.
    put(&home, "a_deep/d1/d2/d3/f1", "x");
    put(&home, "a_deep/d1/d2/d3/f2", "x");
    put(&home, "a_deep/d1/d2/d3/f3", "x");
    put(&home, "a_deep/d1/d2/d3/f4", "x");
    put(&home, ".ssh/id_ed25519", "key");

    let (out, _) = scan_limit(&home, 4, &|r| is_credential(r));
    assert!(out.contains_key(".ssh/id_ed25519"));

    let mut c = Creds::new(home.clone(), vec![]);
    assert_eq!(c.check(), (vec![], 0));

    // When a subsequent check exhausts its budget early,
    // previously known entries must not be dropped from the snapshot.
    let _ = c.check_with_limit(1);

    let (found, _) = c.check();
    assert!(found.is_empty(), "unexpected false alert: {found:?}");
}

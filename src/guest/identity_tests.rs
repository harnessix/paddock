use super::*;

fn table(edges: &[(u32, u32)]) -> ProcTable {
    edges.iter().copied().collect()
}

#[test]
fn outer_app_cases() {
    // launchd 1 > ChatGPT 500 > helper 501 > curl 502
    let t = table(&[(500, 1), (501, 500), (502, 501), (600, 1), (700, 650)]);
    let bundles = |pid: u32| match pid {
        500 => Some("com.openai.chat".to_string()),
        501 => Some("com.openai.chat.helper".to_string()),
        600 => Some("com.apple.TextEdit".to_string()),
        _ => None,
    };
    assert_eq!(
        outer_app(502, &t, bundles).as_deref(),
        Some("com.openai.chat"),
        "the outermost app, not the helper"
    );
    assert_eq!(
        outer_app(501, &t, bundles).as_deref(),
        Some("com.openai.chat"),
        "a helper under an app"
    );
    assert_eq!(
        outer_app(500, &t, bundles),
        None,
        "the app itself has no app above"
    );
    assert_eq!(outer_app(600, &t, bundles), None);
    assert_eq!(
        outer_app(700, &t, bundles),
        None,
        "a parent that is not in the table"
    );
    assert_eq!(outer_app(999, &t, bundles), None, "an unknown pid");
    // A cycle ends.
    let cyc = table(&[(10, 11), (11, 10)]);
    let mut calls = 0;
    assert_eq!(
        outer_app(10, &cyc, |_| {
            calls += 1;
            None
        }),
        None
    );
    assert!(calls <= 64);
}

#[test]
fn cache_evicts() {
    let mut c = Cache::default();
    assert!(c.is_empty());
    for i in 0..CACHE_MAX as u32 {
        c.put(i, "/bin/a", Signing::default());
    }
    assert_eq!(c.len(), CACHE_MAX);
    assert!(c.get(0, "/bin/a").is_some());
    c.put(
        5000,
        "/bin/b",
        Signing {
            bundle: Some("x".into()),
            ..Default::default()
        },
    );
    assert_eq!(c.len(), CACHE_MAX, "capped at 1024");
    assert!(c.get(0, "/bin/a").is_none(), "the oldest went");
    assert!(c.get(1, "/bin/a").is_some());
    assert_eq!(c.get(5000, "/bin/b").unwrap().bundle.as_deref(), Some("x"));
    // The same key again does not grow the cache, and the path is part
    // of the key.
    c.put(5000, "/bin/b", Signing::default());
    assert_eq!(c.len(), CACHE_MAX);
    assert_eq!(c.get(5000, "/bin/b").unwrap().bundle, None);
    assert!(c.get(5000, "/bin/c").is_none());
}

#[test]
fn netstat_fallback_reads_the_epid_column() {
    let out = "\
Active Internet connections (including servers)
Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)      rhiwat  shiwat    pid   epid  state    options
tcp4       0      0  127.0.0.1.3128         127.0.0.1.52345        ESTABLISHED  131072  131072    410      0 00102 00000004
tcp4       0      0  127.0.0.1.52345        127.0.0.1.3128         ESTABLISHED  131072  131072      0    612 00102 00000004
tcp4       0      0  127.0.0.1.52346        127.0.0.1.3128         ESTABLISHED  131072  131072    700    800 00102 00000004
tcp46      0      0  *.3128                 *.*                    LISTEN       131072  131072    410      0 00100 00000006
";
    assert_eq!(
        parse_netstat(out, 52345),
        Some(612),
        "pid 0: the epid names the process"
    );
    assert_eq!(
        parse_netstat(out, 52346),
        Some(700),
        "a pid wins over the epid"
    );
    assert_eq!(parse_netstat(out, 3128), Some(410));
    assert_eq!(parse_netstat(out, 1), None);
    assert_eq!(parse_netstat("", 52345), None);
}

#[test]
fn pcblist_buffer_gives_the_e_pid() {
    // xinpgen of 24 bytes, then an inpcb record (kind 0x10) with the
    // local port 52345, then its socket record (kind 0x01) with
    // so_last_pid 0 and so_e_pid 612, then another pair for port 7 with
    // so_last_pid 9.
    let mut buf = Vec::new();
    buf.extend_from_slice(&24u32.to_le_bytes());
    buf.extend_from_slice(&[0u8; 20]);
    let mut pair = |port: u16, last: i32, e: i32| {
        let mut inp = vec![0u8; 32];
        inp[..4].copy_from_slice(&32u32.to_le_bytes());
        inp[4..8].copy_from_slice(&0x10u32.to_le_bytes());
        inp[18..20].copy_from_slice(&port.to_be_bytes());
        buf.extend_from_slice(&inp);
        let mut so = vec![0u8; 96];
        so[..4].copy_from_slice(&96u32.to_le_bytes());
        so[4..8].copy_from_slice(&1u32.to_le_bytes());
        so[72..76].copy_from_slice(&last.to_le_bytes());
        so[76..80].copy_from_slice(&e.to_le_bytes());
        buf.extend_from_slice(&so);
    };
    pair(52345, 0, 612);
    pair(7, 9, 0);
    pair(8, 0, 0);
    assert_eq!(pcblist_find(&buf, 52345), Some(612));
    assert_eq!(pcblist_find(&buf, 7), Some(9));
    assert_eq!(pcblist_find(&buf, 8), None, "no pid at all");
    assert_eq!(pcblist_find(&buf, 9), None);
    for n in 0..buf.len() {
        let _ = pcblist_find(&buf[..n], 52345);
    }
}

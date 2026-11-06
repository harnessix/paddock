use super::*;

#[test]
fn a_record_without_the_terminal_window_reads_with_none() {
    let r: Running = serde_json::from_value(serde_json::json!({
        "id": "codex-api-1a2b",
        "harness": "codex",
        "project": "/work/api",
        "backend": "seatbelt",
        "network": "egress",
        "gui": "off",
        "pid": 4242,
        "started": 1000,
    }))
    .unwrap();
    assert_eq!(r.terminal_window, None);
    assert_eq!(r.terminal_pid, None);
}

#[test]
fn the_terminal_window_is_in_the_record() {
    let r = Running {
        id: "codex-api-1a2b".into(),
        gui: "off".into(),
        terminal_window: Some(812),
        terminal_pid: Some(530),
        ..Default::default()
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["terminal_window"], 812);
    assert_eq!(v["terminal_pid"], 530);
    let back: Running = serde_json::from_value(v).unwrap();
    assert_eq!(
        (back.terminal_window, back.terminal_pid),
        (Some(812), Some(530))
    );
}

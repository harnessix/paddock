use super::*;

#[test]
fn spawn_scope_respects_memory_high_opt_out() {
    let s = setup();
    write_user_config(&s.env, "[memory]\nhigh = \"max\"\n");
    let sp = Spawner::with_program(
        &s.env,
        SpawnCfg::default(),
        ceiling("p-1", 0, &s.project),
        &s.project,
        vec!["/usr/bin/paddock".into(), "spawn-child".into()],
    );
    let log = std::fs::File::create(s.project.join("high-log")).unwrap();
    let cmd = sp
        .command(
            &s.project.join("job.json"),
            log,
            Limits {
                timeout: 600,
                mem: None,
                cpu_percent: None,
            },
            Some(Path::new("/usr/bin/systemd-run")),
            "paddock-spawn-x",
        )
        .unwrap();
    assert!(!cmd
        .get_args()
        .any(|a| a.to_string_lossy().starts_with("MemoryHigh=")));
}

#[test]
fn limits_can_only_be_lower_than_the_configuration() {
    let c = SpawnCfg {
        timeout: 1800,
        max_mem: Some(4 << 30),
        max_cpu_percent: Some(200),
        ..Default::default()
    };
    // Without a request, the child gets the configured limits.
    let l = c.limits(None, None, None).unwrap();
    assert_eq!(
        (l.timeout, l.mem, l.cpu_percent),
        (1800, Some(4 << 30), Some(200))
    );
    let l = c.limits(Some(600), Some(1 << 30), Some(50)).unwrap();
    assert_eq!(
        (l.timeout, l.mem, l.cpu_percent),
        (600, Some(1 << 30), Some(50))
    );
    assert!(c.limits(Some(3600), None, None).is_err());
    assert!(c.limits(Some(0), None, None).is_err());
    assert!(c.limits(None, Some(8 << 30), None).is_err());
    assert!(c.limits(None, None, Some(400)).is_err());
    // With no configured maximum, a request only adds a limit.
    let open = SpawnCfg::default();
    let l = open.limits(None, Some(2 << 30), None).unwrap();
    assert_eq!((l.mem, l.cpu_percent), (Some(2 << 30), None));
}

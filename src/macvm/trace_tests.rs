use super::*;

fn d(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn field(f: &[(&'static str, f64)], name: &str) -> f64 {
    f.iter().find(|(k, _)| *k == name).map(|(_, v)| *v).unwrap()
}

#[test]
fn percentiles_of_few_and_no_values() {
    assert_eq!(percentile(&[], 0.5), 0.0);
    assert_eq!(percentile(&[7.0], 0.95), 7.0);
    let v: Vec<f64> = (1..=20).map(f64::from).collect();
    assert_eq!(percentile(&v, 0.5), 11.0);
    assert_eq!(percentile(&v, 0.95), 19.0);
    assert_eq!(percentile(&[3.0, 1.0, 2.0], 0.0), 1.0);
}

#[test]
fn a_resize_trace_has_every_stage() {
    let t0 = Instant::now();
    let mut t = Trace::new(t0);
    t.step(d(2));
    t.sent(t0);
    t.behind(true, t0);
    t.step(d(4));
    t.replied(t0 + d(18), Some(6.5));
    t.painted(Duration::from_micros(800));
    t.sent(t0 + d(18));
    t.replied(t0 + d(30), None);
    t.behind(false, t0 + d(30));
    // A reply without a move in flight (a timeout before) adds no time.
    t.replied(t0 + d(40), Some(f64::NAN));
    t.behind(true, t0 + d(40));
    let f = t.fields(3, t0 + d(50));
    assert_eq!(field(&f, "ms"), 50.0);
    assert_eq!(field(&f, "steps"), 2.0);
    assert_eq!(field(&f, "sent"), 2.0);
    assert_eq!(field(&f, "dropped"), 3.0);
    assert_eq!(field(&f, "host_p95"), 4.0);
    assert_eq!(field(&f, "rtt_p50"), 18.0);
    assert_eq!(field(&f, "rtt_p95"), 18.0);
    assert_eq!(field(&f, "agent_p50"), 6.5);
    assert_eq!(field(&f, "paint_p95"), 0.8);
    // Behind from 0 to 30, then from 40 to the end at 50.
    assert_eq!(field(&f, "behind_max"), 30.0);
    assert_eq!(field(&f, "behind_sum"), 40.0);
}

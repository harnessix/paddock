use std::io::{self, BufRead};
fn main() {
    for (n, line) in io::stdin().lock().lines().enumerate() {
        let line = line.unwrap();
        let row = codex_rollout::parse_rollout_line(&line)
            .unwrap_or_else(|e| panic!("line {}: {e}: {line}", n + 1));
        let mut output = serde_json::to_string(&row).unwrap();
        // Exercise forward-compatible fields after upstream serialization.
        if n == 0 {
            output.pop();
            output.push_str(",\"future_field\":{\"ratio\":1.2300}}");
        }
        codex_rollout::parse_rollout_line(&output).unwrap();
        println!("{output}");
    }
}

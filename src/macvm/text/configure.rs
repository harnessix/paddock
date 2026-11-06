//! The texts of the configure step of `paddock macvm setup`: one line
//! per step result, on the terminal and in the event log.

use crate::macvm::proto::StepResult;

/// The longest piece of a step output in a line.
const TAIL: usize = 240;

/// The last `TAIL` characters of an output, on one line.
fn tail(s: &str) -> String {
    let one: String = s
        .trim()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    match one.char_indices().nth_back(TAIL) {
        Some((i, _)) => format!("...{}", &one[i..]),
        None => one,
    }
}

/// The line of a quiet step: as the line of a step, and when it did not
/// take, why the setup goes on.
pub fn configure_quiet_line(line: &str, proven: bool) -> String {
    if proven {
        return line.to_string();
    }
    format!("{line}; a quiet step, the golden goes on without it (the firewall denies the guest system anyway)")
}

/// One step result: the name, ok or FAILED, the exit code and the time;
/// for a failure the output and what the verify printed.
pub fn configure_step_line(r: &StepResult, proven: bool, expect: &str) -> String {
    let secs = r.ms as f64 / 1000.0;
    let code = match (r.timed_out, r.code) {
        (true, _) => format!(
            "timed out after {} s, killed",
            crate::macvm::configure::STEP_TIMEOUT_SECS
        ),
        (false, Some(c)) => format!("exit {c}"),
        (false, None) => "did not run".to_string(),
    };
    if proven {
        return format!("configure step {}: ok, {code}, {secs:.1} s", r.name);
    }
    let mut line = format!("configure step {}: FAILED, {code}, {secs:.1} s", r.name);
    let out = tail(&r.output);
    if !out.is_empty() {
        line.push_str(&format!(", output: {out}"));
    }
    if r.ok && !expect.is_empty() {
        line.push_str(&format!(
            ", the check printed {:?} without {expect:?}",
            tail(&r.verify)
        ));
    }
    line
}

/// The agent sent no result for a step.
pub fn configure_step_missing(name: &str) -> String {
    format!("configure step {name}: FAILED, no result from the guest agent")
}

/// The main loop gave no result for a step of the agent.
pub fn agent_step_not_run(name: &str) -> String {
    format!("the agent did not run its own step {name}")
}

/// This agent does not know a step of its own.
pub fn agent_step_unknown(argv: &[String]) -> String {
    format!(
        "this guest agent has no step {:?}; update paddock",
        argv.join(" ")
    )
}

/// The host sent no guest password with the shutdown request.
pub fn shutdown_no_password() -> String {
    "the shutdown request has no guest password; the host stops the VM instead".to_string()
}

/// `sudo shutdown` in the guest failed: the exit code and the output.
pub fn shutdown_failed(code: Option<i32>, output: &str) -> String {
    let code = code.map_or("none".to_string(), |c| c.to_string());
    let output = output.trim();
    if output.is_empty() {
        format!("sudo shutdown -h now in the guest failed with exit {code}; the host stops the VM instead")
    } else {
        format!("sudo shutdown -h now in the guest failed with exit {code}: {output}; the host stops the VM instead")
    }
}

/// `sudo shutdown` in the guest did not end in time and was killed.
pub fn shutdown_timed_out(secs: u64) -> String {
    format!("sudo shutdown -h now in the guest did not end in {secs} s and was killed; the host stops the VM instead")
}

#[cfg(test)]
#[path = "configure_tests.rs"]
mod tests;

/// A per-machine configure step failed on a profile's machine.
pub fn machine_step_failed(step: &str) -> String {
    format!("the configure step {step} failed on this profile's machine: the guest may sleep, start its screen saver or lock its screen")
}

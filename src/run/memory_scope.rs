//! Create a scope and report when its memory boundary is unavailable.
use crate::env::HostEnv;
use crate::spec::SandboxSpec;

/// Whether paddock can apply `memory.high` on a host: only Linux has the
/// systemd scope that takes it. macOS gets no word about systemd.
pub fn applies(macos: bool) -> bool {
    !macos
}

/// The memory line of `paddock explain`, or None where paddock cannot
/// apply the limit.
pub fn explain_line(high: Option<u64>, macos: bool) -> Option<String> {
    if !applies(macos) {
        return None;
    }
    Some(match high {
        Some(high) => format!("planned high {high} bytes (systemd scope only)"),
        None => "high disabled".into(),
    })
}

/// The warning when the limit could not be applied, or None.
fn unapplied(has_high: bool, has_unit: bool, quiet: bool, macos: bool) -> Option<&'static str> {
    (has_high && !has_unit && !quiet && applies(macos))
        .then_some("paddock: memory.high is not applied: this run has no accessible systemd scope")
}

pub(super) fn establish(
    env: &HostEnv,
    pid: u32,
    new_scope: Option<String>,
    inherited: Option<String>,
    spec: &SandboxSpec,
    quiet: bool,
) -> Option<String> {
    let unit = match new_scope {
        Some(name) => {
            let moved = crate::freeze::wait_stopped(pid).and_then(|_| {
                crate::freeze::move_to_scope(env, pid, &name, &spec.id, spec.memory_high)
            });
            // It continues in any case; without the scope it only cannot be
            // frozen.
            crate::freeze::resume(pid);
            match moved {
                Ok(applied) => {
                    if !applied && !quiet {
                        crate::note::info(
                            "memory.high is not applied: systemd refused the setting",
                        );
                    }
                    Some(name)
                }
                Err(e) => {
                    log::debug!("no scope for the sandbox, so it cannot be frozen: {e:#}");
                    None
                }
            }
        }
        None => inherited,
    };
    if let Some(line) = unapplied(
        spec.memory_high.is_some(),
        unit.is_some(),
        quiet,
        cfg!(target_os = "macos"),
    ) {
        crate::note::say_line(crate::note::Level::Info, line);
    }
    unit
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MACRUN1: a macOS run says nothing about systemd.
    #[test]
    fn macos_says_nothing_about_systemd() {
        assert_eq!(unapplied(true, false, false, true), None);
        assert_eq!(explain_line(Some(1 << 30), true), None);
        assert_eq!(explain_line(None, true), None);
    }

    #[test]
    fn linux_says_when_the_limit_is_not_applied() {
        assert!(unapplied(true, false, false, false)
            .is_some_and(|l| l.contains("no accessible systemd scope")));
        assert_eq!(unapplied(true, true, false, false), None);
        assert_eq!(unapplied(true, false, true, false), None);
        assert_eq!(unapplied(false, false, false, false), None);
        assert_eq!(
            explain_line(Some(1024), false).as_deref(),
            Some("planned high 1024 bytes (systemd scope only)")
        );
        assert_eq!(explain_line(None, false).as_deref(), Some("high disabled"));
    }
}

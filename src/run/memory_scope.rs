//! Create a scope and report when its memory boundary is unavailable.
use crate::env::HostEnv;
use crate::spec::SandboxSpec;

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
                        eprintln!(
                            "paddock: memory.high is not applied: systemd refused the setting"
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
    if spec.memory_high.is_some() && unit.is_none() && !quiet {
        eprintln!("paddock: memory.high is not applied: this run has no accessible systemd scope");
    }
    unit
}

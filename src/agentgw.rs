//! Gateway agents: an agent whose interface is a thin client of a gateway
//! service. The gateway owns the agent's state and runs its tools, so a
//! sandbox around the client alone protects nothing.
//!
//! paddock runs the gateway itself in a sandbox, as a long-running service
//! of the user, and attaches the real client to it from a second, small
//! sandbox:
//!
//! - shape.rs: `[gateway]` of a harness profile, the commands, the port
//!   and how the client gets the URL and the token.
//! - state.rs: the token, the start settings, the state of the supervisor
//!   and the attached clients, all under `<paddock state>/gateways/NAME`.
//! - spec.rs: the gateway sandbox (its own home, loopback only) and the
//!   client sandbox (no access to the gateway's home).
//! - launch.rs: the backend, and the loopback checks.
//! - serve.rs: `paddock gateway-serve NAME`, the supervisor that restarts a
//!   gateway that ended.
//! - control.rs: `start` and `stop`, through systemd, launchd or a detached
//!   process.
//! - attach.rs: `paddock NAME tui`.
//! - report.rs: the JSON of `ps`, `board` and `gateway status`, and the
//!   lines of `doctor`.
//!
//! paddock never installs the gateway's own host service (`openclaw
//! daemon`). A gateway that the user starts on the host stays possible;
//! paddock tells it apart and says how to move to the sandboxed one.

pub mod attach;
pub mod control;
pub mod launch;
pub mod report;
pub mod serve;
pub mod shape;
pub mod spec;
pub mod state;

use crate::profile::Profile;
use anyhow::{Context as _, Result};

pub use shape::{Form, Shape};

/// The checked `[gateway]` of a profile.
pub fn shape_of(profile: &Profile) -> Result<Shape> {
    let s = profile.gateway.clone().with_context(|| {
        format!(
            "the {} profile has no [gateway] table: paddock runs no gateway for it",
            profile.name
        )
    })?;
    s.check(&profile.name)?;
    Ok(s)
}

/// What a command of a gateway agent is, as `paddock wrap` and the shims
/// see it. `program` is the base name of the command, `args` its
/// arguments. The harness's own command (`openclaw tui`) is read by its
/// first argument. A launcher of the harness (`omarchy-launch-openclaw
/// --tui`) is a client with `--tui`, and else a command for the host.
pub fn form_of(
    profile: &Profile,
    shape: &Shape,
    program: &str,
    args: &[String],
) -> (Form, String, Vec<String>) {
    let own = profile.locate.commands.first().is_none_or(|c| c == program);
    if own {
        let form = shape.classify(args);
        let (cmd, extra) = match args.split_first() {
            Some((c, rest)) if form == Form::Client => (c.clone(), rest.to_vec()),
            _ => (String::new(), args.to_vec()),
        };
        return (form, cmd, extra);
    }
    if args.iter().any(|a| a == "--tui") {
        (Form::Client, shape.default_client().to_string(), Vec::new())
    } else {
        (Form::Host, String::new(), args.to_vec())
    }
}

/// The `wrap_on_host` notice of an agent run. A gateway agent's own run is
/// its embedded runtime, so the notice is only for its host gateway.
pub fn wrap_notice(profile: &Profile) -> Option<&String> {
    profile
        .launch
        .wrap_on_host
        .as_ref()
        .filter(|_| profile.gateway.is_none())
}

/// The refusal of a host form inside `paddock run`.
pub fn host_refusal(name: &str, command: &str) -> String {
    format!(
        "`{name} {command}` starts or installs a {name} gateway on the host. paddock runs the gateway in a sandbox instead: paddock gateway {name} start. To run the command on the host anyway: PADDOCK_BYPASS=1 {name} {command}"
    )
}

#[cfg(test)]
#[path = "agentgw/tests.rs"]
mod tests;

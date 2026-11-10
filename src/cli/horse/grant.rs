//! `paddock horse grant` and `paddock horse grants`: the ctl grant of a
//! sandbox.

use anyhow::Result;
use paddock::horse::device::ctl::grant::{self, Grant};
use paddock::horse::{self, Store};

pub(super) fn run(
    st: &Store,
    sandbox: &str,
    allow: Vec<String>,
    horses: Vec<String>,
    commands: u64,
    tokens: u64,
    revoke: bool,
) -> Result<i32> {
    if revoke {
        let ctl = grant::revoke(st, sandbox)?;
        let peek = horse::device::peek::revoke(st, sandbox)?;
        if ctl || peek {
            println!("the grants of sandbox {sandbox} are gone");
        } else {
            println!("sandbox {sandbox} has no ctl grant");
        }
        return Ok(0);
    }
    let g = Grant {
        sandbox: sandbox.into(),
        commands: allow,
        horses,
        budget: commands,
        tokens,
        used: 0,
        spent_tokens: 0,
        granted_ms: horse::now_ms(),
    };
    grant::set(st, &g)?;
    println!("{}", line(&g));
    if tokens == 0 {
        println!("every command that breaks the cached prefix is refused; --tokens N allows breaks up to N tokens, each with an approval");
    }
    Ok(0)
}

fn line(g: &Grant) -> String {
    let mut s = format!(
        "sandbox {}: {}; {} of {} commands left; {} of {} tokens left",
        g.sandbox,
        g.commands.join(", "),
        g.commands_left(),
        g.budget,
        g.tokens_left(),
        g.tokens
    );
    if !g.horses.is_empty() {
        s += &format!("; move to {}", g.horses.join(", "));
    }
    s
}

pub(super) fn list(st: &Store) -> Result<i32> {
    let all = grant::list(st)?;
    if all.is_empty() {
        println!("no ctl grant");
    }
    for g in all {
        println!("{}", line(&g));
    }
    Ok(0)
}

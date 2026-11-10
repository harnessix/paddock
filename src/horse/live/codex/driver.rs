//! Drive a Codex app-server custody.
use super::{wire, Codex, CustodyCtx, Policy, Report};
use crate::horse::live::harness::{ChannelIo, Driver, Ends};
use anyhow::Result;
use std::io::{Read, Write};

/// Run the custody until it ends (`ends`) or the server exits.
pub fn run<R, W>(io: ChannelIo<R, W>, ctx: CustodyCtx, policy: Policy, ends: Ends) -> Result<Report>
where
    R: Read + Send + 'static,
    W: Write,
{
    let (tx, rx) = std::sync::mpsc::channel();
    wire::read_server(io.from, tx);
    let mut d = Driver::new(ctx.store.clone(), &ctx.horse, &ctx.run, ctx.tick)
        .close_wait(ctx.close_timeout)
        .ends(ends);
    let mut c = Codex::start(io.to, rx, ctx, policy)?;
    d.drive(&mut c, &io.control)?;
    Ok(c.report())
}

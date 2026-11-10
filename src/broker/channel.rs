//! The two live requests of a horse-mode TUI run (`horse.channel_wait`
//! from `paddock-host channel`, `horse.safe_point` from the hooks): the
//! device ledger of the broker around `horse::live::tui_state`.

use super::{Broker, Request, Response};
use crate::devices::ledger;
use crate::horse::live::tui_state;

impl Broker {
    /// The response to a live horse request, or None for any other request.
    pub(super) fn handle_horse_live(&self, req: &Request) -> Option<Response> {
        if !tui_state::is_live_op(req) {
            return None;
        }
        let record = |line: &str| {
            let project = self.ctx.project.to_string_lossy();
            let payer = ledger::Payer {
                sandbox: &self.ctx.id,
                root: ledger::root_of(&self.ctx.id, &self.ctx.chain),
                project: &project,
                device: "horse",
            };
            if let Err(e) =
                ledger::Ledger::open(&ledger::path(&self.env)).and_then(|l| l.record(&payer, line))
            {
                log::warn!("horse ledger: {e:#}");
            }
        };
        Some(tui_state::handle(self.horse.get(), req, &record))
    }
}

//! The kv responses for an approval that got no answer.

use super::{Response, Status};
use crate::broker::ApprovalOutcome;
use std::time::Duration;

/// Preserve the distinction between an unanswered card, a card that could
/// not be published, and an outcome that cannot be established.
pub(super) fn approval_failure(
    outcome: &ApprovalOutcome,
    wait: Duration,
) -> Option<(&'static str, Response)> {
    match outcome {
        ApprovalOutcome::Answered(_) => None,
        ApprovalOutcome::NoAnswer => Some((
            "timed out",
            Response::new(
                Status::Timeout,
                format!("nobody answered within {} seconds", wait.as_secs()),
            ),
        )),
        ApprovalOutcome::Unavailable => Some((
            "approval unavailable",
            Response::new(Status::Failed, "the approval request was not published"),
        )),
        ApprovalOutcome::Unknown => Some((
            "approval outcome unknown",
            Response::new(Status::Failed, "the approval outcome is unknown"),
        )),
    }
}

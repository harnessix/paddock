//! The catalog description of the question device.

use super::*;

/// The description of `ask`, for the catalog.
pub fn ask_info() -> Info {
    Info::builtin(
        format!(
            "a question for the user, as text (at most {MAX_QUESTION} characters); or JSON {{\"question\": TEXT, \"choices\": [up to {MAX_CHOICES} short answers], \"timeout\": SECONDS}}. The question appears on the approval card on the host. One read waits at most timeout seconds (default {}, at most {}), then fails with ETIME (still waiting): send the same question again, and the read finds the open question. A question that nobody reads for {} seconds is withdrawn. At most {MAX_RETAINED_QUESTIONS} open questions and retained answers fit in one sandbox; read an earlier answer before asking more.",
            waiting::DEFAULT_READ_TIMEOUT,
            waiting::MAX_READ_TIMEOUT,
            ABANDONED.as_secs()
        ),
        format!(
            "the user's answer, as text (at most {} characters). EACCES: the user dismissed the question",
            broker::MAX_REPLY
        ),
        limits(MAX_QUESTION * 4 + 4096, broker::MAX_REPLY * 4),
    )
}

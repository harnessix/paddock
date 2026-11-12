//! The result of a child's time-limit approval card.

/// What the user decided on the card of a child at its time limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// More time: one of the choices on the card.
    More(String),
    /// Stop the child.
    Stop,
    /// No answer within the approval wait, or the child stopped meanwhile.
    NoAnswer,
    /// No card was published, so there was no opportunity to answer.
    Unavailable,
    /// The store could not establish whether an answer was accepted.
    Unknown,
}

use codex_history::InputSource;
use codex_protocol::protocol::Submission;

/// Process-local provenance accompanying an internal Core submission.
///
/// This envelope is intentionally not serialized. Generic submission callers retain the
/// conservative `Unknown` source unless a Core-owned producer explicitly marks its input. The
/// source is descriptive data, not proof of admission, persistence, or transcript completeness.
#[derive(Debug)]
pub(crate) struct SessionSubmission {
    pub(crate) submission: Submission,
    pub(crate) input_source: InputSource,
}

impl From<Submission> for SessionSubmission {
    fn from(submission: Submission) -> Self {
        Self {
            submission,
            input_source: InputSource::Unknown,
        }
    }
}

#[cfg(test)]
#[path = "submission_tests.rs"]
mod tests;

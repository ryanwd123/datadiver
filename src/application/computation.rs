use crate::application::session::JobToken;
use crate::infrastructure::cancellation::Cancellation;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScheduleReason {
    TextEdit,
    Immediate,
}

impl ScheduleReason {
    pub fn debounce(self) -> Option<std::time::Duration> {
        match self {
            Self::TextEdit => Some(std::time::Duration::from_millis(50)),
            Self::Immediate => None,
        }
    }
}

pub struct ComputationController<R> {
    active: Option<(JobToken, Arc<Cancellation>)>,
    pending: Option<(JobToken, R)>,
    latest: JobToken,
}

impl<R> ComputationController<R> {
    pub fn new(initial: JobToken) -> Self {
        Self {
            active: None,
            pending: None,
            latest: initial,
        }
    }

    pub fn invalidate(&mut self, token: JobToken) {
        // Only the latest token publishes, so the active work is now wasted.
        if let Some((active, cancellation)) = &self.active
            && *active != token
        {
            cancellation.cancel();
        }
        self.latest = token;
        self.pending = None;
    }

    pub fn enqueue(&mut self, token: JobToken, request: R) -> Option<(JobToken, R)> {
        if token != self.latest {
            return None;
        }
        if self.active.is_some() {
            self.pending = Some((token, request));
            None
        } else {
            self.active = Some((token, Arc::default()));
            Some((token, request))
        }
    }

    pub fn finish(&mut self, token: JobToken) -> Option<(JobToken, R)> {
        if self.active.as_ref().is_none_or(|(active, _)| *active != token) {
            return None;
        }
        self.active = None;
        if let Some((next_token, request)) = self.pending.take()
            && next_token == self.latest
        {
            self.active = Some((next_token, Arc::default()));
            return Some((next_token, request));
        }
        None
    }

    pub fn may_publish(&self, token: JobToken) -> bool {
        token == self.latest
    }
    pub fn is_running(&self) -> bool {
        self.active.is_some()
    }

    /// Stops the active work's queries once newer input replaces it.
    pub fn cancellation(&self) -> Arc<Cancellation> {
        self.active
            .as_ref()
            .map(|(_, cancellation)| cancellation.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_text_edits_have_a_debounce() {
        assert_eq!(
            ScheduleReason::TextEdit.debounce(),
            Some(std::time::Duration::from_millis(50))
        );
        assert_eq!(ScheduleReason::Immediate.debounce(), None);
    }

    fn token(revision: u64) -> JobToken {
        JobToken {
            session_generation: 1,
            source_revision: 2,
            request_revision: revision,
        }
    }

    #[test]
    fn active_work_keeps_its_slot_and_pending_is_replaceable() {
        let mut controller = ComputationController::new(token(0));
        controller.invalidate(token(1));
        assert_eq!(
            controller.enqueue(token(1), "first"),
            Some((token(1), "first"))
        );
        controller.invalidate(token(2));
        assert_eq!(controller.enqueue(token(2), "second"), None);
        controller.invalidate(token(3));
        assert_eq!(controller.enqueue(token(3), "third"), None);
        assert!(!controller.may_publish(token(1)));
        assert!(controller.is_running());
        assert_eq!(controller.finish(token(1)), Some((token(3), "third")));
        assert_eq!(controller.finish(token(3)), None);
        assert!(!controller.is_running());
    }

    #[test]
    fn invalid_input_clears_pending_and_cancels_active_work() {
        let mut controller = ComputationController::new(token(0));
        controller.invalidate(token(1));
        controller.enqueue(token(1), 1);
        let first = controller.cancellation();
        controller.invalidate(token(2));
        assert!(first.is_cancelled());
        controller.enqueue(token(2), 2);
        controller.invalidate(token(3));
        assert!(controller.is_running());
        assert_eq!(controller.finish(token(1)), None);
        assert!(!controller.is_running());
    }

    #[test]
    fn invalidating_with_the_active_token_keeps_it_running() {
        let mut controller = ComputationController::new(token(0));
        controller.invalidate(token(1));
        controller.enqueue(token(1), "first");
        controller.invalidate(token(1));
        assert!(!controller.cancellation().is_cancelled());
        // Pending work starts with a fresh cancellation.
        controller.invalidate(token(2));
        controller.enqueue(token(2), "second");
        assert_eq!(controller.finish(token(1)), Some((token(2), "second")));
        assert!(!controller.cancellation().is_cancelled());
    }
}

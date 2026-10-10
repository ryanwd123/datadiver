use crate::application::session::JobToken;

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
    active: Option<JobToken>,
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
            self.active = Some(token);
            Some((token, request))
        }
    }

    pub fn finish(&mut self, token: JobToken) -> Option<(JobToken, R)> {
        if self.active != Some(token) {
            return None;
        }
        self.active = None;
        if let Some((next_token, request)) = self.pending.take()
            && next_token == self.latest
        {
            self.active = Some(next_token);
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
    fn invalid_input_clears_pending_without_canceling_active_work() {
        let mut controller = ComputationController::new(token(0));
        controller.invalidate(token(1));
        controller.enqueue(token(1), 1);
        controller.invalidate(token(2));
        controller.enqueue(token(2), 2);
        controller.invalidate(token(3));
        assert!(controller.is_running());
        assert_eq!(controller.finish(token(1)), None);
        assert!(!controller.is_running());
    }
}

//! Research 2.0 contracts are deliberately independent of legacy routes and rounds.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CONTRACT: &str = "mathcat-research/v2";
pub const DEFAULT_DURATION_SECONDS: u64 = 7200;

#[must_use]
pub fn new_id() -> String {
    ulid::Ulid::new().to_string()
}

#[must_use]
pub fn now() -> String {
    Utc::now().to_rfc3339()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Created,
    Preflighting,
    Running,
    WaitingHuman,
    Pausing,
    Paused,
    Stopping,
    Ended,
}

impl RunState {
    #[must_use]
    pub const fn may_transition(self, next: Self) -> bool {
        use RunState::{
            Created, Ended, Paused, Pausing, Preflighting, Running, Stopping, WaitingHuman,
        };
        matches!(
            (self, next),
            (Created, Preflighting | Stopping | Ended)
                | (Preflighting | Paused, Running | Stopping | Ended)
                | (Preflighting, Pausing)
                | (Paused, WaitingHuman)
                | (Running, WaitingHuman | Pausing | Stopping | Ended)
                | (WaitingHuman, Running | Pausing | Stopping | Ended)
                | (Pausing, Paused | Stopping | Ended)
                | (Stopping, Ended)
        )
    }
}

#[must_use]
pub fn deadline_reached(deadline: &str) -> bool {
    DateTime::parse_from_rfc3339(deadline).map_or(true, |value| value <= Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_run_cannot_restart() {
        assert!(!RunState::Ended.may_transition(RunState::Running));
        assert!(!RunState::Paused.may_transition(RunState::Created));
        assert!(RunState::Paused.may_transition(RunState::Running));
        assert!(deadline_reached("invalid"));
    }
    #[test]
    fn identifiers_are_distinct_and_versioned() {
        assert_ne!(new_id(), new_id());
        assert_eq!(VERSION, "2.5.1");
        assert_eq!(CONTRACT, "mathcat-research/v2");
    }
    #[test]
    fn paused_run_can_resume_to_its_unanswered_question() {
        assert!(RunState::Paused.may_transition(RunState::WaitingHuman));
        assert!(!RunState::Ended.may_transition(RunState::WaitingHuman));
        assert!(!RunState::WaitingHuman.may_transition(RunState::Paused));
    }
}

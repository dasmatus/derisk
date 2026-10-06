//! The agent conversation shown in the command palette.
//!
//! Every natural-language request, from the person at the palette or from
//! an external agent over the agent protocol, becomes a [`Turn`]: the
//! request, the steps the assistant planned, and how each step went. Steps
//! that wait for a launched app's window stay [`StepStatus::Waiting`] until
//! it maps, so the palette shows progress as it happens.

use std::collections::VecDeque;

use crate::action::Action;

/// Turns kept; older ones are dropped.
pub const MAX_TURNS: usize = 50;

/// Who made a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    /// The person, typing in the palette.
    User,
    /// An external agent over the agent protocol.
    Agent,
}

/// How one step went.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepStatus {
    /// Applied.
    Done,
    /// Waits for a launched app's window.
    Waiting,
    /// Failed with this message.
    Failed(String),
    /// Not attempted because an earlier step failed.
    Skipped,
}

/// One planned action and its outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Step {
    /// The action.
    pub action: Action,
    /// A human description, from [`Action::label`].
    pub label: String,
    /// Outcome so far.
    pub status: StepStatus,
}

/// One request and what came of it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Turn {
    /// Increasing ID, for "seen" tracking.
    pub id: u64,
    /// Who asked.
    pub source: Source,
    /// What was asked.
    pub request: String,
    /// The steps, in order.
    pub steps: Vec<Step>,
    /// Set when the request was not understood.
    pub error: Option<String>,
}

impl Turn {
    /// A one-line summary of the outcome.
    pub fn reply(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        let failed = self.steps.iter().find_map(|s| match &s.status {
            StepStatus::Failed(e) => Some((s.label.as_str(), e.as_str())),
            _ => None,
        });
        if let Some((label, error)) = failed {
            return format!("Stopped at \"{label}\": {error}");
        }
        let waiting = self
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Waiting)
            .count();
        match waiting {
            0 => "Done.".to_owned(),
            1 => "Waiting for the app's window to finish 1 step…".to_owned(),
            n => format!("Waiting for the app's window to finish {n} steps…"),
        }
    }

    /// Whether every step finished (done or failed), so nothing is pending.
    pub fn is_settled(&self) -> bool {
        self.steps.iter().all(|s| s.status != StepStatus::Waiting)
    }
}

/// A reference to one step of one turn.
pub type StepRef = (u64, usize);

/// Recent turns, oldest first.
#[derive(Clone, Debug, Default)]
pub struct Conversation {
    turns: VecDeque<Turn>,
    next_id: u64,
}

impl Conversation {
    /// Recent turns, oldest first.
    pub fn turns(&self) -> impl DoubleEndedIterator<Item = &Turn> {
        self.turns.iter()
    }

    /// The newest turn's ID, if any.
    pub fn last_id(&self) -> Option<u64> {
        self.turns.back().map(|t| t.id)
    }

    /// Starts a turn for `request` with planned `actions`, all waiting.
    pub fn start(&mut self, source: Source, request: &str, actions: &[Action]) -> u64 {
        self.next_id += 1;
        if self.turns.len() == MAX_TURNS {
            self.turns.pop_front();
        }
        self.turns.push_back(Turn {
            id: self.next_id,
            source,
            request: request.trim().to_owned(),
            steps: actions
                .iter()
                .map(|a| Step {
                    action: a.clone(),
                    label: a.label(),
                    status: StepStatus::Waiting,
                })
                .collect(),
            error: None,
        });
        self.next_id
    }

    /// Records a request that was not understood.
    pub fn not_understood(&mut self, source: Source, request: &str, error: &str) -> u64 {
        let id = self.start(source, request, &[]);
        if let Some(turn) = self.turns.back_mut() {
            turn.error = Some(error.to_owned());
        }
        id
    }

    /// Sets a step's status; unknown (dropped) turns are ignored.
    pub fn set(&mut self, (turn, step): StepRef, status: StepStatus) {
        if let Some(s) = self
            .turns
            .iter_mut()
            .find(|t| t.id == turn)
            .and_then(|t| t.steps.get_mut(step))
        {
            s.status = status;
        }
    }

    /// Forgets every turn.
    pub fn clear(&mut self) {
        self.turns.clear();
    }
}

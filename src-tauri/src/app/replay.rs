//! What a freshly opened progress window needs to show the job that is already running.

use crate::jobs::queue::JobEvent;

/// What a freshly opened window needs to show the job that is already running.
#[derive(Default)]
pub(super) struct Replay {
    queued: Option<JobEvent>,
    started: Option<JobEvent>,
    confirm: Option<JobEvent>,
}

impl Replay {
    pub(super) fn events(&self) -> Vec<JobEvent> {
        [&self.queued, &self.started, &self.confirm].into_iter().flatten().cloned().collect()
    }

    /// Id of the job waiting on a `NeedsConfirm` answer.
    pub(super) fn awaiting_confirm(&self) -> Option<u64> {
        self.confirm.as_ref().and_then(job_id)
    }

    pub(super) fn apply(&mut self, event: &JobEvent) {
        let id = job_id(event);
        match event {
            JobEvent::Queued { .. } => self.queued = Some(event.clone()),
            JobEvent::Started { .. } => {
                self.started = Some(event.clone());
                self.confirm = None;
            }
            JobEvent::NeedsConfirm { .. } => self.confirm = Some(event.clone()),
            JobEvent::Progress { .. } => {
                if self.awaiting_confirm() == id {
                    self.confirm = None;
                }
            }
            JobEvent::Done { .. } | JobEvent::Unchanged { .. } | JobEvent::Failed { .. } => {
                if self.started.as_ref().and_then(job_id) == id {
                    self.started = None;
                }
                if self.awaiting_confirm() == id {
                    self.confirm = None;
                }
            }
            JobEvent::Drained => *self = Replay::default(),
        }
    }
}

pub(super) fn job_id(event: &JobEvent) -> Option<u64> {
    match event {
        JobEvent::Started { id, .. }
        | JobEvent::Progress { id, .. }
        | JobEvent::NeedsConfirm { id, .. }
        | JobEvent::Done { id, .. }
        | JobEvent::Unchanged { id, .. }
        | JobEvent::Failed { id, .. } => Some(*id),
        JobEvent::Queued { .. } | JobEvent::Drained => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::queue::{QueueSnapshot, CANCELLED};

    fn queued(index: usize, total: usize) -> JobEvent {
        JobEvent::Queued { snapshot: QueueSnapshot { current_index: index, total } }
    }

    fn started(id: u64) -> JobEvent {
        JobEvent::Started {
            id,
            input: format!("{id}.mp4"),
            input_bytes: 1,
            kind: Some("compress".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        }
    }

    fn progress(id: u64) -> JobEvent {
        JobEvent::Progress { id, pass: 1, passes: 2, fraction: 0.5, eta_s: None }
    }

    fn failed(id: u64, message: &str) -> JobEvent {
        JobEvent::Failed { id, message: message.into(), original_intact: true }
    }

    #[test]
    fn replay_tracks_only_the_running_job() {
        let mut r = Replay::default();
        r.apply(&queued(2, 3));
        r.apply(&started(2));
        r.apply(&progress(2));
        assert_eq!(r.events(), vec![queued(2, 3), started(2)]);
        // A dropped waiting job's Failed must not clear the running job.
        r.apply(&failed(3, CANCELLED));
        assert_eq!(r.events(), vec![queued(2, 3), started(2)]);
        r.apply(&failed(2, "x"));
        assert_eq!(r.events(), vec![queued(2, 3)]);
        r.apply(&JobEvent::Drained);
        assert!(r.events().is_empty());
    }
}

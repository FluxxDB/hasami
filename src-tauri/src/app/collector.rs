//! Groups the launches that arrive close together (one per selected file) into one batch.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::cli::Action;
use crate::jobs::queue::JobKind;

/// Launches closer together than this are combined into one batch.
pub const COLLECT_WINDOW: Duration = Duration::from_millis(400);

/// Actions gathered during one collection window, grouped by kind in first-seen order.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Batch {
    pub jobs: Vec<(JobKind, Vec<PathBuf>)>,
    pub trims: Vec<PathBuf>,
}

/// Combines actions that arrive within `window` of each other. Time is passed in, so it's testable.
pub(crate) struct Collector {
    window: Duration,
    last: Option<Instant>,
    batch: Batch,
}

impl Collector {
    pub(crate) fn new(window: Duration) -> Self {
        Self { window, last: None, batch: Batch::default() }
    }

    pub(crate) fn push(&mut self, action: Action, now: Instant) {
        match action {
            Action::Compress(files) => self.add(JobKind::Compress, files),
            Action::Convert(target, files) => self.add(JobKind::Convert(target), files),
            Action::Trim(files) => self.batch.trims.extend(files),
            // Handled in `lib::run` before Tauri starts, so they never reach the batch.
            Action::Register | Action::Unregister => return,
        }
        self.last = Some(now);
    }

    fn add(&mut self, kind: JobKind, files: Vec<PathBuf>) {
        match self.batch.jobs.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, existing)) => existing.extend(files),
            None => self.batch.jobs.push((kind, files)),
        }
    }

    /// When the current batch closes, if one is open.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.last.map(|last| last + self.window)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.last.is_none()
    }

    /// The batch, once `window` has passed since the last action.
    pub(crate) fn take_due(&mut self, now: Instant) -> Option<Batch> {
        if now < self.deadline()? {
            return None;
        }
        self.last = None;
        Some(std::mem::take(&mut self.batch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Target;

    fn files(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn launches_within_the_window_form_one_batch_per_kind() {
        let t0 = Instant::now();
        let mut c = Collector::new(COLLECT_WINDOW);
        c.push(Action::Compress(files(&["a.mp4"])), t0);
        c.push(Action::Convert(Target::Mov, files(&["b.mkv"])), t0 + ms(300));
        c.push(Action::Compress(files(&["c.mp4"])), t0 + ms(600));

        // Each launch extends the window: 400 ms after the last one, not the first.
        assert_eq!(c.take_due(t0 + ms(999)), None);
        let batch = c.take_due(t0 + ms(1000)).expect("batch due");
        assert_eq!(
            batch.jobs,
            vec![(JobKind::Compress, files(&["a.mp4", "c.mp4"])), (JobKind::Convert(Target::Mov), files(&["b.mkv"])),]
        );
        assert!(batch.trims.is_empty());
        assert!(c.is_empty());
        assert_eq!(c.deadline(), None);
    }

    #[test]
    fn a_gap_longer_than_the_window_starts_a_new_batch() {
        let t0 = Instant::now();
        let mut c = Collector::new(COLLECT_WINDOW);
        c.push(Action::Compress(files(&["a.mp4"])), t0);
        assert_eq!(c.deadline(), Some(t0 + ms(400)));
        let first = c.take_due(t0 + ms(401)).unwrap();
        assert_eq!(first.jobs, vec![(JobKind::Compress, files(&["a.mp4"]))]);

        c.push(Action::Compress(files(&["b.mp4"])), t0 + ms(900));
        assert_eq!(c.take_due(t0 + ms(1200)), None);
        let second = c.take_due(t0 + ms(1300)).unwrap();
        assert_eq!(second.jobs, vec![(JobKind::Compress, files(&["b.mp4"]))]);
    }

    #[test]
    fn convert_targets_are_separate_kinds() {
        let t0 = Instant::now();
        let mut c = Collector::new(COLLECT_WINDOW);
        c.push(Action::Convert(Target::Mp4, files(&["a.mkv"])), t0);
        c.push(Action::Convert(Target::Mp3, files(&["b.mkv"])), t0 + ms(10));
        c.push(Action::Convert(Target::Mp4, files(&["c.mkv"])), t0 + ms(20));
        let batch = c.take_due(t0 + ms(420)).unwrap();
        assert_eq!(
            batch.jobs,
            vec![
                (JobKind::Convert(Target::Mp4), files(&["a.mkv", "c.mkv"])),
                (JobKind::Convert(Target::Mp3), files(&["b.mkv"])),
            ]
        );
    }

    #[test]
    fn trims_are_collected_apart_from_jobs() {
        let t0 = Instant::now();
        let mut c = Collector::new(COLLECT_WINDOW);
        c.push(Action::Trim(files(&["a.mp4"])), t0);
        c.push(Action::Trim(files(&["b.mp4"])), t0 + ms(100));
        let batch = c.take_due(t0 + ms(500)).unwrap();
        assert!(batch.jobs.is_empty());
        assert_eq!(batch.trims, files(&["a.mp4", "b.mp4"]));
    }

    #[test]
    fn register_never_opens_a_batch() {
        let t0 = Instant::now();
        let mut c = Collector::new(COLLECT_WINDOW);
        c.push(Action::Register, t0);
        c.push(Action::Unregister, t0);
        assert!(c.is_empty());
        assert_eq!(c.take_due(t0 + ms(1000)), None);
    }
}

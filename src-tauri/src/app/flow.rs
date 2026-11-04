//! The dispatcher's decisions, free of Tauri and threads so they can be unit-tested: when to open
//! the progress and editor windows, where each job event goes, buffering events until a window
//! listens, deferred discards, and when the app may exit.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::collector::{Collector, COLLECT_WINDOW};
use super::events::EditorEvent;
use super::replay::{job_id, Replay};
use crate::cli::Action;
use crate::jobs::queue::{JobEvent, JobKind, CANCELLED};

/// Something the dispatcher must do, decided by [`Flow`].
#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    /// Open the progress window.
    OpenWindow {
        kind: &'static str,
        ext: Option<&'static str>,
    },
    /// Send to the progress window (only produced once it listens).
    Emit(JobEvent),
    /// Open the editor window for these files.
    OpenEditor(Vec<PathBuf>),
    /// Send to the editor window (only produced once it listens).
    EmitEditor(EditorEvent),
    Enqueue(JobKind, Vec<PathBuf>),
    /// Answer a pending low-resolution question with "no".
    Decline(u64),
    /// Send this file to the Recycle Bin (a deferred discard); the result comes back to
    /// [`Flow::on_recycled`].
    Recycle(String),
    Exit,
}

/// An export the flow is following: its input, and which editor window asked for it.
struct ExportInfo {
    input: String,
    editor: u64,
}

/// A clip discarded while exports of it were still to finish.
struct PendingDiscard {
    path: String,
    /// The exports still to end `Done`.
    waiting: HashSet<u64>,
    /// Which editor window asked for it.
    editor: u64,
    /// Handed to the dispatcher to recycle; waiting for [`Flow::on_recycled`].
    recycling: bool,
}

/// The dispatcher's decisions, free of Tauri and threads so they can be unit-tested.
///
/// `settled` arguments mean: the queue is idle and every event it emitted has been passed in.
pub(crate) struct Flow {
    collector: Collector,
    replay: Replay,
    /// `kind` (and `ext`) of the latest batch, for the window's URL. A `Started` that carries
    /// its own `kind`/`ext` overrides it in the window.
    hint: (&'static str, Option<&'static str>),
    window_open: bool,
    /// The window's listener is attached; until then events wait in `buffer`.
    ready: bool,
    buffer: Vec<JobEvent>,
    /// The editor window exists, or is being created.
    editor_open: bool,
    /// The editor's listener is attached; until then its events wait in `editor_buffer`.
    editor_ready: bool,
    editor_buffer: Vec<EditorEvent>,
    /// Counts editor windows opened, so a later window never hears of an earlier one's exports.
    editor_count: u64,
    /// Exports not yet finished, by job id. Their events go to the editor, never the progress
    /// window (the queue sends no `Queued` snapshots for them, so every export event has its id).
    exports: HashMap<u64, ExportInfo>,
    /// Exports that ended, by job id: whether they ended `Done`. A discard may name one that
    /// ended before the discard arrived.
    finished: HashMap<u64, bool>,
    /// Deferred discards not yet resolved. The app doesn't exit while there are any.
    discards: Vec<PendingDiscard>,
    /// Ids for discard outcomes shown in the progress window, counting down from
    /// [`FIRST_NOTICE_ID`], well clear of the queue's job ids.
    next_notice_id: u64,
}

/// The first synthetic notice id: 2^53 − 1, the largest integer a JS number holds exactly, so the
/// page can tell neighbouring ids apart.
const FIRST_NOTICE_ID: u64 = (1 << 53) - 1;

impl Flow {
    pub(crate) fn new() -> Self {
        Self {
            collector: Collector::new(COLLECT_WINDOW),
            replay: Replay::default(),
            hint: ("compress", None),
            window_open: false,
            ready: false,
            buffer: Vec::new(),
            editor_open: false,
            editor_ready: false,
            editor_buffer: Vec::new(),
            editor_count: 0,
            exports: HashMap::new(),
            finished: HashMap::new(),
            discards: Vec::new(),
            next_notice_id: FIRST_NOTICE_ID,
        }
    }

    /// When the open batch closes, if one is open.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.collector.deadline()
    }

    pub(crate) fn on_action(&mut self, action: Action, now: Instant) {
        self.collector.push(action, now);
    }

    /// Start the collected batch once it's due: enqueue its jobs and hand its Trim files to the
    /// editor, opening it if needed.
    pub(crate) fn on_tick(&mut self, now: Instant) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(batch) = self.collector.take_due(now) else {
            return out;
        };
        if let Some((kind, _)) = batch.jobs.first() {
            self.hint = (kind.name(), kind.ext());
        }
        for (kind, files) in batch.jobs.into_iter().filter(|(_, files)| !files.is_empty()) {
            out.push(Effect::Enqueue(kind, files));
        }
        if batch.trims.is_empty() {
            return out;
        }
        if self.editor_open {
            let paths = batch.trims.iter().map(|f| f.display().to_string()).collect();
            self.send_editor(EditorEvent::Files { paths }, &mut out);
        } else {
            out.push(Effect::OpenEditor(batch.trims));
            self.editor_open = true;
            self.editor_ready = false;
            self.editor_buffer.clear();
            self.editor_count += 1;
        }
        out
    }

    /// The dispatcher enqueued export `id` of `input` for the current editor window.
    pub(crate) fn on_export_enqueued(&mut self, id: u64, input: String) {
        self.exports.insert(id, ExportInfo { input, editor: self.editor_count });
    }

    /// The current editor discarded `path`, to be recycled once every export in `after` ends
    /// `Done`. One that already ended otherwise (or that this app never ran) keeps the clip now.
    pub(crate) fn on_discard(&mut self, path: String, after: Vec<u64>) -> Vec<Effect> {
        let mut out = Vec::new();
        let mut waiting = HashSet::new();
        let mut doomed = false;
        for id in after {
            if self.exports.contains_key(&id) {
                waiting.insert(id);
            } else if self.finished.get(&id) != Some(&true) {
                doomed = true;
            }
        }
        let mut discard = PendingDiscard { path, waiting, editor: self.editor_count, recycling: false };
        if doomed {
            let event = EditorEvent::DiscardKept { message: kept_message(&discard.path), path: discard.path.clone() };
            self.report_discard(&discard, event, &mut out);
        } else {
            if discard.waiting.is_empty() {
                discard.recycling = true;
                out.push(Effect::Recycle(discard.path.clone()));
            }
            self.discards.push(discard);
        }
        out
    }

    /// A deferred discard's recycle finished.
    pub(crate) fn on_recycled(&mut self, path: String, result: Result<(), String>, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        if let Some(i) = self.discards.iter().position(|d| d.recycling && d.path == path) {
            let discard = self.discards.remove(i);
            let event = match result {
                Ok(()) => EditorEvent::DiscardDone { path },
                Err(message) => EditorEvent::DiscardKept { path, message },
            };
            self.report_discard(&discard, event, &mut out);
        }
        self.exit_if_finished(settled, &mut out);
        out
    }

    /// Export `id` ended (`ok`: it ended `Done`): move on the discards waiting for it.
    fn on_export_finished(&mut self, id: u64, ok: bool, out: &mut Vec<Effect>) {
        self.finished.insert(id, ok);
        let mut i = 0;
        while i < self.discards.len() {
            let d = &mut self.discards[i];
            if d.recycling || !d.waiting.remove(&id) {
                i += 1;
            } else if !ok {
                let discard = self.discards.remove(i);
                let event =
                    EditorEvent::DiscardKept { message: kept_message(&discard.path), path: discard.path.clone() };
                self.report_discard(&discard, event, out);
            } else {
                if d.waiting.is_empty() {
                    d.recycling = true;
                    out.push(Effect::Recycle(d.path.clone()));
                }
                i += 1;
            }
        }
    }

    /// Tell the editor that asked; with it gone, a kept clip shows in the progress window (like a
    /// late export failure) and a recycled one needs no word.
    fn report_discard(&mut self, discard: &PendingDiscard, event: EditorEvent, out: &mut Vec<Effect>) {
        if self.editor_open && discard.editor == self.editor_count {
            self.send_editor(event, out);
        } else if let EditorEvent::DiscardKept { path, message } = event {
            let id = self.next_notice_id;
            self.next_notice_id -= 1;
            self.show_export_failure(id, path, message, true, out);
        }
    }

    pub(crate) fn on_job_event(&mut self, event: JobEvent, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        let drained = event == JobEvent::Drained;
        match job_id(&event).filter(|id| self.exports.contains_key(id)) {
            Some(id) => self.on_export_event(id, event, &mut out),
            None => self.on_progress_event(event, &mut out),
        }
        if drained && settled {
            self.exit_if_finished(settled, &mut out);
        }
        out
    }

    /// A compress or convert job's event, or one about the whole queue.
    fn on_progress_event(&mut self, event: JobEvent, out: &mut Vec<Effect>) {
        let catch_up = self.replay.events();
        self.replay.apply(&event);
        if self.window_open {
            self.send(event, out);
        } else if reopens_window(&event) {
            // The new window first learns which job is running, then gets this event.
            self.open_window(self.hint, out);
            self.buffer = catch_up;
            self.send(event, out);
        }
    }

    /// Route an export's event to the editor that asked for it. With that editor gone, only a
    /// failure, or a saved export with a note (its 20 MB copy wasn't made), is shown, in the
    /// progress window.
    fn on_export_event(&mut self, id: u64, event: JobEvent, out: &mut Vec<Effect>) {
        let info = &self.exports[&id];
        let to_editor = self.editor_open && info.editor == self.editor_count;
        let input = info.input.clone();
        let failure = |message: String, original_intact: bool| EditorEvent::ExportFailed {
            id,
            input: input.clone(),
            message,
            original_intact,
        };
        let editor_event = match event {
            JobEvent::Started { .. } => EditorEvent::ExportStarted { id, input: input.clone() },
            JobEvent::Progress { pass, passes, fraction, .. } => {
                let overall = (f64::from(pass.saturating_sub(1)) + fraction) / f64::from(passes.max(1));
                EditorEvent::ExportProgress { id, fraction: overall }
            }
            // Exports never ask; don't leave the queue waiting on an answer nobody can give.
            JobEvent::NeedsConfirm { .. } => {
                out.push(Effect::Decline(id));
                return;
            }
            JobEvent::Done { output, bytes, note, .. } => {
                self.exports.remove(&id);
                EditorEvent::ExportDone { id, input: input.clone(), output, bytes, note }
            }
            JobEvent::Unchanged { reason, .. } => {
                self.exports.remove(&id);
                failure(reason, true)
            }
            JobEvent::Failed { message, original_intact, .. } => {
                self.exports.remove(&id);
                failure(message, original_intact)
            }
            JobEvent::Queued { .. } | JobEvent::Drained => return,
        };
        let ended = match &editor_event {
            EditorEvent::ExportDone { .. } => Some(true),
            EditorEvent::ExportFailed { .. } => Some(false),
            _ => None,
        };
        if to_editor {
            self.send_editor(editor_event, out);
        } else {
            match editor_event {
                EditorEvent::ExportFailed { message, original_intact, .. } if message != CANCELLED => {
                    self.show_export_failure(id, input, message, original_intact, out);
                }
                // Saved, but something else wasn't: a notice, not a failure.
                EditorEvent::ExportDone { output, bytes, note: Some(note), .. } => {
                    let done = JobEvent::Done { id, output, bytes, note: Some(note) };
                    self.show_export_outcome(id, input, done, out);
                }
                _ => {}
            }
        }
        // After the export's own outcome, so the editor hears of it first.
        if let Some(ok) = ended {
            self.on_export_finished(id, ok, out);
        }
    }

    /// Show a failed export in the progress window, opening it if needed.
    fn show_export_failure(
        &mut self,
        id: u64,
        input: String,
        message: String,
        original_intact: bool,
        out: &mut Vec<Effect>,
    ) {
        self.show_export_outcome(id, input, JobEvent::Failed { id, message, original_intact }, out);
    }

    /// Show how an export ended (`outcome`, its `Failed` or `Done` event) in the progress window,
    /// opening it if needed.
    fn show_export_outcome(&mut self, id: u64, input: String, outcome: JobEvent, out: &mut Vec<Effect>) {
        if !self.window_open {
            self.open_window(("export", None), out);
        }
        let started = JobEvent::Started {
            id,
            input,
            input_bytes: 0, // not shown with a failure or a note
            kind: Some("export".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        };
        self.send(started, out);
        self.send(outcome, out);
    }

    /// The window's listener is attached.
    pub(crate) fn on_ready(&mut self) -> Vec<Effect> {
        if !self.window_open {
            return Vec::new();
        }
        if self.ready {
            // The page reloaded and lost its state; bring it back up to date.
            return self.replay.events().into_iter().map(Effect::Emit).collect();
        }
        self.ready = true;
        self.buffer.drain(..).map(Effect::Emit).collect()
    }

    pub(crate) fn on_window_closed(&mut self, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        self.forget_window();
        // Nothing could answer the question any more: closing the window means "no".
        if let Some(id) = self.replay.awaiting_confirm() {
            out.push(Effect::Decline(id));
        }
        self.exit_if_finished(settled, &mut out);
        out
    }

    /// An `OpenWindow` effect could not be carried out; the next event tries again. If nothing
    /// else is coming (the window was for the final drain), exit instead of waiting forever with
    /// no window.
    pub(crate) fn on_open_failed(&mut self, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        self.forget_window();
        self.exit_if_finished(settled, &mut out);
        out
    }

    /// The editor's listener is attached. A reloaded editor gets nothing again: it asks for its
    /// files itself, and its earlier exports' progress is not replayed.
    pub(crate) fn on_editor_ready(&mut self) -> Vec<Effect> {
        if !self.editor_open || self.editor_ready {
            return Vec::new();
        }
        self.editor_ready = true;
        self.editor_buffer.drain(..).map(Effect::EmitEditor).collect()
    }

    /// An event from an editor command, for the editor window.
    pub(crate) fn on_editor_event(&mut self, event: EditorEvent) -> Vec<Effect> {
        let mut out = Vec::new();
        if self.editor_open {
            self.send_editor(event, &mut out);
        }
        out
    }

    /// The editor window closed, or could not be opened. Its exports keep running.
    pub(crate) fn on_editor_closed(&mut self, settled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        self.editor_open = false;
        self.editor_ready = false;
        self.editor_buffer.clear();
        self.exit_if_finished(settled, &mut out);
        out
    }

    fn forget_window(&mut self) {
        self.window_open = false;
        self.ready = false;
        self.buffer.clear();
    }

    fn open_window(&mut self, (kind, ext): (&'static str, Option<&'static str>), out: &mut Vec<Effect>) {
        out.push(Effect::OpenWindow { kind, ext });
        self.window_open = true;
        self.ready = false;
        self.buffer.clear();
    }

    fn send(&mut self, event: JobEvent, out: &mut Vec<Effect>) {
        if self.ready {
            out.push(Effect::Emit(event));
        } else {
            self.buffer.push(event);
        }
    }

    fn send_editor(&mut self, event: EditorEvent, out: &mut Vec<Effect>) {
        if self.editor_ready {
            out.push(Effect::EmitEditor(event));
        } else {
            self.editor_buffer.push(event);
        }
    }

    fn exit_if_finished(&self, settled: bool, out: &mut Vec<Effect>) {
        if settled && !self.window_open && !self.editor_open && self.collector.is_empty() && self.discards.is_empty() {
            out.push(Effect::Exit);
        }
    }
}

/// Whether `event` brings back a window the user closed. Progress ticks would bring it back at
/// once, `Drained` alone has nothing to show, and the user already knows about their own cancel.
fn reopens_window(event: &JobEvent) -> bool {
    match event {
        JobEvent::Progress { .. } | JobEvent::Drained => false,
        JobEvent::Failed { message, .. } => message != CANCELLED,
        _ => true,
    }
}

/// Why a deferred discard left the clip in place.
fn kept_message(path: &str) -> String {
    let name = Path::new(path).file_name().map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned());
    format!("Kept \"{name}\": its export didn't finish, so it wasn't sent to the Recycle Bin.")
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::app::commands::DISCARD_FAILED;
    use crate::cli::Target;
    use crate::editor::request::ExportRequest;
    use crate::jobs::queue::{ExportJob, JobQueue, JobRunner, JobSpec, QueueSnapshot};
    use crate::jobs::{JobError, JobOutcome, JobProgress};
    use crate::media::run::CancelToken;

    fn files(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

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

    fn done(id: u64) -> JobEvent {
        JobEvent::Done { id, output: format!("{id}.out.mp4"), bytes: 2, note: None }
    }

    fn emits(events: &[JobEvent]) -> Vec<Effect> {
        events.iter().cloned().map(Effect::Emit).collect()
    }

    const OPEN_COMPRESS: Effect = Effect::OpenWindow { kind: "compress", ext: None };
    const OPEN_EXPORT: Effect = Effect::OpenWindow { kind: "export", ext: None };

    /// A flow whose window is open and listening, showing job 1 of 2.
    fn showing_job_1() -> Flow {
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 2), false), vec![OPEN_COMPRESS]);
        f.on_job_event(started(1), false);
        f.on_ready();
        f
    }

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn to_editor(events: &[EditorEvent]) -> Vec<Effect> {
        events.iter().cloned().map(Effect::EmitEditor).collect()
    }

    /// A flow whose editor window is open and listening, opened for `t.mp4`.
    fn editing() -> Flow {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["t.mp4"])), t0);
        assert_eq!(f.on_tick(t0 + ms(400)), vec![Effect::OpenEditor(files(&["t.mp4"]))]);
        assert_eq!(f.on_editor_ready(), vec![]);
        f
    }

    /// `editing()` with export 5 of `t.mp4` enqueued and started, as the queue reports it (pinned
    /// by `jobs::queue::tests::exports_emit_no_snapshots`).
    fn exporting() -> Flow {
        let mut f = editing();
        f.on_export_enqueued(5, "t.mp4".into());
        // The queue's real sequence for an export: no `Queued`, straight to `Started`.
        assert_eq!(f.on_job_event(export_started(5), false).len(), 1);
        f
    }

    fn export_started(id: u64) -> JobEvent {
        JobEvent::Started {
            id,
            input: "t.mp4".into(),
            input_bytes: 1,
            kind: Some("export".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        }
    }

    #[test]
    fn a_batch_enqueues_each_kind_and_opens_the_window_on_its_first_event() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Convert(Target::Mov, files(&["a.mkv"])), t0);
        f.on_action(Action::Compress(files(&["b.mp4"])), t0 + ms(100));
        assert_eq!(f.deadline(), Some(t0 + ms(500)));
        assert_eq!(f.on_tick(t0 + ms(499)), vec![]);
        assert_eq!(
            f.on_tick(t0 + ms(500)),
            vec![
                Effect::Enqueue(JobKind::Convert(Target::Mov), files(&["a.mkv"])),
                Effect::Enqueue(JobKind::Compress, files(&["b.mp4"])),
            ]
        );
        assert_eq!(f.on_job_event(queued(1, 2), false), vec![Effect::OpenWindow { kind: "convert", ext: Some("mov") }]);
    }

    #[test]
    fn events_wait_for_the_listener_then_flush_in_order() {
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![OPEN_COMPRESS]);
        assert_eq!(f.on_job_event(started(1), false), vec![]);
        assert_eq!(f.on_job_event(failed(1, "boom"), false), vec![]);
        assert_eq!(f.on_ready(), emits(&[queued(1, 1), started(1), failed(1, "boom")]));
        assert_eq!(f.on_job_event(JobEvent::Drained, true), emits(&[JobEvent::Drained]));
    }

    #[test]
    fn a_reloaded_page_gets_the_running_job_again() {
        let mut f = showing_job_1();
        assert_eq!(f.on_ready(), emits(&[queued(1, 2), started(1)]));
    }

    #[test]
    fn closing_mid_job_keeps_the_queue_and_reopens_on_the_next_real_event() {
        let mut f = showing_job_1();
        assert_eq!(f.on_window_closed(false), vec![], "no exit while jobs remain");
        assert_eq!(f.on_job_event(progress(1), false), vec![], "progress ticks don't reopen");
        assert_eq!(f.on_job_event(done(1), false), vec![OPEN_COMPRESS]);
        // The new window learns which job ran before it hears how it ended.
        assert_eq!(f.on_ready(), emits(&[queued(1, 2), started(1), done(1)]));
    }

    #[test]
    fn closing_during_confirm_declines_and_then_exits() {
        let mut f = showing_job_1();
        let ask = JobEvent::NeedsConfirm { id: 1, width: 426, height: 240, fps: 15.0 };
        assert_eq!(f.on_job_event(ask.clone(), false), emits(&[ask]));
        assert_eq!(f.on_window_closed(false), vec![Effect::Decline(1)]);
        // The resulting cancel doesn't bring the window back; the drain ends the process.
        assert_eq!(f.on_job_event(failed(1, CANCELLED), false), vec![]);
        assert_eq!(f.on_job_event(JobEvent::Drained, true), vec![Effect::Exit]);
    }

    #[test]
    fn exit_waits_until_the_last_event_is_handled() {
        let mut f = showing_job_1();
        f.on_window_closed(false);
        assert_eq!(f.on_job_event(JobEvent::Drained, false), vec![], "not settled yet");
        assert_eq!(f.on_job_event(JobEvent::Drained, true), vec![Effect::Exit]);
    }

    #[test]
    fn no_exit_while_the_window_is_open() {
        let mut f = showing_job_1();
        assert_eq!(f.on_job_event(JobEvent::Drained, true), emits(&[JobEvent::Drained]));
        assert_eq!(f.on_window_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn no_exit_while_a_batch_is_being_collected() {
        let t0 = Instant::now();
        let mut f = showing_job_1();
        f.on_job_event(JobEvent::Drained, true);
        f.on_action(Action::Compress(files(&["c.mp4"])), t0);
        assert_eq!(f.on_window_closed(true), vec![]);
    }

    #[test]
    fn a_failed_window_open_is_retried_on_the_next_event() {
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![OPEN_COMPRESS]);
        assert_eq!(f.on_open_failed(false), vec![], "jobs remain, so no exit");
        assert_eq!(f.on_job_event(started(1), false), vec![OPEN_COMPRESS]);
        assert_eq!(f.on_ready(), emits(&[queued(1, 1), started(1)]));
    }

    #[test]
    fn a_failed_window_open_with_nothing_left_exits() {
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![OPEN_COMPRESS]);
        f.on_job_event(failed(1, "boom"), false);
        f.on_job_event(JobEvent::Drained, true);
        assert_eq!(f.on_open_failed(true), vec![Effect::Exit]);
    }

    #[test]
    fn a_failed_window_open_waits_while_a_batch_is_collected() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        assert_eq!(f.on_job_event(queued(1, 1), false), vec![OPEN_COMPRESS]);
        f.on_action(Action::Compress(files(&["a.mp4"])), t0);
        assert_eq!(f.on_open_failed(true), vec![]);
    }

    // ---- Editor ----

    /// A real [`JobQueue`] whose jobs wait for [`RealQueue::release`], then report one progress
    /// tick (pass 1 of 1, half done) and finish (they fail when the input's name starts with "fail",
    /// and are done with a note when it starts with "note"). Its events are collected in emission order, so
    /// a Flow can be fed exactly what the queue sends.
    struct RealQueue {
        queue: JobQueue,
        rx: Receiver<JobEvent>,
        gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
    }

    struct GatedRunner(Arc<(Mutex<bool>, std::sync::Condvar)>);

    impl JobRunner for GatedRunner {
        fn run(
            &self,
            spec: &JobSpec,
            _: bool,
            cancel: &CancelToken,
            progress: &mut dyn FnMut(JobProgress),
        ) -> Result<JobOutcome, JobError> {
            let (open, cv) = &*self.0;
            let mut is_open = open.lock().unwrap();
            while !*is_open {
                if cancel.is_cancelled() {
                    return Err(JobError::Cancelled);
                }
                is_open = cv.wait_timeout(is_open, ms(10)).unwrap().0;
            }
            progress(JobProgress { pass: 1, passes: 1, fraction: 0.5 });
            let name = spec.input.to_string_lossy();
            if name.starts_with("fail") {
                return Err(JobError::Message("Couldn't export this clip.".into()));
            }
            let note = name.starts_with("note").then(|| "Saved, but no 20 MB copy.".to_owned());
            Ok(JobOutcome::Done { output: spec.input.with_extension("out.mp4"), bytes: 2, note })
        }
    }

    impl RealQueue {
        fn new() -> Self {
            let (tx, rx) = mpsc::channel();
            let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
            let queue = JobQueue::new(
                Box::new(GatedRunner(gate.clone())),
                Box::new(move |event| {
                    let _ = tx.send(event);
                }),
            );
            Self { queue, rx, gate }
        }

        fn release(&self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }

        /// Every event not taken yet, up to and including the first that matches `end`.
        fn until(&self, end: impl Fn(&JobEvent) -> bool) -> Vec<JobEvent> {
            let mut events = Vec::new();
            loop {
                let event = self.rx.recv_timeout(Duration::from_secs(5)).expect("the queue went quiet");
                let last = end(&event);
                events.push(event);
                if last {
                    return events;
                }
            }
        }

        fn drained(&self) -> Vec<JobEvent> {
            self.release();
            self.until(|e| *e == JobEvent::Drained)
        }
    }

    /// Feed `events` to `f` as the dispatcher would: settled only at the final `Drained`.
    fn feed(f: &mut Flow, events: Vec<JobEvent>) -> Vec<Effect> {
        let n = events.len();
        let mut out = Vec::new();
        for (i, event) in events.into_iter().enumerate() {
            let settled = i + 1 == n && event == JobEvent::Drained;
            out.extend(f.on_job_event(event, settled));
        }
        out
    }

    fn export_job(input: &str) -> JobKind {
        JobKind::Export(ExportJob(Arc::new(ExportRequest {
            session: 1,
            input: PathBuf::from(input),
            mode: crate::editor::request::ExportMode::Quick,
            fit_20mb: false,
            duration_s: 2.0,
            video: Vec::new(),
            audio: Vec::new(),
            master_db: 0.0,
            crop: None,
        })))
    }

    #[test]
    fn an_export_with_no_batch_running_never_opens_the_progress_window() {
        let mut f = editing();
        let rq = RealQueue::new();
        let id = rq.queue.enqueue_one(export_job("t.mp4"), PathBuf::from("t.mp4"));
        f.on_export_enqueued(id, "t.mp4".into());
        let effects = feed(&mut f, rq.drained());
        assert_eq!(
            effects,
            to_editor(&[
                EditorEvent::ExportStarted { id, input: "t.mp4".into() },
                EditorEvent::ExportProgress { id, fraction: 0.5 },
                EditorEvent::ExportDone { id, input: "t.mp4".into(), output: "t.out.mp4".into(), bytes: 2, note: None },
            ])
        );
        assert!(f.on_editor_closed(true).contains(&Effect::Exit));
    }

    #[test]
    fn a_batch_running_beside_exports_shows_only_its_own_files() {
        let t0 = Instant::now();
        let mut f = editing();
        let rq = RealQueue::new();
        f.on_action(Action::Compress(files(&["a.mp4", "b.mp4"])), t0);
        for effect in f.on_tick(t0 + ms(400)) {
            let Effect::Enqueue(kind, files) = effect else { panic!("{effect:?}") };
            rq.queue.enqueue(kind, files);
        }
        let id = rq.queue.enqueue_one(export_job("t.mp4"), PathBuf::from("t.mp4"));
        f.on_export_enqueued(id, "t.mp4".into());
        let mut effects = feed(&mut f, rq.drained());
        effects.extend(f.on_ready());

        let opened: Vec<_> = effects.iter().filter(|e| matches!(e, Effect::OpenWindow { .. })).collect();
        assert_eq!(opened, vec![&OPEN_COMPRESS]);
        for effect in &effects {
            match effect {
                // "File N of 2": the export isn't one of the window's files.
                Effect::Emit(JobEvent::Queued { snapshot }) => assert_eq!(snapshot.total, 2, "{effects:?}"),
                Effect::Emit(event) => assert_ne!(job_id(event), Some(id), "{event:?} reached the progress window"),
                _ => {}
            }
        }
        assert!(effects.contains(&Effect::EmitEditor(EditorEvent::ExportStarted { id, input: "t.mp4".into() })));
    }

    #[test]
    fn a_trim_batch_opens_the_editor() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["a.mp4"])), t0);
        f.on_action(Action::Compress(files(&["c.mp4"])), t0 + ms(10));
        f.on_action(Action::Trim(files(&["b.mp4"])), t0 + ms(20));
        assert_eq!(
            f.on_tick(t0 + ms(420)),
            vec![Effect::Enqueue(JobKind::Compress, files(&["c.mp4"])), Effect::OpenEditor(files(&["a.mp4", "b.mp4"])),]
        );
    }

    #[test]
    fn a_trim_while_the_editor_is_open_appends_files() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["a.mp4"])), t0);
        f.on_tick(t0 + ms(400));
        // The page isn't listening yet: the new files wait for it.
        f.on_action(Action::Trim(files(&["b.mp4", "c.mp4"])), t0 + ms(500));
        assert_eq!(f.on_tick(t0 + ms(900)), vec![]);
        let appended = EditorEvent::Files { paths: strings(&["b.mp4", "c.mp4"]) };
        assert_eq!(f.on_editor_ready(), to_editor(&[appended]));
        // Once it listens, they go straight to it.
        f.on_action(Action::Trim(files(&["d.mp4"])), t0 + ms(1000));
        assert_eq!(f.on_tick(t0 + ms(1400)), to_editor(&[EditorEvent::Files { paths: strings(&["d.mp4"]) }]));
    }

    #[test]
    fn export_events_go_to_the_editor_not_the_progress_window() {
        let mut f = editing();
        f.on_export_enqueued(5, "t.mp4".into());
        assert_eq!(
            f.on_job_event(export_started(5), false),
            to_editor(&[EditorEvent::ExportStarted { id: 5, input: "t.mp4".into() }])
        );
        // Pass 2 of 3, half done: halfway through the whole export.
        let tick = JobEvent::Progress { id: 5, pass: 2, passes: 3, fraction: 0.5, eta_s: None };
        assert_eq!(f.on_job_event(tick, false), to_editor(&[EditorEvent::ExportProgress { id: 5, fraction: 0.5 }]));
        assert_eq!(
            f.on_job_event(done(5), false),
            to_editor(&[EditorEvent::ExportDone {
                id: 5,
                input: "t.mp4".into(),
                output: "5.out.mp4".into(),
                bytes: 2,
                note: None
            }])
        );
        let mut f = exporting();
        let noted = JobEvent::Done { id: 5, output: "o.mp4".into(), bytes: 2, note: Some("n".into()) };
        assert_eq!(
            f.on_job_event(noted, false),
            to_editor(&[EditorEvent::ExportDone {
                id: 5,
                input: "t.mp4".into(),
                output: "o.mp4".into(),
                bytes: 2,
                note: Some("n".into())
            }])
        );
        assert_eq!(f.on_job_event(JobEvent::Drained, true), vec![]);
    }

    #[test]
    fn an_export_failure_goes_to_the_editor() {
        let mut f = exporting();
        assert_eq!(
            f.on_job_event(failed(5, "boom"), false),
            to_editor(&[EditorEvent::ExportFailed {
                id: 5,
                input: "t.mp4".into(),
                message: "boom".into(),
                original_intact: true
            }])
        );
    }

    #[test]
    fn export_failure_without_editor_shows_in_progress_window() {
        let mut f = exporting();
        assert_eq!(f.on_editor_closed(false), vec![]);
        assert_eq!(f.on_job_event(progress(5), false), vec![], "nothing shows a running export");
        assert_eq!(f.on_job_event(failed(5, "boom"), false), vec![OPEN_EXPORT]);
        let shown = JobEvent::Started {
            id: 5,
            input: "t.mp4".into(),
            input_bytes: 0,
            kind: Some("export".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        };
        assert_eq!(f.on_ready(), emits(&[shown, failed(5, "boom")]));
        assert_eq!(f.on_job_event(JobEvent::Drained, true), emits(&[JobEvent::Drained]));
        assert_eq!(f.on_window_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn a_noted_export_without_editor_shows_in_progress_window() {
        let mut f = editing();
        let rq = RealQueue::new();
        let id = rq.queue.enqueue_one(export_job("note.mp4"), PathBuf::from("note.mp4"));
        f.on_export_enqueued(id, "note.mp4".into());
        assert_eq!(f.on_editor_closed(false), vec![], "the export is still running");

        let effects = feed(&mut f, rq.drained());
        assert_eq!(effects, vec![OPEN_EXPORT], "only the note opens the window");
        let shown = JobEvent::Started {
            id,
            input: "note.mp4".into(),
            input_bytes: 0,
            kind: Some("export".into()),
            ext: None,
            width: None,
            height: None,
            fps: None,
        };
        // A Done, not a Failed: the export was saved.
        let done = JobEvent::Done {
            id,
            output: "note.out.mp4".into(),
            bytes: 2,
            note: Some("Saved, but no 20 MB copy.".into()),
        };
        assert_eq!(f.on_ready(), emits(&[shown, done, JobEvent::Drained]));
        assert_eq!(f.on_window_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn a_later_editor_hears_nothing_of_an_earlier_ones_exports() {
        let t0 = Instant::now();
        let mut f = exporting();
        f.on_editor_closed(false);
        f.on_action(Action::Trim(files(&["u.mp4"])), t0);
        assert_eq!(f.on_tick(t0 + ms(400)), vec![Effect::OpenEditor(files(&["u.mp4"]))]);
        f.on_editor_ready();
        assert_eq!(f.on_job_event(done(5), false), vec![]);
    }

    #[test]
    fn no_exit_while_the_editor_is_open() {
        let mut f = exporting();
        assert_eq!(f.on_job_event(done(5), false).len(), 1);
        assert_eq!(f.on_job_event(JobEvent::Drained, true), vec![]);
        // A progress window closing doesn't end the app under the editor either.
        f.on_job_event(queued(2, 2), false);
        assert_eq!(f.on_window_closed(true), vec![]);
        assert_eq!(f.on_editor_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn exit_after_editor_closes_and_exports_drain() {
        let mut f = exporting();
        assert_eq!(f.on_editor_closed(false), vec![], "the export is still running");
        assert_eq!(f.on_job_event(done(5), false), vec![]);
        assert_eq!(f.on_job_event(JobEvent::Drained, true), vec![Effect::Exit]);
    }

    #[test]
    fn a_failed_editor_open_with_nothing_left_exits() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["t.mp4"])), t0);
        f.on_tick(t0 + ms(400));
        assert_eq!(f.on_editor_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn proxy_progress_waits_for_the_editor() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.on_action(Action::Trim(files(&["t.mp4"])), t0);
        f.on_tick(t0 + ms(400));
        let tick = EditorEvent::ProxyProgress { session: 1, fraction: 0.25 };
        assert_eq!(f.on_editor_event(tick.clone()), vec![]);
        assert_eq!(f.on_editor_ready(), vec![Effect::EmitEditor(tick.clone())]);
        assert_eq!(f.on_editor_event(tick.clone()), vec![Effect::EmitEditor(tick)]);
    }

    #[test]
    fn an_export_that_asks_to_confirm_is_declined() {
        let mut f = exporting();
        let ask = JobEvent::NeedsConfirm { id: 5, width: 426, height: 240, fps: 15.0 };
        assert_eq!(f.on_job_event(ask, false), vec![Effect::Decline(5)]);
    }

    // ---- Deferred discards ----

    /// `editing()` with an export of `input` enqueued on a real queue, and Discard of `input`
    /// registered to wait for it.
    fn discarding_after_export(input: &str) -> (Flow, RealQueue, u64) {
        let mut f = editing();
        let rq = RealQueue::new();
        let id = rq.queue.enqueue_one(export_job(input), PathBuf::from(input));
        f.on_export_enqueued(id, input.into());
        assert_eq!(f.on_discard(input.into(), vec![id]), vec![], "nothing happens until the export ends");
        (f, rq, id)
    }

    fn kept(path: &str) -> EditorEvent {
        EditorEvent::DiscardKept { path: path.into(), message: kept_message(path) }
    }

    #[test]
    fn a_discard_after_a_done_export_recycles_the_file() {
        let (mut f, rq, _) = discarding_after_export("t.mp4");
        let effects = feed(&mut f, rq.drained());
        assert!(effects.contains(&Effect::Recycle("t.mp4".into())), "{effects:?}");
        assert_eq!(
            f.on_recycled("t.mp4".into(), Ok(()), true),
            to_editor(&[EditorEvent::DiscardDone { path: "t.mp4".into() }])
        );
        assert_eq!(f.on_editor_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn a_discard_after_a_failed_export_keeps_the_file_and_says_so() {
        let (mut f, rq, _) = discarding_after_export("fail.mp4");
        let effects = feed(&mut f, rq.drained());
        assert!(!effects.iter().any(|e| matches!(e, Effect::Recycle(_))), "{effects:?}");
        let failure = effects.iter().position(|e| matches!(e, Effect::EmitEditor(EditorEvent::ExportFailed { .. })));
        let kept_at = effects.iter().position(|e| *e == Effect::EmitEditor(kept("fail.mp4")));
        assert!(failure.is_some() && kept_at > failure, "{effects:?}");
        assert!(kept_message("C:\\v\\fail.mp4").starts_with("Kept \"fail.mp4\": "));
        // Nothing is left pending: closing the editor ends the app.
        assert_eq!(f.on_editor_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn a_discard_still_happens_after_the_editor_closes() {
        let (mut f, rq, _) = discarding_after_export("t.mp4");
        assert_eq!(f.on_editor_closed(false), vec![]);
        let effects = feed(&mut f, rq.drained());
        assert!(effects.contains(&Effect::Recycle("t.mp4".into())), "{effects:?}");
        assert!(!effects.contains(&Effect::Exit), "the app waits for the recycle: {effects:?}");
        // Success needs no window; then the app exits.
        assert_eq!(f.on_recycled("t.mp4".into(), Ok(()), true), vec![Effect::Exit]);
    }

    #[test]
    fn a_kept_file_after_the_editor_closes_shows_in_the_progress_window() {
        let (mut f, rq, _) = discarding_after_export("fail.mp4");
        assert_eq!(f.on_editor_closed(false), vec![]);
        let effects = feed(&mut f, rq.drained());
        assert!(!effects.iter().any(|e| matches!(e, Effect::Recycle(_))), "{effects:?}");
        assert_eq!(effects.iter().filter(|e| **e == OPEN_EXPORT).count(), 1, "{effects:?}");
        let shown = f.on_ready();
        let messages: Vec<_> = shown
            .iter()
            .filter_map(|e| match e {
                Effect::Emit(JobEvent::Failed { message, original_intact, .. }) => {
                    Some((message.clone(), *original_intact))
                }
                _ => None,
            })
            .collect();
        assert_eq!(messages, vec![("Couldn't export this clip.".to_string(), true), (kept_message("fail.mp4"), true)]);
        assert_eq!(f.on_window_closed(true), vec![Effect::Exit]);
    }

    #[test]
    fn a_failed_recycle_after_the_editor_closes_shows_in_the_progress_window() {
        let (mut f, rq, _) = discarding_after_export("t.mp4");
        f.on_editor_closed(false);
        feed(&mut f, rq.drained());
        let effects = f.on_recycled("t.mp4".into(), Err(DISCARD_FAILED.into()), true);
        assert_eq!(effects, vec![OPEN_EXPORT]);
        assert!(f.on_ready().contains(&Effect::Emit(JobEvent::Failed {
            id: 9_007_199_254_740_991, // Number.MAX_SAFE_INTEGER
            message: DISCARD_FAILED.into(),
            original_intact: true
        })));
    }

    #[test]
    fn a_discard_of_exports_that_already_ended_resolves_at_once() {
        let mut f = exporting();
        f.on_job_event(done(5), false);
        assert_eq!(f.on_discard("t.mp4".into(), vec![5]), vec![Effect::Recycle("t.mp4".into())]);

        let mut f = exporting();
        f.on_job_event(failed(5, "boom"), false);
        assert_eq!(f.on_discard("t.mp4".into(), vec![5]), to_editor(&[kept("t.mp4")]));
        // An export this app never ran can't vouch for anything either.
        assert_eq!(f.on_discard("u.mp4".into(), vec![99]), to_editor(&[kept("u.mp4")]));
    }
}

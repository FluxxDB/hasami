# Hasami Plan 2: Quick Trim Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Right-click → **Hasami ▸ Quick Trim** opens a DaVinci-like trim editor. You cut the clip, set per-track volume and mute, and export as `[CLIPPED] <name>` (replacing the raw, keeping it in `Raw\`, or as an audio-only `.mp3`). Several selected clips form a trim queue with Skip and Discard.

**Architecture:**
- **Rust backend (on top of Plan 1):**
  - prepares editor media per session: empty-track detection, per-track audio extraction, waveform peaks, thumbnails, and an optional H.264 preview proxy;
  - renders a timeline-derived `ExportRequest` with an ffmpeg `-filter_complex`;
  - runs exports as background jobs in Plan 1's serial queue.
- **Svelte editor window:**
  - holds the timeline in a pure, unit-tested TS model;
  - plays it with a muted `<video>` plus one `<audio>` per track through Web Audio gain nodes;
  - talks to Rust through commands and one `hasami://editor` event.

**Tech Stack:** Rust (Tauri 2.11, the Plan 1 modules), the bundled ffmpeg/ffprobe, Svelte 5 runes + TypeScript, Vitest, the Tauri asset protocol.

**Spec:** `docs/superpowers/specs/2026-10-30-hasami-design.md`. Read §5 (whole), §6, §7, §8 and §9 first. Plan 1 (`docs/superpowers/plans/2026-10-30-hasami-foundation-compress-convert.md`) built the modules reused here.

## Global Constraints

- Windows only. Build and test with the **Windows** toolchain from `C:\Users\ledan\Documents\Projects\hasami`. From WSL:
  - Rust: `CARGO_TARGET_DIR='C:\Users\ledan\.cache\hasami-target' WSLENV=CARGO_TARGET_DIR cargo.exe …` in `src-tauri/`.
  - Node: `cmd.exe /c npm …` at the repo root.
- Toolchain pins from Plan 1:
  - rustc 1.89 with `src-tauri/.cargo/config.toml` (`incompatible-rust-versions = "fallback"`). Never run `cargo update`.
  - tauri 2.11.x; Node 20.18 with Vite 6 and Vitest 3.
- Every child process goes through `media::ffmpeg::hidden_command` (`CREATE_NO_WINDOW`). Arguments are passed as `OsString` args, never shell strings.
- **File safety (spec §8)** for every export:
  - Write to `temp_path_for(final)` and verify (duration within tolerance; size ≤ limit when one applies).
  - Commit with a non-replacing rename. Only after that recycle the original (Quick export) or move it into `Raw\` (Save raw).
  - On failure, the original is untouched and the temp file is deleted, unless `kept_new_file` says otherwise.
- **Naming (spec §5.7):**
  - Output is `[CLIPPED] <stem>.<ext>` beside the original.
  - The prefix is never doubled.
  - A collision appends ` (2)`, ` (3)`… via `fsops::unique_path`.
  - If the source is in a folder named `Raw` (case-insensitive), outputs go to that folder's parent, and Save raw leaves the original where it is.
- **Video exports (spec §5.7):**
  - Same video codec, resolution and fps as the source, at the source's average video bitrate.
  - Hardware encoder when available (AMF on this machine), otherwise software.
  - Source container kept.
  - Every unmuted audio track mixed at its volume into **one** stereo track: `amix=inputs=N:normalize=0`.
  - Everything after the video's end is dropped, including in **Export audio**.
- **Fit to 20 MB:** one toggle that applies to both video export buttons. Output is H.264/AAC `.mp4` at **≤ 20,000,000 bytes** using Plan 1's compress planner (`plan_compress`) and the two-pass encode.
- **Exports run in the background (spec §5.7):**
  - Clicking a video export enqueues a job and immediately closes the window, or loads the next queued clip.
  - Originals are recycled or moved only after their export verifies.
- **Timeline (spec §5.3):**
  - **Video:** exactly one video track, always contiguous; deleting ripples.
  - **Audio:** audio deletes leave silence; audio past the video's end is hatched.
  - **Ctrl+B** cuts every track.
  - **Linked selection** is on by default and can be toggled.
  - **Snapping** to the playhead, cut points and segment edges on any track, and the video end. No grid.
  - **Undo/redo** covers every edit, including volume and mute.
- Keyboard (spec §5.5, DaVinci): Space; J/K/L (taps go 1× → 2× → 4× → 8×); ←/→ one frame; Shift+←/→ one second; ↑/↓ previous/next cut; Home/End; Ctrl+B; Delete/Backspace; Ctrl+Z / Ctrl+Shift+Z; N (snapping); Ctrl+Shift+L (linked); Ctrl+scroll (zoom).
- **Visual (spec §9):**
  - **Look:** `docs/superpowers/specs/assets/quick-trim-mockup.html` is the approved look.
  - **Red** `#d43545` appears ONLY on the playhead, the Quick export border and the edited dot.
  - **Selection:** a bone-white 1.5px outline.
  - **Track colors:** A1 `#7fa58c`, A2 `#c9a063`, A3 `#7f9cc4`, A4 `#b08bb3`, A5 `#4fa8a2`, A6 `#d89a3e`. Clip fill is about 15% over near-black with a border at about 38%.
  - **Type:** Inter plus Instrument Serif italic, wordmark only. Labels are uppercase, at least 11px, `letter-spacing: 0.071em`.
  - **Shapes:** 8px outline buttons; no gradients, glows or filled buttons.
- **Frontend process:**
  - Use the `impeccable` skill (craft floor; run its detector on changed UI).
  - Review against the Vercel `web-design-guidelines` skill in Task 12.
  - Theme scrollbars, focus rings and selection.
- Commit messages: plain summaries, no AI attribution.

## Review Focus

1. **Clip with no audio, or only empty audio tracks:**
   - the editor shows the video only, with the "+N empty tracks" toggle;
   - video exports produce no audio stream;
   - **Export audio** is disabled with the hint "This clip has no audio to export".

   Tests in Tasks 2 and 7.
2. **Trying to delete the last remaining video:** the delete is refused (the timeline is unchanged), so the video track always holds at least one segment. Test in Task 3.
3. **Every audio track muted, or all audio deleted:** video exports have no audio stream and are not padded with silence; Export audio is disabled. Tests in Tasks 3 and 7.
4. **The source deleted or renamed in Explorer while it's open:** the export fails with "Hasami couldn't read this video." and nothing else is touched. Test in Task 7.
5. **A very long input (e.g. a 2-hour recording):** thumbnails stay ≤ 300, peaks ≤ 200,000 values per track, and audio is streamed, never decoded whole into memory. Tests in Task 2 (pure sizing functions) and Task 10 (the engine uses `<audio>` elements).

## Task Dependencies

Tasks 1–6 are independent of each other.

| Task | Needs |
|---|---|
| 7 | 1, 5, 6 |
| 8 | 2, 5, 7 |
| 9 | 3, 4 |
| 10 | 9 |
| 11 | 8, 10 |
| 12 | all |

---

## File Structure

```
editor.html                                Vite entry for the editor window
src/editor/main.ts                         mounts EditorWindow; mock mode when not in Tauri
src/editor/timeline.ts (+ .test.ts)        pure timeline model
src/editor/history.ts (+ .test.ts)         undo/redo stack
src/editor/playbackModel.ts (+ .test.ts)   shuttle state, timecode, cut navigation, time↔source mapping
src/editor/keys.ts (+ .test.ts)            keyboard → command map
src/editor/api.ts                          typed wrappers for commands + the hasami://editor event
src/editor/store.svelte.ts (+ .test.ts)    EditorStore (runes): media, history, selection, playhead, zoom, toggles, queue, exports
src/editor/engine.ts                       PlaybackEngine (video + per-track audio, Web Audio gains)
src/editor/geometry.ts (+ .test.ts)        px↔time, edge hit-testing, snapping in px
src/editor/mock.ts                         fake media/peaks/thumbs for browser dev
src/editor/components/{TopBar,Viewer,Toolbar,TrackHeaders,Lanes,StatusBar,Summary,ConfirmDialog}.svelte
src/editor/EditorWindow.svelte             layout
src-tauri/src/editor/mod.rs
src-tauri/src/editor/media.rs              prepare, empty detection, extraction, peaks, thumbs, proxy
src-tauri/src/editor/request.rs            ExportRequest types (serde) + filter graph + encoder choice
src-tauri/src/editor/session.rs            EditorSessions (work dirs, cleanup)
src-tauri/src/jobs/export.rs               export job
src-tauri/src/fsops.rs                     (modify) clipped_path, commit_new, move_to_raw
src-tauri/src/jobs/compress.rs             (modify) extract encode_to_limit
src-tauri/src/jobs/queue.rs                (modify) JobKind::Export, enqueue_one
src-tauri/src/app.rs                       (modify) editor window, commands, event routing, exit rule
src-tauri/tests/{editor_media,export}.rs
```

---

### Task 1: Output naming, non-replacing commit, Raw folder move

**Files:**
- Modify: `src-tauri/src/fsops.rs`
- Test: unit tests in `fsops.rs`

**Interfaces:**
- Consumes: the existing `unique_path`, `rename_no_replace`, `FsError`, `recycle` in `fsops.rs`.
- Produces:
  ```rust
  pub const CLIPPED_PREFIX: &str = "[CLIPPED] ";
  pub fn clipped_path(input: &Path, ext: &str) -> PathBuf   // already unique-ified
  pub fn commit_new(temp: &Path, final_path: &Path) -> Result<(), FsError>
  pub fn move_to_raw(original: &Path) -> Result<Option<PathBuf>, FsError>  // None = already in a Raw folder, left in place
  pub fn in_raw_folder(path: &Path) -> bool
  ```
  - `clipped_path`:
    - directory = `input`'s parent, or the grandparent when `in_raw_folder(input)`;
    - stem = the input stem with any leading `[CLIPPED] ` removed, then `CLIPPED_PREFIX + stem + "." + ext`;
    - result = `unique_path(that, None)`.
  - `commit_new`:
    - does a non-replacing rename of temp → final with the same 5 × 200 ms retry `replace_in_place` uses;
    - on failure deletes the temp file unless the rename may have happened, and returns a plain `FsError`;
    - never touches any original.
  - `move_to_raw`:
    - creates `<parent>\Raw` if missing;
    - target = `unique_path(Raw\<file name>, None)`;
    - moves the file with a non-replacing rename.
  - `in_raw_folder`: the parent folder's name equals `Raw`, compared case-insensitively.

- [ ] **Step 1: Write failing tests:**
  - `clipped_path_prefixes_and_keeps_ext`: `C:\d\Replay 1.mp4`, `"mp4"` → `C:\d\[CLIPPED] Replay 1.mp4`.
  - `clipped_path_never_doubles_prefix`: `C:\d\[CLIPPED] Replay 1.mp4` → `C:\d\[CLIPPED] Replay 1.mp4` when that path is free. Uses a temp dir.
  - `clipped_path_counts_collisions`: if `[CLIPPED] a.mp4` exists → `[CLIPPED] a (2).mp4`.
  - `clipped_path_from_raw_folder_goes_to_parent`: `<tmp>\raw\a.mov`, `"mp4"` → `<tmp>\[CLIPPED] a.mp4`.
  - `commit_new_moves_and_never_overwrites`: with the final path free → Ok, contents moved, temp gone. With the final path existing → Err, the existing file's bytes unchanged, temp deleted.
  - `move_to_raw_creates_folder_and_numbers`: `a.mp4` → `Raw\a.mp4`. A second `a.mp4` → `Raw\a (2).mp4`.
  - `move_to_raw_leaves_files_already_in_raw`: `<tmp>\Raw\a.mp4` → `Ok(None)`, file still there.
- [ ] **Step 2:** Run `cargo.exe test fsops::` and confirm the new tests fail.
- [ ] **Step 3:** Implement the four functions, reusing `rename_no_replace` and the retry helper.
- [ ] **Step 4:** Run `cargo.exe test fsops::` and confirm all pass, including the existing ones.
- [ ] **Step 5: Commit.** `git commit -m "feat: clipped output naming, non-replacing commit, move to Raw"`

### Task 2: Editor media preparation

**Files:**
- Create: `src-tauri/src/editor/{mod.rs,media.rs}`; add `pub mod editor;` to `lib.rs`
- Test: unit tests in `media.rs`; `src-tauri/tests/editor_media.rs`

**Interfaces:**
- Consumes: `probe`, `MediaInfo`, `FfmpegPaths`, `hidden_command`, `run_ffmpeg`, `CancelToken` (Plan 1); the `tests/common` fixtures.
- Produces:
  ```rust
  pub const EMPTY_TRACK_PEAK_DB: f64 = -60.0;
  pub const PEAKS_PER_SECOND: f64 = 100.0;
  pub const MAX_PEAKS: usize = 200_000;
  pub const MAX_THUMBS: usize = 300;
  pub const THUMB_HEIGHT: u32 = 72;
  #[derive(Serialize)] pub struct TrackMedia { pub index: usize /* N in 0:a:N */, pub empty: bool, pub file: Option<PathBuf> /* extracted audio, None when empty */ }
  #[derive(Serialize)] pub struct PreparedMedia { pub info: MediaInfo, pub tracks: Vec<TrackMedia> }
  #[derive(Serialize)] pub struct Peaks { pub rate: f64 /* values per second */, pub values: Vec<f32> /* 0..=1 */ }
  #[derive(Serialize)] pub struct Thumb { pub t: f64, pub file: PathBuf }
  pub fn prepare(paths: &FfmpegPaths, input: &Path, work_dir: &Path) -> Result<PreparedMedia, JobError>
  pub fn detect_empty_tracks(paths: &FfmpegPaths, input: &Path, count: usize) -> Result<Vec<bool>, MediaError>
  pub fn peaks_rate(duration_s: f64) -> f64          // min(PEAKS_PER_SECOND, MAX_PEAKS / duration)
  pub fn thumb_interval(duration_s: f64) -> f64      // max(1.0, duration / MAX_THUMBS)
  pub fn peaks(paths: &FfmpegPaths, track_file: &Path, duration_s: f64) -> Result<Peaks, MediaError>
  pub fn thumbnails(paths: &FfmpegPaths, input: &Path, duration_s: f64, out_dir: &Path) -> Result<Vec<Thumb>, MediaError>
  pub fn make_proxy(paths: &FfmpegPaths, input: &Path, out_dir: &Path, cancel: &CancelToken, on_progress: &mut dyn FnMut(f64)) -> Result<PathBuf, JobError>
  ```
- Rules:
  - **Empty track:** its peak level (ffmpeg `volumedetect` `max_volume`) is below `EMPTY_TRACK_PEAK_DB`.
  - **Extraction:**
    - `aac` → `-map 0:a:N -c copy` into `work_dir\a<N>.m4a`; any other codec → `-c:a aac -b:a 192k`.
    - Empty tracks aren't extracted.
  - **Peaks:**
    - decode the track to mono s16le at 8000 Hz through a pipe;
    - each value is the max |sample| / 32768 over a window of `8000 / rate` samples.
  - **Thumbnails:**
    - one ffmpeg run with `fps=1/interval,scale=-2:THUMB_HEIGHT` writing `out_dir\thumb_%05d.jpg`;
    - `t = (k − 1) × interval`.
  - **Proxy:** `libx264 -preset ultrafast -crf 23`, scaled to height ≤ 720 (never upscaled), `-pix_fmt yuv420p`, `-an`, into `out_dir\proxy.mp4`.
  - `prepare` probe failure → `JobError::Message(UNREADABLE)` (reuse `jobs::UNREADABLE`; make it `pub(crate)` if needed).

- [ ] **Step 1: Write failing tests:**
  - Unit tests:
    - `peaks_rate_caps_long_inputs`: `peaks_rate(90.0) == 100.0`; `peaks_rate(7200.0) == 200_000.0 / 7200.0`.
    - `thumb_interval_bounds_count`: `thumb_interval(90.0) == 1.0`; `thumb_interval(7200.0) == 24.0`. (Review Focus 5)
  - Integration tests in `tests/editor_media.rs`:
    - `prepare_marks_silent_tracks_empty`: `fixture_multitrack_hevc` (sine, anullsrc, sine) → `tracks.len() == 3`, `empty == [false, true, false]`, files exist only for 0 and 2, both `.m4a`.
    - `prepare_video_only_has_no_tracks` (Review Focus 1): `tracks.is_empty()`.
    - `peaks_follow_the_signal`: `peaks` on the extracted track-0 file of the multitrack fixture → `rate == 100.0`, `values.len()` within ±2 of 500, and every value > 0.2.
    - `peaks_of_silence_are_flat`: `peaks` on a 3 s silent `.m4a`, generated in the test with `anullsrc` + `-c:a aac` → every value < 0.01.
    - `thumbnails_cover_the_clip`: the 5 s fixture → 5 thumbs at `t` = 0,1,2,3,4, each a JPEG 72 px tall.
    - `proxy_is_h264_720p`: from the 1080p fixture → codec `h264`, height 720, no audio.
    - `prepare_unreadable_is_plain`: a text file named `.mp4` → `Err` with message `"Hasami couldn't read this video."`.
- [ ] **Step 2:** Run `cargo.exe test editor::media --lib` and `cargo.exe test --test editor_media`, and confirm they fail.
- [ ] **Step 3:** Implement. Use `run_ffmpeg` for the proxy (progress and cancel). Plain `hidden_command` calls with checked status are fine for the others.
- [ ] **Step 4:** Run the same commands and confirm they pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: prepare editor media (empty tracks, audio extraction, peaks, thumbnails, proxy)"`

### Task 3: Timeline model and undo history (TypeScript)

**Files:**
- Create: `src/editor/timeline.ts`, `src/editor/timeline.test.ts`, `src/editor/history.ts`, `src/editor/history.test.ts`

**Interfaces:**
- Produces (all pure, returning new objects):
  ```ts
  export interface Seg { id: string; link: string; src: [number, number]; at: number }   // seconds; at = timeline start
  export interface AudioTrack { index: number; segs: Seg[]; gainDb: number; muted: boolean }
  export interface Timeline { sourceDuration: number; fps: number; video: Seg[]; audio: AudioTrack[]; nextId: number }
  export function createTimeline(sourceDuration: number, fps: number, audioIndexes: number[]): Timeline
  export function quantize(t: number, fps: number): number            // Math.round(t*fps)/fps
  export function segLen(s: Seg): number
  export function videoEnd(tl: Timeline): number
  export function cutAt(tl: Timeline, t: number): Timeline
  export function linkedIds(tl: Timeline, segId: string): string[]
  export function deleteSegs(tl: Timeline, ids: string[], linked: boolean): Timeline
  export function moveEdge(tl: Timeline, segId: string, edge: 'start' | 'end', toT: number, linked: boolean): Timeline
  export function setGain(tl: Timeline, index: number, gainDb: number): Timeline   // clamp to [-60, 6]; -60 shown as −∞
  export function setMuted(tl: Timeline, index: number, muted: boolean): Timeline
  export function snapCandidates(tl: Timeline, playhead: number, exclude?: { segId: string; edge: 'start' | 'end' }): number[]
  export function snap(t: number, candidates: number[], thresholdS: number): number
  export function cutCount(tl: Timeline): number                       // video.length - 1
  export interface ExportRequest { session: number; input: string; mode: 'quick' | 'save_raw' | 'audio'; fit_20mb: boolean; duration_s: number; video: { src_start: number; src_end: number }[]; audio: { index: number; gain_db: number; pieces: { at: number; src_start: number; src_end: number }[] }[] }
  export function toExportRequest(tl: Timeline, session: number, input: string, mode: ExportRequest['mode'], fit20: boolean): ExportRequest
  export function hasAudibleAudio(tl: Timeline): boolean               // any unmuted track with material before videoEnd
  // history.ts
  export interface History<T> { past: T[]; present: T; future: T[] }
  export const HISTORY_CAP = 200
  export function initHistory<T>(t: T): History<T>
  export function push<T>(h: History<T>, t: T): History<T>             // clears future; drops oldest past beyond cap; no-op if t === present
  export function undo<T>(h: History<T>): History<T>
  export function redo<T>(h: History<T>): History<T>
  export function isEdited<T>(h: History<T>): boolean                  // past.length > 0
  ```
- Semantics (fixed by this plan):
  - `createTimeline`: one video seg `src [0,D]`, `at 0`, `link "L0"`. One seg per audio index, also `link "L0"`. `gainDb 0`, `muted false`. Ids are `"S" + n` from `nextId`.
  - Video `at` values are always derived: each is the sum of the earlier lengths.
  - `cutAt(t)`:
    - `t` is quantized first.
    - Every segment on every track with `at < t < at + len` is split.
    - All right-hand pieces that came from segments sharing a link get one new shared link `"L" + n`. Left pieces keep theirs.
  - `deleteSegs`:
    - If the selection would remove every video segment, return `tl` unchanged.
    - With video segments selected and `linked`: for each removed video range `[a, b)` (timeline time, processed right to left), cut every audio track at `a` and `b`, remove audio material inside `[a, b)`, and shift every later audio segment left by `b − a`. Video ripples.
    - With video segments selected and `!linked`: only the video ripples; audio doesn't move.
    - Audio segments in `ids` that weren't removed by the above are deleted where they sit, leaving silence.
  - `moveEdge`:
    - `toT` is quantized and clamped so `src` stays within `[0, sourceDuration]` and the length stays ≥ one frame.
    - **Video edges:**
      - The `end` edge changes `src[1]` by `Δ = toT − (at + len)`.
      - The `start` edge changes `src[0]` by `Δ = toT − at`; the segment's `at` stays put, and its length changes by `−Δ`.
      - Later video segments ripple by the length change.
      - With `linked`, every audio segment with the same link gets the same `src` change, and audio segments after the edge shift by the same length change.
    - **Audio edges:** `start` moves `at` and `src[0]` together, and `end` moves `src[1]`. Both are clamped so they never overlap a neighbor on the same track and `at ≥ 0`.
  - `snapCandidates`: the playhead, every segment's start and end on every track (minus `exclude`), and `videoEnd`.
  - `snap`: returns the nearest candidate within `thresholdS`, otherwise `t`.
  - `toExportRequest`:
    - video pieces in order;
    - per unmuted track, pieces clipped to `[0, videoEnd]`, dropping zero-length ones, and dropping tracks with no pieces;
    - `duration_s = videoEnd`;
    - **audio mode:** set `video: []`, with audio pieces as above.

- [ ] **Step 1: Write failing tests** with `D = 90`, `fps = 60`, audio `[0, 1]` (call it `tl0`). Expected values:
  - `cutAt_splits_every_track`: `cutAt(cutAt(tl0, 12), 31)` gives video src `[[0,12],[12,31],[31,90]]` at `[0,12,31]`. Each audio track has 3 segs with the same src and at. The middle pieces share one link across video, A0 and A1; so do the right pieces, with a different link.
  - `cutAt_quantizes`: `cutAt(tl0, 12.004).video[0].src[1] === 12`.
  - `linked_delete_ripples_all_tracks` (from `tl2 = cutAt(cutAt(tl0,12),31)`, deleting `linkedIds(tl2, <video seg 2 id>)` with `linked = true`):
    - `videoEnd === 71`;
    - video src `[[0,12],[31,90]]` at `[0,12]`;
    - A0 segs src `[[0,12],[31,90]]` at `[0,12]`.
  - `unlinked_delete_moves_only_video`: deleting only the middle video seg with `linked = false` gives `videoEnd === 71`, with A0 still `[[0,12]@0,[12,31]@12,[31,90]@31]`.
  - `audio_delete_leaves_silence`: deleting A1's middle seg alone leaves A1 `[[0,12]@0,[31,90]@31]` and the video unchanged.
  - `last_video_segment_cannot_be_deleted` (Review Focus 2): `deleteSegs(tl0, [tl0.video[0].id], true)` returns the same `tl0`.
  - `extend_video_end_linked`: after the linked delete, move the end edge of video seg 1 (`[0,12]`) to `14` with `linked = true`. Result:
    - video src `[[0,14],[31,90]]` at `[0,14]`;
    - A0 `[[0,14]@0,[31,90]@14]`.
  - `extend_clamps_to_source`: moving the end edge of the last video seg to `200` gives `src[1] === 90`.
  - `audio_edge_stops_at_neighbor`: after `audio_delete_leaves_silence`, moving the end of A1's first seg to `40` gives `src[1] === 31` (the neighbor starts at 31).
  - `snap_picks_nearest_within_threshold`: `snap(12.05,[12,31],0.1) === 12`; `snap(12.5,[12],0.1) === 12.5`.
  - `export_request_clips_and_skips_muted` (Review Focus 3): A0 has a seg running past `videoEnd` and A1 is muted. The request has one audio track (index 0) whose last piece ends at `videoEnd` measured on the timeline (`at + len ≤ videoEnd`). `hasAudibleAudio` is false when both are muted.
  - `history_*`: undo/redo restore states; a push after undo clears `future`; the 201st push drops the oldest; `isEdited` is false initially.
- [ ] **Step 2:** Run `cmd.exe /c npm test -- src/editor` and confirm the tests fail.
- [ ] **Step 3:** Implement `timeline.ts` and `history.ts`.
- [ ] **Step 4:** Run `cmd.exe /c npm test -- src/editor` and confirm they pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: pure timeline model and undo history for the trim editor"`

### Task 4: Playback model and keyboard map (TypeScript)

**Files:**
- Create: `src/editor/playbackModel.ts` (+ `.test.ts`), `src/editor/keys.ts` (+ `.test.ts`)

**Interfaces:**
- Consumes: `Timeline`, `Seg`, `videoEnd`, `segLen` (Task 3).
- Produces:
  ```ts
  export interface Shuttle { dir: -1 | 0 | 1; speed: 1 | 2 | 4 | 8 }
  export const STOPPED: Shuttle
  export function shuttle(s: Shuttle, key: 'J' | 'K' | 'L'): Shuttle
  export function formatTimecode(t: number, fps: number): string      // "HH:MM:SS:FF", FF = floor(frac * round(fps))
  export function cutPoints(tl: Timeline): number[]                    // 0, every video seg boundary, videoEnd (sorted, unique)
  export function prevCut(tl: Timeline, t: number): number             // greatest cut < t - 1e-6, else 0
  export function nextCut(tl: Timeline, t: number): number             // least cut > t + 1e-6, else videoEnd
  export function stepFrames(t: number, fps: number, n: number, end: number): number   // quantized, clamped to [0, end]
  export function videoSourceAt(tl: Timeline, t: number): { seg: Seg; srcT: number } | null
  export function audioSourceAt(segs: Seg[], t: number): { seg: Seg; srcT: number } | null
  // keys.ts
  export type Command = 'playPause' | 'J' | 'K' | 'L' | 'frameBack' | 'frameFwd' | 'secBack' | 'secFwd' | 'prevCut' | 'nextCut' | 'home' | 'end' | 'cut' | 'delete' | 'undo' | 'redo' | 'toggleSnap' | 'toggleLinked'
  export function keyToCommand(e: { key: string; shiftKey: boolean; ctrlKey: boolean; metaKey?: boolean }): Command | null
  ```
- `shuttle` rules:
  - `L` from stopped or reverse → `{1,1}`; `L` while forward doubles the speed up to 8.
  - `J` mirrors `L` for reverse.
  - `K` → `STOPPED` (`{0,1}`).
- `keyToCommand`:
  - a lowercase letter matches its uppercase command;
  - Ctrl+Z → undo; Ctrl+Shift+Z → redo; Ctrl+B → cut; Ctrl+Shift+L → toggleLinked;
  - N with no modifier → toggleSnap;
  - Delete and Backspace → delete;
  - ArrowLeft/Right with Shift → sec; without Shift → frame;
  - ArrowUp → prevCut; ArrowDown → nextCut; " " → playPause; Home/End.

- [ ] **Step 1: Write failing tests:**
  - `shuttle`: the L-tap sequence from stopped is `{1,1},{1,2},{1,4},{1,8},{1,8}`; J after `{1,4}` → `{-1,1}`; K → STOPPED.
  - `formatTimecode(21.7166, 60) === "00:00:21:43"`; `formatTimecode(3725.5, 30) === "01:02:05:15"`.
  - `cutPoints` on Task 3's `tl2` → `[0,12,31,90]`. `prevCut(tl2, 12) === 0`, `nextCut(tl2, 12) === 31`, `nextCut(tl2, 95) === 90`.
  - `stepFrames(1, 60, -1, 90) === 59/60`; `stepFrames(0, 60, -1, 90) === 0`; `stepFrames(89.99, 60, 60, 90) === 90`.
  - `videoSourceAt` after the linked delete from Task 3: `t = 12.5` → `srcT === 31.5`.
  - `keyToCommand`: one assertion per row of the spec §5.5 table, plus `keyToCommand({key:'b', ctrlKey:false, shiftKey:false}) === null`.
- [ ] **Step 2:** Run `cmd.exe /c npm test -- src/editor` and confirm the tests fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same command and confirm they pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: playback model, timecode and DaVinci keyboard map"`

### Task 5: Export request, filter graph and encoder choice (Rust)

**Files:**
- Create: `src-tauri/src/editor/request.rs`
- Test: unit tests in `request.rs`; one integration test in `src-tauri/tests/export.rs` (render only)

**Interfaces:**
- Consumes: `MediaInfo`, `FfmpegPaths`, `hidden_command` (Plan 1).
- Produces:
  ```rust
  #[derive(Debug, Clone, PartialEq, Deserialize)] #[serde(rename_all = "snake_case")]
  pub enum ExportMode { Quick, SaveRaw, Audio }
  #[derive(Debug, Clone, PartialEq, Deserialize)] pub struct VideoPiece { pub src_start: f64, pub src_end: f64 }
  #[derive(Debug, Clone, PartialEq, Deserialize)] pub struct AudioPiece { pub at: f64, pub src_start: f64, pub src_end: f64 }
  #[derive(Debug, Clone, PartialEq, Deserialize)] pub struct AudioTrackRequest { pub index: usize, pub gain_db: f64, pub pieces: Vec<AudioPiece> }
  #[derive(Debug, Clone, PartialEq, Deserialize)] pub struct ExportRequest { pub session: u64, pub input: PathBuf, pub mode: ExportMode, pub fit_20mb: bool, pub duration_s: f64, pub video: Vec<VideoPiece>, pub audio: Vec<AudioTrackRequest> }
  pub struct Graph { pub filter: String, pub video_out: Option<String>, pub audio_out: Option<String> }
  pub fn build_graph(req: &ExportRequest) -> Graph
  pub struct VideoEncoder { pub codec: &'static str, pub args: Vec<String> }
  pub fn encoder_available(paths: &FfmpegPaths, name: &str) -> bool        // 1-frame test encode, cached per name (OnceLock<Mutex<HashMap>>)
  pub fn same_codec_encoder(source_codec: &str, container: &str, bitrate: u64, hw: &dyn Fn(&str) -> bool) -> VideoEncoder
  pub fn source_video_bitrate(info: &MediaInfo) -> u64                    // video.bit_rate, else bit_rate − 192k × audio tracks, else 8_000_000; at least 1_000_000
  pub fn audio_codec_args(container: &str) -> Vec<String>                 // webm: libopus -b:a 160k; else aac -b:a 192k
  ```
- `build_graph` (fixed by this plan):
  - **Video:**
    - piece `i` is `[0:v:0]trim=start=S:end=E,setpts=PTS-STARTPTS[v<i>]`;
    - then `[v0][v1]…concat=n=N:v=1:a=0[vout]`, or `[v0]null[vout]` when N == 1;
    - `video_out = Some("[vout]")`;
    - no pieces → `None`.
  - **Audio:**
    - per track `k`, piece `j` is `[0:a:K]atrim=start=S:end=E,asetpts=PTS-STARTPTS,adelay=D:all=1[a<k>_<j>]`, with `D = round(at × 1000)` ms;
    - the track's pieces go through `amix=inputs=J:normalize=0:duration=longest` (skipped when J == 1), then `volume=GdB` (G formatted with 2 decimals), giving `[t<k>]`;
    - all tracks go through `amix=inputs=T:normalize=0:duration=longest` (skipped when T == 1), then `atrim=end=DUR,aformat=channel_layouts=stereo`, giving `[aout]`, with `DUR = duration_s`;
    - no tracks → `audio_out = None`.
  - Numbers are formatted with `{:.6}` and trailing zeros trimmed.
- `same_codec_encoder`:
  - `h264`: `h264_amf` if `hw("h264_amf")`, with `-rc vbr_peak -b:v B -maxrate 1.5B`; else `libx264 -preset medium -b:v B -maxrate 1.5B -bufsize 2B -pix_fmt yuv420p`.
  - `hevc`: `hevc_amf` / `libx265`, with the same bitrate args, plus `-tag:v hvc1` for mp4/mov.
  - `av1`: `av1_amf` / `libsvtav1 -b:v B`.
  - `vp9`: `libvpx-vp9 -b:v B`.
  - Anything else falls back to the h264 rules.

- [ ] **Step 1: Write failing tests:**
  - `graph_single_piece_no_audio`: one video piece `[2,5]`, no audio → filter exactly `"[0:v:0]trim=start=2:end=5,setpts=PTS-STARTPTS[v0];[v0]null[vout]"`, with `audio_out` None.
  - `graph_concat_and_mix`: video `[0,12],[31,90]`. Track 0 with pieces `{at 0, 0→12}` and `{at 12, 31→90}`, gain 0. Track 2 with `{at 0, 0→12}`, gain −6. Duration 71.
    - The filter contains `concat=n=2:v=1:a=0[vout]`, `[0:a:0]atrim=start=31:end=90,asetpts=PTS-STARTPTS,adelay=12000:all=1`, `volume=-6.00dB`, and `amix=inputs=2:normalize=0:duration=longest`.
    - It ends with `atrim=end=71,aformat=channel_layouts=stereo[aout]`.
    - The test asserts these substrings.
  - `graph_audio_mode_has_no_video`: `video: []` → `video_out` None, `audio_out` Some.
  - `encoder_hevc_hw_and_tag`: `same_codec_encoder("hevc","mp4",10_000_000,&|_| true)` → codec `"hevc_amf"`, args contain `"-tag:v","hvc1"` and `"-b:v","10000000"`. With `&|_| false` → codec `"libx265"`.
  - `encoder_unknown_falls_back_to_h264`: `"prores"` → `libx264` (no hw).
  - `bitrate_fallbacks`: `video.bit_rate = Some(9e6)` → 9_000_000. No video bitrate, `bit_rate 10e6`, 3 tracks → 10e6 − 576_000. Nothing known → 8_000_000.
  - Integration test `render_graph_cuts_duration` in `tests/export.rs`:
    - Build the multitrack fixture and a request with video `[0,1],[2,4]` and track 0 pieces `{0,0→1},{1,2→4}`.
    - Run ffmpeg with `-filter_complex <graph> -map [vout] -map [aout] -c:v libx264 -c:a aac` to a temp mp4.
    - The probe shows duration within 0.1 of 3.0, 1 video stream and 1 audio stream.
- [ ] **Step 2:** Run `cargo.exe test editor::request --lib` and `cargo.exe test --test export render_graph`, and confirm they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same commands and confirm they pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: export request types, ffmpeg filter graph and same-codec encoder choice"`

### Task 6: Extract the size-limited encode from compress (refactor)

**Files:**
- Modify: `src-tauri/src/jobs/compress.rs`
- Test: the existing `tests/compress.rs` and compress unit tests, unchanged

**Interfaces:**
- Produces:
  ```rust
  /// Two-pass encode of `input` per `plan` into `output`, retried smaller until it verifies at
  /// ≤ SIZE_LIMIT_BYTES. Creates and removes its own pass-log folder under `work_root`.
  /// Errors are already plain-language (UNREADABLE / DIDNT_CHECK_OUT / NEVER_FIT / Encode).
  pub(crate) fn encode_to_limit(paths: &FfmpegPaths, input: &Path, info: &MediaInfo, plan: &CompressPlan,
      output: &Path, work_root: &Path, cancel: &CancelToken, on_progress: &mut dyn FnMut(JobProgress)) -> Result<MediaInfo, JobError>
  ```
  `compress_in` calls it; its behavior doesn't change.

- [ ] **Step 1:** Run `cargo.exe test --test compress` and `cargo.exe test jobs::compress` to record the baseline (all pass).
- [ ] **Step 2:** Move the pass-log folder, the `encode_until_fits` loop and `plain_verify_error` into `encode_to_limit`. `compress_in` keeps probing, planning, the confirm and AlreadySmall checks, `unique_path`/temp naming and the commit.
- [ ] **Step 3:** Add the unit test `encode_to_limit_writes_to_the_given_output`: encode `fixture_big_h264(dir, 4)` to `dir/out.mp4` with a plan from `plan_compress`. The output is ≤ 20,000,000 bytes, the input is untouched, and no `hasami-2pass-*` folder remains under the given `work_root`.
- [ ] **Step 4:** Run the full `cargo.exe test`; everything passes, with the same counts plus 1.
- [ ] **Step 5: Commit.** `git commit -m "refactor: expose encode_to_limit for reuse by editor exports"`

### Task 7: Export job

**Files:**
- Create: `src-tauri/src/jobs/export.rs`; add `pub mod export;` to `jobs/mod.rs`
- Test: `src-tauri/tests/export.rs` (alongside Task 5's render test)

**Interfaces:**
- Consumes: Task 1 (`clipped_path`, `commit_new`, `move_to_raw`, `in_raw_folder`), Task 5 (`ExportRequest`, `build_graph`, `same_codec_encoder`, `encoder_available`, `source_video_bitrate`, `audio_codec_args`), Task 6 (`encode_to_limit`), Plan 1 (`probe`, `plan_compress`, `run_ffmpeg`, `verify_output`, `commit_replace`, `temp_path_for`, `JobOutcome`, `JobProgress`, `JobError`, `UNREADABLE`).
- Produces:
  ```rust
  pub fn export(paths: &FfmpegPaths, req: &ExportRequest, cancel: &CancelToken, on_progress: &mut dyn FnMut(JobProgress)) -> Result<JobOutcome, JobError>
  ```
- Flow (fixed by this plan):
  1. Probe `req.input`; on error → `JobError::Message(UNREADABLE)`.
  2. **Audio mode:**
     - with no audio tracks in the request → `Message("This clip has no audio to export.")`;
     - final = `clipped_path(input, "mp3")`;
     - render `[aout]` with `libmp3lame -q:a 2`, in one pass.
  3. **Video modes without fit:**
     - ext = the source container;
     - one pass with `-filter_complex`, mapping `[vout]` (and `[aout]` if any, else `-an`), the encoder from `same_codec_encoder(…, &|n| encoder_available(paths, n))`, `audio_codec_args`, and `-f` for the container's muxer.
  4. **Video modes with fit:**
     - ext = `mp4`;
     - pass 1/3 renders an intermediate (`libx264 -preset veryfast -crf 14`, `aac 320k`) to a unique work folder under the system temp;
     - probe it, `plan_compress` (`AlreadySmall` → just `-c copy` remux to the temp);
     - passes 2–3/3 run `encode_to_limit` into the temp;
     - the work folder is removed on every path.
  5. Verify with `verify_output(temp, req.duration_s, fit.then_some(SIZE_LIMIT_BYTES))`. Tolerance comes from Plan 1. Map non-size errors to plain text: "Couldn't export this clip: the result didn't check out."
  6. **Commit:**
     - **Quick:** `commit_replace(temp, input, final)` (new-name path: renamed in, then the original recycled).
     - **SaveRaw:** `commit_new(temp, final)`, then `move_to_raw(input)`. If the move fails → `Message("Saved \"<final file name>\", but couldn't move the original into the Raw folder.")` with the new file kept.
     - **Audio:** `commit_new(temp, final)`.
  7. `Done { output: final, bytes }`. On any error or cancel, delete the temp file unless `keeps_new_file()`.

- [ ] **Step 1: Write failing tests** (the multitrack fixture copied into a temp dir as `clip.mp4`; request: video `[0,1],[2,4]`, track 0 pieces `{0,0→1},{1,2→4}`, track 2 piece `{0,0→1}` at −6 dB, duration 3):
  - `quick_export_replaces_with_clipped`:
    - `Done`, output `[CLIPPED] clip.mp4`;
    - duration within 0.1 of 3;
    - video codec `hevc` (same as the source);
    - exactly 1 audio stream;
    - `clip.mp4` gone (recycled);
    - no `.hasami-tmp` left.
  - `save_raw_moves_original`: the output exists, and `Raw\clip.mp4` exists with the original bytes.
  - `audio_export_is_mp3`: `[CLIPPED] clip.mp3`, 1 stream, codec `mp3`, duration within 0.1 of 3, and `clip.mp4` untouched.
  - `fit_export_under_limit`: `fixture_big_h264(dir, 8)` with one piece `[0,8]`, Quick, fit → output `[CLIPPED] big_h264.mp4`, ≤ 20,000,000 bytes, codec `h264`.
  - `all_muted_has_no_audio` (Review Focus 3): `audio: []`, Quick → 0 audio streams.
  - `video_only_source` (Review Focus 1): `fixture_video_only` with `audio: []` → `Done`, 0 audio streams. An Audio mode request → `Err` with message `"This clip has no audio to export."`.
  - `missing_source_is_plain` (Review Focus 4): delete `clip.mp4` before export → `Err` with message `"Hasami couldn't read this video."`, and no new files in the dir.
  - `raw_folder_source_exports_to_parent`: source at `<tmp>\Raw\clip.mp4`, SaveRaw → output `<tmp>\[CLIPPED] clip.mp4`, and the source is still in `Raw`.
  - `cancel_leaves_original`: cancel on the first progress callback → `Err(Cancelled)`, original bytes unchanged, no temp left.
- [ ] **Step 2:** Run `cargo.exe test --test export` and confirm the new tests fail.
- [ ] **Step 3:** Implement `export`.
- [ ] **Step 4:** Run `cargo.exe test --test export` and the full `cargo.exe test`, and confirm they pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: editor export job (quick, save raw, audio, fit to 20 MB)"`

### Task 8: Queue, sessions, commands and window wiring

**Files:**
- Create: `src-tauri/src/editor/session.rs`
- Modify: `src-tauri/src/jobs/queue.rs`, `src-tauri/src/app.rs`, `src-tauri/Cargo.toml` (tauri feature `protocol-asset`), `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src/lib/jobs.ts`, `src/progress/state.ts` (kind `'export'`)
- Test: unit tests in `queue.rs`, `session.rs`, `app.rs` (Flow)

**Interfaces:**
- Consumes: Tasks 2, 5 and 7; Plan 1's `JobQueue`, `Flow`, `Dispatcher`, `Replay`, `Collector`.
- Produces:
  - **`queue.rs`:**
    - `pub struct ExportJob(pub Arc<ExportRequest>)` with `PartialEq`/`Eq`/`Hash` by `Arc::ptr_eq` and pointer hash;
    - `JobKind::Export(ExportJob)`, where `name() == "export"` and `ext() == None`;
    - `pub fn enqueue_one(&self, kind: JobKind, input: PathBuf) -> u64` returns the job id.
    - `MediaRunner` runs `Export` via `jobs::export::export`; `preview` returns None for exports.
  - **`session.rs`:**
    - `pub struct EditorSessions`;
    - `fn open(&mut self, paths: &FfmpegPaths, input: &Path) -> Result<(u64, PreparedMedia, PathBuf /*work_dir*/), JobError>`;
    - `fn work_dir(&self, id: u64) -> Option<&Path>`, `fn input(&self, id: u64) -> Option<&Path>`, `fn close(&mut self, id: u64)` (deletes the work dir);
    - `Drop` closes all of them. Work dirs are `%TEMP%\hasami-editor-<pid>-<id>`.
  - **Commands** (all in `app.rs`; errors as plain `String`):
    - `editor_files() -> Vec<String>`
    - `editor_open(path: String) -> Result<EditorMediaDto, String>`
    - `editor_peaks(session: u64, index: usize) -> Result<Peaks, String>`
    - `editor_thumbs(session: u64) -> Result<Vec<Thumb>, String>`
    - `editor_proxy(session: u64) -> Result<String, String>`
    - `editor_export(request: ExportRequest) -> Result<u64, String>`
    - `editor_discard(path: String) -> Result<(), String>` (`fsops::recycle`)
    - `editor_close(session: u64)`
    - `editor_ready()`
  - `EditorMediaDto { session, path, file_name, size_bytes, duration_s, bit_rate, video: Option<{width, height, fps, codec}>, tracks: Vec<{ index, empty, file: Option<String> }> }`.
  - **Event** `"hasami://editor"` carrying `#[serde(tag = "type")] enum EditorEvent`:
    - `Files { paths: Vec<String> }`
    - `ExportStarted { id, input }`
    - `ExportProgress { id, fraction }` (fraction across all passes)
    - `ExportDone { id, input, output, bytes }`
    - `ExportFailed { id, input, message, original_intact }`
    - `ProxyProgress { session, fraction }`
  - **Window:** label `"editor"`, url `editor.html`, 1280×800, min 960×600, `decorations(false)`, resizable, created on the first Trim batch.
  - **Flow changes:**
    - A Trim batch opens the editor with those files, or emits `Files` to an open editor; `TRIM_NOTICE` and the held-trim logic are removed.
    - Job events for export ids go to the editor as `EditorEvent`s and never open the progress window, except an `ExportFailed` while no editor window exists, which is shown in the progress window as `Failed` with kind `"export"`.
    - Editor events wait for `editor_ready`, the same way progress events wait for `progress_ready`.
    - Exit when settled, with no progress window and no editor window.
  - **Config:**
    - the `protocol-asset` feature and `app.security.assetProtocol.enable = true`, with an empty static scope;
    - at runtime, `asset_protocol_scope().allow_file(input)` and `allow_directory(work_dir, true)` on `editor_open`;
    - CSP adds `media-src 'self' asset: http://asset.localhost`;
    - capabilities add `core:window:allow-toggle-maximize`, `core:window:allow-destroy`, `core:window:allow-set-focus`.
  - `src/progress/state.ts`: kind `'export'` with the title "HASAMI *export*".

- [ ] **Step 1: Write failing tests:**
  - Queue: `enqueue_one_returns_increasing_ids`; `export_jobs_run_through_the_runner` (a fake runner sees `JobKind::Export`).
  - Session: `open_creates_and_close_removes_work_dir`, using the multitrack fixture.
  - Flow:
    - `a_trim_batch_opens_the_editor`;
    - `a_trim_while_the_editor_is_open_appends_files`;
    - `export_events_go_to_the_editor_not_the_progress_window`;
    - `export_failure_without_editor_shows_in_progress_window`;
    - `no_exit_while_the_editor_is_open`;
    - `exit_after_editor_closes_and_exports_drain`.
  - TS: the progress `state.test.ts` gets the case `export_kind_title`.
- [ ] **Step 2:** Run `cargo.exe test` and `cmd.exe /c npm test`, and confirm the new tests fail.
- [ ] **Step 3:** Implement. Keep each command a thin layer over `EditorSessions` / `JobQueue`. Every error string is plain language.
- [ ] **Step 4:** Run the same commands and confirm everything passes.
- [ ] **Step 5: Manual check.** Run `cmd.exe /c npm run tauri -- dev -- -- trim "<copy of a real clip>"`. An editor window opens; Task 9 fills a placeholder page with the UI. Launch a second `trim` while it's open; the files list grows. Describe what you saw in the report.
- [ ] **Step 6: Commit.** `git commit -m "feat: editor sessions, commands, export jobs in the queue, editor window wiring"`

### Task 9: Editor window UI shell

**Files:**
- Create: `editor.html` (add it to Vite's `rollupOptions.input`), `src/editor/main.ts`, `src/editor/api.ts`, `src/editor/store.svelte.ts` (+ `.test.ts`), `src/editor/geometry.ts` (+ `.test.ts`), `src/editor/mock.ts`, `src/editor/EditorWindow.svelte`, `src/editor/components/{TopBar,Viewer,Toolbar,TrackHeaders,Lanes,StatusBar}.svelte`
- Test: `store.test.ts`, `geometry.test.ts`, a visual check against the mockup

**Interfaces:**
- Consumes: Task 3 (`Timeline`, the history functions), Task 4 (`formatTimecode`, `Shuttle`), and Task 8's command and event names and DTO shapes (mirrored in `api.ts`).
- Produces:
  ```ts
  // api.ts — typed wrappers; in mock mode resolved by mock.ts
  export interface EditorMedia { session: number; path: string; file_name: string; size_bytes: number; duration_s: number; bit_rate: number | null; video: { width: number; height: number; fps: number; codec: string } | null; tracks: { index: number; empty: boolean; file: string | null }[] }
  export type EditorEvent = { type: 'Files'; paths: string[] } | { type: 'ExportStarted'; id: number; input: string } | { type: 'ExportProgress'; id: number; fraction: number } | { type: 'ExportDone'; id: number; input: string; output: string; bytes: number } | { type: 'ExportFailed'; id: number; input: string; message: string; original_intact: boolean } | { type: 'ProxyProgress'; session: number; fraction: number }
  export const api: { files(): Promise<string[]>; open(path: string): Promise<EditorMedia>; peaks(s: number, i: number): Promise<{ rate: number; values: number[] }>; thumbs(s: number): Promise<{ t: number; file: string }[]>; proxy(s: number): Promise<string>; exportClip(r: ExportRequest): Promise<number>; discard(path: string): Promise<void>; close(s: number): Promise<void>; ready(): Promise<void>; onEvent(cb: (e: EditorEvent) => void): Promise<() => void>; src(path: string): string /* convertFileSrc */ }
  // geometry.ts
  export const SNAP_PX = 8; export const EDGE_HIT_PX = 6
  export function timeToX(t: number, pxPerSecond: number, scrollX: number): number
  export function xToTime(x: number, pxPerSecond: number, scrollX: number): number
  export function edgeAt(segs: { id: string; at: number; len: number }[], x: number, pxPerSecond: number, scrollX: number): { segId: string; edge: 'start' | 'end' } | null
  export function fitZoom(duration: number, widthPx: number): number
  export function zoomAround(pxPerSecond: number, factor: number, anchorX: number, scrollX: number): { pxPerSecond: number; scrollX: number }   // clamp 2..2000 px/s
  // store.svelte.ts
  export class EditorStore {
    media: EditorMedia | null; history: History<Timeline>; selection: string[]; playhead: number
    pxPerSecond: number; scrollX: number; linked: boolean; snapping: boolean; fit20: boolean; showEmpty: boolean
    peaks: Record<number, { rate: number; values: number[] }>; thumbs: { t: number; file: string }[]
    shuttle: Shuttle; queue: { files: string[]; index: number }; exports: { running: number[]; done: { input: string; output: string }[]; failed: { input: string; message: string; originalIntact: boolean }[] }
    get timeline(): Timeline; get edited(): boolean; get estimateBytes(): number   // quick estimate = size_bytes / duration_s × videoEnd
    load(media: EditorMedia): void      // fresh history from createTimeline(duration, fps, non-empty indexes)
    edit(next: Timeline): void          // push onto history
  }
  ```
- Layout and look follow the approved mockup exactly. That is `docs/superpowers/specs/assets/quick-trim-mockup.html`: open it and match the regions, sizes, labels, colors and type.
  - **Top bar:**
    - the wordmark "HASAMI *quick trim*";
    - the filename and an "EDITED" red dot when `edited`;
    - the meta line: `W×H · FPS · CODEC · N AUDIO TRACKS · M:SS · SIZE`;
    - the Fit to 20 MB toggle, Export audio, Export + save raw and Quick export;
    - custom window controls: minimize, maximize, close.
  - **Viewer:** the `<video>` is contained on black with the transport row below.
  - **Toolbar:** Snap, Linked, Cut Ctrl+B, Undo, Redo, "M:SS KEPT OF M:SS", and the zoom slider.
  - **Headers:** 200px wide; color chips; mute plus a volume slider with a dB readout ("−∞ dB" at −60).
  - **Lanes:**
    - a ruler with major labels every "nice" interval;
    - video segments with thumbnails;
    - audio segments with waveform `<canvas>` drawn from peaks in the track color;
    - a "Video ends" line plus a hatched overhang;
    - a red playhead;
    - the "+N EMPTY TRACKS" toggle under the last track when any track is empty.
  - **Status bar:** "N CUTS · N SELECTED", "QUICK EXPORT ≈ X MB AT SOURCE QUALITY · FIT TO 20 MB: ≤ 20 MB", and the shortcut hints.
  - Mock mode (`editor.html?mock=1`, or automatically when `window.__TAURI_INTERNALS__` is absent): `mock.ts` supplies a 90 s 1080p60 clip with 3 tracks (one empty), synthetic peaks and colored thumbnail data-URIs.
- Frontend process: run `impeccable` setup (`context`). Use the branding DESIGN.md plus spec §9 as the visual authority (impeccable "init" may write PRODUCT.md from spec §1–2, with assumptions labeled). Read the craft floor before editing UI, and run the impeccable detector on the changed files at the end.

- [ ] **Step 1: Write failing tests:**
  - Geometry: `timeToX(10, 20, 50) === 150`; `xToTime(150, 20, 50) === 10`; `edgeAt` returns `end` of seg A at `x = A.end ± 5` and null at ±7; `fitZoom(90, 1080) === 12`; `zoomAround` keeps the time under the anchor fixed (assert `xToTime(anchor)` is equal before and after).
  - Store:
    - `load` with tracks `[{0,false},{1,true},{2,false}]` creates a timeline with audio indexes `[0,2]`;
    - `edited` is false after load and true after one `edit`;
    - `estimateBytes` for `size 412e6, duration 90` with a 44 s videoEnd is ≈ 201.4e6 (within 1e3).
- [ ] **Step 2:** Run `cmd.exe /c npm test -- src/editor` and confirm they fail.
- [ ] **Step 3:** Implement the shell rendering read-only from the store, with no interactions yet beyond the toggles' visual state.
- [ ] **Step 4:** Run `cmd.exe /c npm test`, `npm run check` and `npm run build`. All succeed.
- [ ] **Step 5: Visual check.**
  - Serve `cmd.exe /c npx vite --port 5182` and screenshot `editor.html?mock=1` at 1380×900 with the gstack browse tool (`~/.claude/skills/gstack/browse/dist/browse`).
  - Compare it against a screenshot of the mockup file. Fix the visible differences, then put both screenshot paths in the report.
  - Run the impeccable detector on the changed UI files and fix or justify every finding in the report.
- [ ] **Step 6: Commit.** `git commit -m "feat: trim editor window shell matching the approved mockup"`

### Task 10: Editing interactions and the playback engine

**Files:**
- Create: `src/editor/engine.ts`
- Modify: `src/editor/store.svelte.ts` (+ tests), `src/editor/components/{Viewer,Toolbar,TrackHeaders,Lanes}.svelte`, `src/editor/EditorWindow.svelte`
- Test: `store.test.ts` additions; a manual run in Tauri

**Interfaces:**
- Consumes: Tasks 3, 4 and 9; Task 8's `api.proxy` and the `ProxyProgress` event.
- Produces:
  ```ts
  // store.svelte.ts additions
  command(c: Command): void                      // dispatches every keys.ts command against timeline/selection/playhead/shuttle
  selectAt(segId: string): void                  // linked ? linkedIds : [segId]
  seek(t: number): void                          // quantized, clamped to [0, videoEnd]
  beginEdgeDrag(segId: string, edge: 'start'|'end'): void; dragEdgeTo(t: number): void; endEdgeDrag(): void   // one history entry per drag
  setGain(index: number, db: number, commit: boolean): void; toggleMute(index: number): void                // slider commits on release
  // engine.ts
  export class PlaybackEngine {
    constructor(video: HTMLVideoElement, ctx: AudioContext)
    load(videoSrc: string, tracks: { index: number; src: string }[]): void
    setTimeline(tl: Timeline): void
    play(): void; pause(): void; seek(t: number): void; setShuttle(s: Shuttle): void
    onTime(cb: (t: number) => void): () => void; onEnded(cb: () => void): () => void
    destroy(): void
  }
  export const DRIFT_TOLERANCE_S = 0.04
  ```
- **Engine rules:**
  - **Video:**
    - the `<video>` stays `muted` and plays its source at `|speed|` (`playbackRate`);
    - at each video segment end (detected with `requestVideoFrameCallback`), set `currentTime` to the next segment's `src[0]`;
    - stop at `videoEnd`.
  - **Audio:**
    - each track is one `<audio>` element through `MediaElementAudioSourceNode` → `GainNode` (`10^(db/20)`; 0 when muted or `db ≤ −60`) → destination;
    - in a silent gap the element is paused;
    - on entering a segment, `currentTime = srcT`;
    - when `|audio.currentTime − expected| > DRIFT_TOLERANCE_S`, re-seek.
  - **Reverse shuttle:** audio paused, `video.currentTime` stepped back by `speed / 30` every 1/30 s across segment boundaries.
  - Audio is never decoded whole into memory (Review Focus 5).
- **Interactions:**
  - **Clicks:** clicking a segment selects it (linked aware); clicking the ruler or empty lane area seeks.
  - **Edge drag:** dragging within `EDGE_HIT_PX` of an edge resizes it, with snapping (when `snapping`, `snap` with threshold `SNAP_PX / pxPerSecond`) and a 1px snap indicator line.
  - **Zoom:** Ctrl+scroll zooms around the cursor; plain scroll pans horizontally.
  - **Keyboard:** handled through `keyToCommand` at the window level, ignored while focus is in a text input.
  - **Speed badge:** shows "2×" and so on while shuttling.
  - **Preview fallback:** on `<video>` `error`, or `canPlayType('video/mp4; codecs="hvc1"')` empty for an HEVC source, show the "PREPARING PREVIEW" overlay with `ProxyProgress`, then load `api.proxy(session)`.

- [ ] **Step 1: Write failing store tests:**
  - `command('cut')` at playhead 12 adds 1 cut;
  - `delete` removes the selection (linked) and ripples;
  - `undo`/`redo` restore;
  - `toggleSnap` and `toggleLinked` flip the flags;
  - `frameFwd` moves by 1/fps;
  - `nextCut` jumps;
  - an edge drag creates exactly one history entry;
  - `setGain(0, -6, false)` doesn't push and `setGain(0, -6, true)` does;
  - L taps update `shuttle`.
- [ ] **Step 2:** Run `cmd.exe /c npm test -- src/editor` and confirm they fail.
- [ ] **Step 3:** Implement the store commands, the engine, and the component wiring.
- [ ] **Step 4:** Run `cmd.exe /c npm test`, `npm run check` and `npm run build`. All pass.
- [ ] **Step 5: Manual check** with `cmd.exe /c npm run tauri -- dev -- -- trim "<copy of a real clip>"`, on copies only (never touch `C:\Users\ledan\Videos\Clips` itself). Verify and report:
  - playback with per-track volume and mute heard;
  - cut, delete (linked and unlinked) and audio-only delete leaving silence;
  - edge drag restoring material, with snapping;
  - J/L speeds, frame steps, ↑/↓ cut jumps, undo/redo;
  - zoom;
  - "Video ends" hatching after extending audio.
- [ ] **Step 6: Commit.** `git commit -m "feat: trim editor interactions and synced multi-track playback"`

### Task 11: Exports, trim queue, summary and unsaved-edit confirmation

**Files:**
- Create: `src/editor/components/{Summary,ConfirmDialog}.svelte`
- Modify: `src/editor/store.svelte.ts` (+ tests), `src/editor/components/{TopBar,StatusBar}.svelte`, `src/editor/EditorWindow.svelte`, `src/editor/main.ts`
- Test: `store.test.ts` additions; manual end-to-end

**Interfaces:**
- Consumes: Tasks 3 (`toExportRequest`, `hasAudibleAudio`), 8 (commands, `EditorEvent`) and 9–10.
- Produces:
  ```ts
  // store.svelte.ts additions
  exportClip(mode: 'quick' | 'save_raw' | 'audio'): Promise<void>   // builds the request; video modes then advance(); audio mode stays
  advance(): Promise<void>        // next queued file → api.open + load; none left → showSummary
  skip(): Promise<void>           // confirm when edited; then advance()
  discard(): Promise<void>        // confirm when edited; api.discard(current) then advance()
  applyEvent(e: EditorEvent): void
  get showSummary(): boolean; get canExportAudio(): boolean; get shouldClose(): boolean   // summary shown, nothing running, nothing failed
  ```
- Rules:
  - **Video exports:** Quick export and Export + save raw apply `fit20`.
  - **Export audio:**
    - disabled (with hint "This clip has no audio to export") when `!hasAudibleAudio`;
    - doesn't leave the clip.
  - **Queue:**
    - "3 OF 30" plus SKIP and DISCARD appear in the top bar when `queue.files.length > 1`;
    - `Files` events append to `queue.files`.
  - **"EXPORTING N" strip:** in the top bar while `exports.running.length > 0`.
  - **Summary (after the last file):**
    - lists each export's result, with failures giving the message and, when `originalIntact`, "The original was not changed.";
    - the window closes itself (`getCurrentWindow().close()`) once `shouldClose`, and stays open otherwise.
  - **Unsaved edits:** a close request (window control or Alt+F4, via `onCloseRequested`) asks "Close without exporting? Your edits will be lost." [CLOSE] [KEEP EDITING] in an in-window `<dialog>`. Skip and Discard of an edited clip use the same dialog with "Skip this clip? Your edits will be lost." / "Send this clip to the Recycle Bin?".
  - On leaving a clip, `api.close(session)`.

- [ ] **Step 1: Write failing store tests** (with `api` mocked):
  - `exportClip('quick')` calls `api.exportClip` with `mode 'quick'` and `fit_20mb` equal to the store's `fit20`, then loads the next file;
  - with no next file, `showSummary` is true;
  - `applyEvent(ExportDone)` moves the id from running to done;
  - `ExportFailed` adds to failed;
  - `shouldClose` is false while running, true when the summary is shown and nothing is running or failed, and false with a failure;
  - `canExportAudio` is false when all tracks are muted;
  - `Files` appends;
  - `skip` on an unedited clip advances without confirming.
- [ ] **Step 2:** Run `cmd.exe /c npm test -- src/editor` and confirm they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cmd.exe /c npm test`, `npm run check` and `npm run build`. All pass.
- [ ] **Step 5: Manual end-to-end** on copies of 3 real clips in a temp folder. Report each result:
  - **Multi-select:** launch `trim` with all three in quick succession → one editor showing "1 OF 3".
  - **Clip 1:** cut, then Quick export → clip 2 loads immediately and "EXPORTING 1" shows. `[CLIPPED] <clip1>.mp4` appears, is the same codec as the source, and the raw is in the Recycle Bin.
  - **Clip 2:** Export + save raw with Fit on → the output is ≤ 20 MB and the raw is in `Raw\`.
  - **Clip 3:** Export audio → `[CLIPPED] <clip3>.mp3`, staying on the clip; then Discard → the raw is in the Recycle Bin and the summary shows. The window closes once the exports finish.
  - **Close confirmation:** with edits, closing asks for confirmation.
- [ ] **Step 6: Commit.** `git commit -m "feat: editor exports, trim queue with skip/discard, summary and close confirmation"`

### Task 12: Design and accessibility review pass, release build

**Files:**
- Modify: whatever the reviews require under `src/editor/**`
- Test: the full suites, the detector, the build

**Interfaces:**
- Consumes: everything above.
- Produces: no new interfaces.

- [ ] **Step 1:** Run the `impeccable` critique and audit flows on the editor window, and the Vercel `web-design-guidelines` review on `src/editor/**`. Record their findings in the report.
- [ ] **Step 2:** Fix every finding that is a real problem, such as focus rings, keyboard reachability of every control, `aria-label`s on icon buttons, contrast, and reduced motion. Justify any finding left standing in the report.
- [ ] **Step 3:** Run the full `cargo.exe test`, `cmd.exe /c npm test`, `npm run check`, and the impeccable detector on `src/editor`. All are clean.
- [ ] **Step 4:** Run `cmd.exe /c npm run tauri -- build`, then a silent per-user install of the new setup. Run `trim` on one copied clip through the installed exe (Quick export works), then uninstall silently.
- [ ] **Step 5: Commit.** `git commit -m "fix: editor design and accessibility review findings"`

---

## Follow-ups after Dang's hands-on test (2026-11-01)

Spec §5.1, §5.3, §5.5, §5.7, §5.8 and §12 items 7 and 15 were amended on 2026-11-01. Read those sections first; they are the authority for Tasks 13–15. Tasks 13, 14 and 15 are independent and can run in parallel. Task 15 consumes Task 14's command through `api.ts`, which Task 15 writes.

### Task 13: Ctrl+B cuts only the selected clips when one is under the playhead

**Files:**
- Modify: `src/editor/timeline.ts` (+ tests), the `cut` command in `src/editor/store.svelte.ts` (+ tests)

**Interfaces:**
- Produces: `cutAt(tl: Timeline, t: number, only?: string[]): Timeline`.
  - With `only` omitted, behavior is unchanged: every track.
  - With `only`, only segments whose id is in `only` and that span `t` are split.
  - The link rules don't change: right-hand pieces that came from segments sharing a link get one new shared link.
- Store `command('cut')`:
  - if any selected segment spans the playhead (`at < t < at + len`), call `cutAt(tl, t, selection)`;
  - otherwise call `cutAt(tl, t)`.
  - After a selected cut, the selection stays on the left pieces.

- [ ] **Step 1: Write failing tests:**
  - `cutAt_only_splits_listed`: on `tl0` (D 90, fps 60, audio [0,1]), `cutAt(tl0, 12, [A1 seg id])` splits A1 only. Video and A0 each still have 1 seg.
  - Running the randomized invariant test with `only` drawn from random selections still passes.
  - Store:
    - unlinked, select A1 only, Ctrl+B at 12 → only A1 is cut;
    - linked, select the video → the video and its linked audio are cut, which with `tl0` is every track;
    - nothing selected → every track;
    - a selection not under the playhead → every track.
- [ ] **Step 2:** Run `cmd.exe /c npm test -- src/editor` and confirm the new tests fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cmd.exe /c npm test`, `npm run check` and `npm run build`. All pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: Ctrl+B cuts only the selected clips when one is under the playhead"`

### Task 14: Next video in the folder (Rust)

**Files:**
- Modify: `src-tauri/src/app.rs` (the new command and its registration; `EditorMediaDto` gains `modified_ms`)
- Create: `src-tauri/src/editor/folder.rs` (+ unit tests), and declare it in `editor/mod.rs`

**Interfaces:**
- Produces:
  ```rust
  pub const VIDEO_EXTS: [&str; 4] = ["mp4", "mov", "mkv", "webm"];
  /// The next video after `current` in Explorer's "date, descending" order: siblings in the same folder with a
  /// video extension, newest modified first, ties by file name (case-insensitive); `[CLIPPED] ` outputs skipped;
  /// subfolders not listed. `current` may no longer exist: its position comes from `current_modified_ms` + its name.
  pub fn next_video(current: &Path, current_modified_ms: u64) -> std::io::Result<Option<PathBuf>>
  ```
  - `EditorMediaDto.modified_ms: u64` is the input's modified time in milliseconds since the Unix epoch, read in `editor_open`.
  - Command: `editor_next_file(path: String, modified_ms: u64) -> Result<Option<String>, String>`. Errors are plain language.
- [ ] **Step 1: Write failing unit tests** in `folder.rs`, using a temp dir and `File::set_modified` to fix the times:
  - files a (t=300), b (t=200), c (t=100) → next(a) = b, next(b) = c, next(c) = None;
  - `[CLIPPED] x.mp4` (t=250) is skipped, so next(a) = b;
  - a `.txt` file and a `Raw\` subfolder are ignored;
  - equal times order by name;
  - next(a) still works after a is deleted, given a's old time;
  - extensions match case-insensitively (`.MP4`).
- [ ] **Step 2:** Run `cargo.exe test editor::folder` and confirm the new tests fail.
- [ ] **Step 3:** Implement, then wire the command and `modified_ms`.
- [ ] **Step 4:** Run the full `cargo.exe test` and confirm it passes.
- [ ] **Step 5: Commit.** `git commit -m "feat: find the next video in the folder in date-descending order"`

### Task 15: The editor stays open: export progress, locked clip, Next video, results list

**Files:**
- Modify: `src/editor/{api.ts, mock.ts, store.svelte.ts (+ tests), EditorWindow.svelte, components/TopBar.svelte, components/Summary.svelte}`, plus whatever else the UI needs.

**Interfaces:**
- Consumes:
  - Task 14's command `editor_next_file(path, modified_ms) -> string | null`, and `EditorMedia.modified_ms: number`. Add both to `api.ts` (`api.nextFile(path, modifiedMs)`) and to the mock, where a fake folder of 5 clips walks through.
  - The existing `ExportStarted`/`ExportProgress`/`ExportDone`/`ExportFailed` events.
- Rules (spec §5.7 and §5.8 as amended):
  - **Quick export / Export + save raw:**
    - The clip locks. Edits, Delete, Ctrl+B and edge drags are refused, and the transport still plays.
    - A progress bar with a percentage replaces the export buttons. It is driven by that export's `ExportProgress`, and reads "Queued" until `ExportStarted`.
    - Done → the top bar shows "Saved [CLIPPED] name".
    - Failed → the top bar shows the message and "The original was not changed." when `original_intact`, and the clip unlocks.
  - **Export audio:** the same progress display, and the clip stays editable.
  - **Next video button:** always in the top bar.
    - In queue mode it goes to the next selected file first.
    - Otherwise it calls `api.nextFile(current.path, current.modified_ms)` and opens the result.
    - It is disabled when nothing is next (check ahead of time so the button's disabled state is honest).
    - It asks "Move on? Your edits weren't exported." only when the clip is edited and not exported.
    - It never interrupts a running export.
  - **Skip** is removed (Next video replaces it). **Discard** stays in queue mode and also appears for a single clip, and moves on through the same Next logic. Remember R18/R21: discarding behind a running export goes through the backend's deferred discard.
  - **The window never closes itself:** remove `shouldClose` auto-close. Closing it by hand keeps the existing unsaved-edits confirmation. Exports still running is not a reason to ask.
  - **Results list:** the "EXPORTING N" strip becomes a button that opens the results list (reusing `Summary.svelte` as a popover or panel over the editor). After the last queued file, Next continues into the folder instead of showing the summary.
  - Visual rules are unchanged: red only on the playhead, the Quick export border and the edited dot. The progress bar is a thin bone-white fill on a neutral track, with no red and no gradient.
- [ ] **Step 1: Write failing store tests** (with `api` mocked):
  - quick export locks, `command('cut')` is a no-op while locked, and done shows the saved name;
  - a failure unlocks with the right message;
  - audio export doesn't lock;
  - Next walks the queue then the folder;
  - Next is disabled when `nextFile` returns null;
  - edited and not exported → confirm, exported → no confirm;
  - no auto-close (`shouldClose` gone or always false);
  - Discard moves on through Next.
- [ ] **Step 2:** Run `cmd.exe /c npm test -- src/editor` and confirm the new tests fail.
- [ ] **Step 3:** Implement. Run the impeccable craft floor and detector on the changed UI.
- [ ] **Step 4:** Run `cmd.exe /c npm test`, `npm run check` and `npm run build`. All pass. Check the browser mock with gstack browse and save screenshots.
- [ ] **Step 5: Commit.** `git commit -m "feat: editor stays open with export progress, Next video and a results list"`

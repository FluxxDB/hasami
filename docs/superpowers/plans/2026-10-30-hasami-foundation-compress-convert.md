# Hasami Plan 1: Foundation, Compress & Convert Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A per-user-installed Windows app whose Explorer right-click menu offers **Hasami ▸ Compress / Convert to .mp4 / .mov / .mp3**. It runs those jobs one at a time in a small progress window and never loses an original.

**Architecture:**
- A Tauri 2 app with no main window. Explorer launches `hasami.exe <action> <files…>`, and a single-instance hook funnels every launch into one process.
- That process feeds a serial job queue. Jobs run a bundled, hidden ffmpeg, write to a temp file, verify it, then swap it in and recycle the original.
- The Svelte progress window only renders queue events and sends Cancel and Continue commands back.

**Tech Stack:**
- Rust (stable, MSVC), Tauri 2, `tauri-plugin-single-instance`, `serde`/`serde_json`, `winreg`, `trash`, `thiserror`
- Svelte 5 + TypeScript + Vite, Vitest
- Bundled ffmpeg/ffprobe (GPL build), NSIS installer

**Spec:** `docs/superpowers/specs/2026-10-30-hasami-design.md`. Read §4, §7, §8, §9, and §10 before starting. **Plan 2** (Quick Trim editor and trim queue) follows this plan and reuses its media, fsops, verify, and queue modules.

## Global Constraints

- Windows only. Build and test with the **Windows** toolchain from the repo at `C:\Users\ledan\Documents\Projects\hasami`. From WSL, run `cargo.exe …` inside `src-tauri/` and `cmd.exe /c npm …` at the repo root. Never use WSL's own ffmpeg, cargo, or node.
- Size limit: output is **≤ 20,000,000 bytes**, always. Compress output is **H.264 + AAC in `.mp4`** with **one** mixed audio track.
- The resolution ladder never upscales and never raises fps. **Warn before encoding if the chosen height is < 480.** The floor is ~240p/15 fps with 32 kbps mono audio. Below that, fail with a message.
- **Never invoke HandBrake. No console or other window from ffmpeg** (`CREATE_NO_WINDOW`, `0x08000000`, on every child process).
- File safety (spec §8):
  - Write to `.<final-name>.hasami-tmp` in the destination folder.
  - Verify with ffprobe (duration within tolerance; size ≤ limit when one applies).
  - Rename to the final name, and only then recycle the original.
  - On failure or cancel, delete the temp file and leave the original untouched.
- Originals go to the Recycle Bin (`trash` crate) and are never permanently deleted by Hasami. No Windows dialog may appear.
- Naming:
  - Compress keeps the stem and switches to `.mp4`.
  - Convert keeps the stem and switches to the target extension.
  - If a *different* file already holds the name, append ` (2)`, ` (3)`, …
- Menu: classic cascading **Hasami** submenu on `.mp4 .mov .mkv .webm`, registered under `HKCU` only. "Convert to .X" is hidden on `.X` files. "Quick Trim" is registered now and launches `trim` (a stub in this plan).
- One running process, one serial job queue. The process stays alive until the queue is empty and no window is open.
- Visual design: `docs/branding/DESIGN.md` as adapted by spec §9.
  - Colors: void `#080808`, bone `#fcfcfc`, pebble `#b5b2b2`, smoke `#262525`, red `#d43545` (Plan 1 uses it nowhere).
  - Fonts: Inter + Instrument Serif italic, bundled locally.
  - Uppercase labels at least 11px with `letter-spacing: 0.071em`. Buttons are 8px-radius outlines.
- Mixing audio: `amix=inputs=N:normalize=0` (never let amix divide levels by N). Zero audio streams means no audio filter.
- Child-process arguments go through `std::process::Command` args, never a shell string. Paths can contain spaces, brackets, apostrophes, and non-ASCII characters.

## Review Focus

1. **Awkward filenames** (`[CLIPPED] Ráplay 'x' (2).mp4`, spaces, non-ASCII) must work through Explorer args, ffmpeg, and rename. Test in Task 7.
2. **Original locked by another program** (open in a player) must fail cleanly: temp file removed, original intact, a readable error. Test in Task 4.
3. **Inputs with no audio, or many tracks including silent ones:**
   - Compress of a video-only file works.
   - Convert to .mp3 of a video-only file fails with "This video has no audio".
   - One loud track mixed with 5 silent ones keeps its level within 1 dB.

   Tests in Tasks 7 and 8.
4. **Input already ≤ 20,000,000 bytes and already H.264/AAC MP4 with at most one audio track:** leave it untouched and report "Already under 20 MB". Any other small input is re-encoded with its video bitrate capped at the source's. Test in Task 7.
5. **Cancel mid-encode, or a second right-click while a job runs:**
   - Cancel kills ffmpeg, removes the temp file, and leaves the original untouched.
   - New files queue behind the current job instead of starting a parallel encode.

   Tests in Tasks 6 and 9.

---

## File Structure

```
.gitignore                                  (modify: add node_modules/, dist/, src-tauri/target/, src-tauri/binaries/*.exe)
package.json, vite.config.ts, tsconfig.json, svelte.config.js, vitest config in vite.config.ts
progress.html                               Vite entry for the progress window
scripts/fetch-ffmpeg.ps1                    downloads the pinned ffmpeg build into src-tauri/binaries/
src/lib/theme.css                           design tokens + @font-face (spec §9)
src/lib/fonts/                              Inter (400/500/700) + Instrument Serif Italic, woff2, OFL license files
src/lib/jobs.ts                             TS mirror of Rust event/command payloads
src/progress/main.ts                        mounts ProgressWindow
src/progress/ProgressWindow.svelte          progress UI
src/progress/format.ts (+ format.test.ts)   bytes / eta / percent formatting
src-tauri/Cargo.toml, tauri.conf.json, build.rs, capabilities/default.json
src-tauri/nsis/hooks.nsh                    post-install register, pre-uninstall unregister
src-tauri/src/main.rs                       entry: CLI → register/unregister or run app
src-tauri/src/cli.rs                        argv → Action
src-tauri/src/app.rs                        Tauri setup, single-instance collect window, progress window, commands
src-tauri/src/media/mod.rs
src-tauri/src/media/ffmpeg.rs               locate binaries, hidden Command builder
src-tauri/src/media/probe.rs                ffprobe → MediaInfo
src-tauri/src/media/run.rs                  run ffmpeg with -progress parsing + cancellation
src-tauri/src/fsops.rs                      temp/final/unique names, commit, recycle
src-tauri/src/verify.rs                     post-encode verification
src-tauri/src/compress/plan.rs              pure ladder/bitrate planner
src-tauri/src/jobs/mod.rs                   Job, JobKind, JobEvent types
src-tauri/src/jobs/compress.rs              compress job
src-tauri/src/jobs/convert.rs               convert plan + job
src-tauri/src/jobs/queue.rs                 serial executor
src-tauri/src/shell_menu.rs                 registry layout write/remove
src-tauri/tests/common/mod.rs               fixture generator (uses bundled ffmpeg)
src-tauri/tests/*.rs                        integration tests named per task
```

---

### Task 1: Scaffold the Tauri + Svelte app with theme tokens

**Files:**
- Create: everything in File Structure except `media/`, `fsops.rs`, `verify.rs`, `compress/`, `jobs/`, and `shell_menu.rs` (stub `main.rs`/`app.rs` only)
- Create: `scripts/fetch-ffmpeg.ps1`
- Test: `src/progress/format.test.ts` (a placeholder test is fine until Task 9)

**Interfaces:**
- Produces:
  - Tauri app identifier `com.hasami.app`, product name `Hasami`.
  - No window is declared in `tauri.conf.json` (`app.windows: []`). Windows are created at runtime.
  - `bundle.externalBin: ["binaries/ffmpeg", "binaries/ffprobe"]`.
  - Bundle targets: `["nsis"]`, `nsis.installMode: "currentUser"`, `nsis.installerHooks: "nsis/hooks.nsh"`.
  - CSS custom properties in `src/lib/theme.css`: `--void --black --charcoal --smoke --graphite --bone --ash --pebble --iron --red --line --sans --serif` with the spec §9 values (`--line: #1f1e1f`, `--red: #d43545`), plus a `.cap` class (uppercase, 11px, `letter-spacing: 0.071em`).

- [ ] **Step 1:** Scaffold Svelte 5 + TS + Vite at the repo root and Tauri 2 in `src-tauri/`, then apply the config above. Write `fetch-ffmpeg.ps1` so it downloads one **pinned** ffmpeg release build (record the exact version URL and SHA256 in the script, and fail on mismatch). It copies the binaries to `src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe` and `ffprobe-x86_64-pc-windows-msvc.exe`. Add the binaries, `node_modules/`, `dist/`, and `src-tauri/target/` to `.gitignore`.
- [ ] **Step 2:** Download the font woff2 files and their OFL texts into `src/lib/fonts/`, and reference them via `@font-face` in `theme.css`. Nothing loads from Google Fonts at runtime.
- [ ] **Step 3: Verify.** Run `powershell.exe -File scripts/fetch-ffmpeg.ps1`, then `cargo.exe build` (in `src-tauri/`), `cmd.exe /c npm test`, and `cmd.exe /c npm run build`. All succeed.
- [ ] **Step 4: Commit.** `git commit -m "chore: scaffold Tauri 2 + Svelte app with theme tokens and ffmpeg fetch script"`

### Task 2: CLI action parsing

**Files:**
- Create: `src-tauri/src/cli.rs`
- Test: unit tests in `cli.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum Target { Mp4, Mov, Mp3 }
  pub enum Action { Trim(Vec<PathBuf>), Compress(Vec<PathBuf>), Convert(Target, Vec<PathBuf>), Register, Unregister }
  pub fn parse_args(args: &[String]) -> Result<Action, CliError>   // args excludes argv[0]
  ```
  Verbs: `trim`, `compress`, `convert-mp4`, `convert-mov`, `convert-mp3`, `register`, `unregister`.

- [ ] **Step 1: Write failing tests:**
  - `parse_compress_multiple`: `["compress", "C:\\a b\\x.mp4", "C:\\y.mkv"]` → `Compress` with both paths in order.
  - `parse_convert_mp3`: → `Convert(Target::Mp3, …)`.
  - `parse_register`: `["register"]` → `Register`.
  - `unknown_verb_errors`: `["explode"]` → `Err`.
  - `file_verb_without_files_errors`: `["compress"]` → `Err`.
- [ ] **Step 2:** Run `cargo.exe test cli::` and confirm the tests fail.
- [ ] **Step 3:** Implement `parse_args`.
- [ ] **Step 4:** Run `cargo.exe test cli::` and confirm the tests pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: parse hasami CLI actions"`

### Task 3: ffmpeg location, hidden commands, probe, and test fixtures

**Files:**
- Create: `src-tauri/src/media/{mod.rs,ffmpeg.rs,probe.rs}`, `src-tauri/tests/common/mod.rs`
- Test: `src-tauri/tests/probe.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct FfmpegPaths { pub ffmpeg: PathBuf, pub ffprobe: PathBuf }
  impl FfmpegPaths { pub fn resolve() -> Result<Self, MediaError> }
  pub fn hidden_command(program: &Path) -> std::process::Command  // sets CREATE_NO_WINDOW
  pub struct VideoStream { pub index: usize, pub codec: String, pub width: u32, pub height: u32, pub fps: f64, pub bit_rate: Option<u64> }
  pub struct AudioStream { pub index: usize, pub codec: String, pub channels: u32, pub sample_rate: u32, pub bit_rate: Option<u64> }
  pub struct MediaInfo { pub path: PathBuf, pub container: String, pub duration_s: f64, pub size_bytes: u64, pub bit_rate: Option<u64>, pub video: Option<VideoStream>, pub audio: Vec<AudioStream> }
  pub fn probe(paths: &FfmpegPaths, file: &Path) -> Result<MediaInfo, MediaError>
  ```
  - `resolve` order: env `HASAMI_FFMPEG_DIR`, then the current exe's directory (`ffmpeg.exe`), then `<crate>/binaries/ffmpeg-x86_64-pc-windows-msvc.exe` (dev).
  - `container` is the extension, lowercased without the dot.
  - `fps` comes from `avg_frame_rate`.
- Test fixtures (`tests/common/mod.rs`), all generated into a fresh temp dir with lavfi, at most 5 s each unless noted:
  - `fixture_multitrack_hevc(dir) -> PathBuf`: 1920×1080, 60 fps, libx265, 3 AAC stereo tracks (sine 440 Hz, `anullsrc`, sine 880 Hz).
  - `fixture_big_h264(dir, secs) -> PathBuf`: `testsrc2` + `noise=alls=60:allf=t`, `-b:v 60M`, so it's over 20 MB.
  - `fixture_video_only(dir)`.
  - `fixture_small_h264_mp4(dir)`: under 2 MB, one AAC track.
  - `fixture_six_tracks(dir)`: one sine at −12 dBFS plus 5 `anullsrc` tracks.

- [ ] **Step 1: Write failing tests:**
  - `probe_multitrack`: codec `"hevc"`, 1920×1080, fps within 0.01 of 60, `audio.len() == 3`, duration within 0.1 s of 5.
  - `probe_video_only`: `audio.is_empty()`.
  - `probe_missing_file_errors`.
- [ ] **Step 2:** Run `cargo.exe test --test probe` and confirm the tests fail.
- [ ] **Step 3:** Implement. `probe` runs `ffprobe -v error -print_format json -show_format -show_streams`. Every child process goes through `hidden_command`.
- [ ] **Step 4:** Run `cargo.exe test --test probe` and confirm the tests pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: locate bundled ffmpeg, probe media, add test fixtures"`

### Task 4: File operations (names, commit, recycle)

**Files:**
- Create: `src-tauri/src/fsops.rs`
- Test: unit tests in `fsops.rs` (temp dirs)

**Interfaces:**
- Produces:
  ```rust
  pub fn temp_path_for(final_path: &Path) -> PathBuf          // same dir, ".<file name>.hasami-tmp"
  pub fn unique_path(desired: &Path, allow_existing: Option<&Path>) -> PathBuf
      // returns `desired` if free or equal to allow_existing; else "<stem> (2).<ext>", "(3)", …
  pub fn with_extension(original: &Path, ext: &str) -> PathBuf  // same dir + stem, new ext
  pub fn commit_replace(temp: &Path, original: &Path, final_path: &Path) -> Result<(), FsError>
  pub fn recycle(path: &Path) -> Result<(), FsError>
  ```
  `commit_replace`:
  1. If `final_path == original`: rename `original` → `.<name>.hasami-old`, rename `temp` → `final_path`, recycle the `-old` file. If the second rename fails, rename `-old` back.
  2. Otherwise: rename `temp` → `final_path`, then recycle `original`.
  3. Any error leaves `original` (or its restored copy) at its original path and the temp file deleted.

- [ ] **Step 1: Write failing tests:**
  - `temp_path_is_hidden_sibling`: `C:\d\Replay 1.mp4` → `C:\d\.Replay 1.mp4.hasami-tmp`.
  - `unique_path_appends_counter`: when `x.mp4` and `x (2).mp4` exist, you get `x (3).mp4`.
  - `unique_path_allows_original`: `desired == allow_existing` → `desired`.
  - `commit_replace_same_name`: afterwards `final` has the temp content, and neither the temp nor the `-old` file exists.
  - `commit_replace_new_ext`: `a.mkv` + temp → `a.mp4` exists, `a.mkv` is gone.
  - **Review Focus 2,** `commit_fails_cleanly_when_original_locked`: hold `original` open via `OpenOptions` with `share_mode(0)` (`std::os::windows::fs::OpenOptionsExt`). `commit_replace` returns `Err`; the original's bytes are unchanged; the temp file has been removed.
- [ ] **Step 2:** Run `cargo.exe test fsops::` and confirm the tests fail.
- [ ] **Step 3:** Implement. `recycle` uses `trash::delete`. Map errors to `FsError` with a user-facing message, e.g. "Couldn't replace the file because another program is using it."
- [ ] **Step 4:** Run `cargo.exe test fsops::` and confirm the tests pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: safe temp/commit/recycle file operations"`

### Task 5: Output verification and the compress planner

**Files:**
- Create: `src-tauri/src/verify.rs`, `src-tauri/src/compress/{mod.rs,plan.rs}`
- Test: unit tests in `plan.rs`; `src-tauri/tests/verify.rs`

**Interfaces:**
- Consumes: `probe`, `MediaInfo` (Task 3).
- Produces:
  ```rust
  pub fn verify_output(paths: &FfmpegPaths, file: &Path, expected_duration_s: f64, max_bytes: Option<u64>) -> Result<MediaInfo, VerifyError>
  // tolerance = max(0.25 s, 1% of expected); size checked against max_bytes

  pub const SIZE_LIMIT_BYTES: u64 = 20_000_000;
  pub struct CompressPlan { pub width: u32, pub height: u32, pub fps: f64, pub video_bps: u64, pub audio_bps: u32, pub audio_mono: bool, pub warn_low_res: bool }
  pub enum CompressDecision { AlreadySmall, Encode(CompressPlan) }
  pub fn plan_compress(info: &MediaInfo) -> Result<CompressDecision, CompressError>   // CompressError::TooLong
  ```
- Algorithm (fixed by this plan):
  1. Total budget: `SIZE_LIMIT_BYTES * 8 * 0.96` bits.
  2. **AlreadySmall** if all of these hold: `size_bytes ≤ SIZE_LIMIT_BYTES`, container `mp4`, video codec `h264` (or no video), `audio.len() ≤ 1`, and that audio is `aac`.
  3. Audio: no audio streams → `0`. Otherwise take the first of `[128k stereo, 96k stereo, 64k stereo, 32k mono]` whose `bps × duration ≤ 10%` of the budget. If none qualifies, use `32k mono`.
  4. `video_bps = (budget − audio_bps × duration) / duration`. If that's < `50_000` → `TooLong`.
  5. Rungs, in order: `(1920,1080,60) (1920,1080,30) (1280,720,60) (1280,720,30) (854,480,30) (640,360,30) (426,240,15)`.
     - Skip rungs taller than the source, or with fps above the source fps. Keep the source's aspect ratio by scaling the width to the rung's height, rounded to even.
     - Choose the first rung where `video_bps / (w × h × fps) ≥ 0.05`. If none qualifies, use the last allowed rung.
  6. If the input is ≤ the size limit but not AlreadySmall, set `video_bps = min(video_bps, source video bit_rate)`.
  7. `warn_low_res = height < 480`.

- [ ] **Step 1: Write failing tests** (build `MediaInfo` values directly):
  - `plan_90s_1080p60`: duration 89.3, 1920×1080@60, 3 AAC tracks, 412 MB → `Encode`; 1280×720 @ 30, `audio_bps == 128_000`, `!warn_low_res`.
  - `plan_10min`: 600 s 1080p60 → 426×240 @ 15, `audio_bps == 32_000`, `audio_mono`, `warn_low_res`. No stereo option fits in 10% of the budget, so it falls back to 32k mono.
  - `plan_3h_too_long`: 10800 s → `Err(TooLong)`.
  - `plan_never_upscales`: a 1280×720@30 source of 30 s → `height ≤ 720`, `fps ≤ 30`.
  - `plan_already_small` (**Review Focus 4**): 2 MB mp4/h264/1 AAC → `AlreadySmall`. The same file as `mkv` → `Encode`, with `video_bps ≤` the source video bit_rate.
  - `plan_no_audio`: video-only → `audio_bps == 0`.
  - In `tests/verify.rs`: `verify_rejects_oversize` (`max_bytes = 1000` on a fixture → `Err`), `verify_rejects_wrong_duration` (expected 99 s → `Err`), `verify_accepts_good`.
- [ ] **Step 2:** Run `cargo.exe test plan:: --lib` and `cargo.exe test --test verify`, and confirm they fail.
- [ ] **Step 3:** Implement both modules.
- [ ] **Step 4:** Run the same commands and confirm they pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: output verification and 20 MB compress planner"`

### Task 6: ffmpeg runner with progress and cancellation

**Files:**
- Create: `src-tauri/src/media/run.rs`
- Test: `src-tauri/tests/run.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct CancelToken(Arc<AtomicBool>);   // Clone, cancel(), is_cancelled()
  pub fn run_ffmpeg(paths: &FfmpegPaths, args: &[OsString], total_s: f64, cancel: &CancelToken, on_progress: &mut dyn FnMut(f64)) -> Result<(), RunError>
  // RunError::{Cancelled, Failed { stderr_tail: String }}
  ```
  - Always prepend `-hide_banner -nostdin -y -progress pipe:1 -nostats`.
  - Parse `out_time_us=` lines into a fraction clamped to [0, 1].
  - Poll the cancel token at least every 200 ms. On cancel, kill the child and return `Cancelled`.
  - Keep the last 20 stderr lines for errors.

- [ ] **Step 1: Write failing tests:**
  - `run_reports_monotonic_progress_to_one`: transcode a 5 s fixture; the progress values are non-decreasing and the last is ≥ 0.99.
  - `run_cancel_kills` (**Review Focus 5, part 1**): start a transcode of `fixture_big_h264(dir, 20)`, cancel after the first progress callback, get `Err(Cancelled)` within 2 s.
  - `run_failure_has_stderr`: bad input path → `Failed` with a non-empty `stderr_tail`.
- [ ] **Step 2:** Run `cargo.exe test --test run` and confirm the tests fail.
- [ ] **Step 3:** Implement. Read stdout on a thread, collect stderr on another, and poll `try_wait` with the cancel check.
- [ ] **Step 4:** Run `cargo.exe test --test run` and confirm the tests pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: ffmpeg runner with progress parsing and cancellation"`

### Task 7: Compress job

**Files:**
- Create: `src-tauri/src/jobs/{mod.rs,compress.rs}`
- Test: `src-tauri/tests/compress.rs`

**Interfaces:**
- Consumes: Tasks 3–6.
- Produces:
  ```rust
  pub struct JobProgress { pub pass: u8, pub passes: u8, pub fraction: f64 }
  pub enum JobOutcome { Done { output: PathBuf, bytes: u64 }, Unchanged { reason: String }, NeedsConfirm { plan: CompressPlan } }
  pub fn compress(paths: &FfmpegPaths, input: &Path, confirmed_low_res: bool, cancel: &CancelToken, on_progress: &mut dyn FnMut(JobProgress)) -> Result<JobOutcome, JobError>
  ```
  Behavior:
  - `AlreadySmall` → `Unchanged { reason: "Already under 20 MB" }`.
  - `warn_low_res && !confirmed_low_res` → `NeedsConfirm` (nothing is written).
  - Otherwise run a two-pass libx264 encode (`-preset medium`, `-b:v video_bps`, `-pass 1/2` with a passlog in the system temp dir, cleaned up afterwards).
    - Video filter: `scale=w:h,fps=fps`.
    - Audio: `amix=inputs=N:normalize=0` over every audio stream when N > 1, a direct map when N == 1, and `-an` when N == 0. Encode with `aac -b:a audio_bps` (`-ac 1` when mono). Add `-movflags +faststart`.
  - Output is `temp_path_for(final)`, where `final = unique_path(with_extension(input, "mp4"), Some(input))`.
  - `verify_output(…, info.duration_s, Some(SIZE_LIMIT_BYTES))`. If the size check fails, re-encode with `video_bps × 0.9`, up to 3 retries. If all retries fail → `Err`.
  - Then `commit_replace(temp, input, final)`.
  - On any error or cancel, delete the temp file and the passlogs.

- [ ] **Step 1: Write failing tests:**
  - `compress_big_file_under_limit`: `fixture_big_h264(dir, 8)` → `Done`. The output is ≤ 20,000,000 bytes, the codec is h264, there's 1 audio stream, the original path now holds the new file (same name, since it was .mp4), and no `.hasami-tmp` files remain in `dir`.
  - `compress_multitrack_hevc_mixes_to_one`: 3 tracks in → 1 AAC track out.
  - `compress_mkv_becomes_mp4`: input `a.mkv` → `a.mp4` exists, `a.mkv` is gone.
  - `compress_awkward_name` (**Review Focus 1**): rename the fixture to `[CLIPPED] Ráplay 'x' (2).mp4` → `Done`, output at the same path.
  - `compress_video_only` (**Review Focus 3**): `Done`, 0 audio streams.
  - `compress_already_small` (**Review Focus 4**): `fixture_small_h264_mp4` → `Unchanged`, the file's bytes and mtime are unchanged.
  - `compress_low_res_needs_confirm`: a 600 s, 640×360 lavfi fixture (`testsrc2` at low bitrate so it generates fast) → `NeedsConfirm` and nothing is written; with `confirmed_low_res = true` → `Done`.
  - `compress_cancel_leaves_original` (**Review Focus 5, part 1**): cancel during pass 1 → `Err(Cancelled)`, the original's bytes are unchanged, no temp file remains.
  - `six_track_mix_keeps_level` (**Review Focus 3**): `fixture_six_tracks` → compress, then measure the output's `volumedetect` `mean_volume`; it's within 1 dB of the source's loud track.
- [ ] **Step 2:** Run `cargo.exe test --test compress` and confirm the tests fail.
- [ ] **Step 3:** Implement `compress`.
- [ ] **Step 4:** Run `cargo.exe test --test compress` and confirm the tests pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: compress job with 20 MB guarantee, retries, and safe replace"`

### Task 8: Convert jobs

**Files:**
- Create: `src-tauri/src/jobs/convert.rs`
- Test: unit tests (plan) in `convert.rs`; `src-tauri/tests/convert.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum ConvertPlan { Remux, ReencodeVideo, ExtractMp3 }
  pub fn plan_convert(info: &MediaInfo, target: Target) -> Result<ConvertPlan, JobError>
  pub fn convert(paths: &FfmpegPaths, input: &Path, target: Target, cancel: &CancelToken, on_progress: &mut dyn FnMut(JobProgress)) -> Result<JobOutcome, JobError>
  ```
- Rules (fixed by this plan):
  - **Mp4:** Remux if the video is in `{h264, hevc, av1}` (or there's no video) and every audio stream is in `{aac, mp3, opus, ac3}`.
  - **Mov:** Remux if the video is in `{h264, hevc, prores}` and every audio stream is in `{aac, pcm_s16le, pcm_s24le}`.
  - Otherwise ReencodeVideo: `libx264 -crf 18 -preset medium`, audio `aac -b:a 192k` per stream. Map **all** audio streams (`-map 0:v:0? -map 0:a?`). Remux uses `-c copy` with the same maps.
  - **Mp3:** ExtractMp3 = `amix=inputs=N:normalize=0` (or a direct map when N == 1), `libmp3lame -q:a 2`. No audio → `Err` with the message **"This video has no audio"** (Review Focus 3).
  - `target`'s extension equal to the input's → `Err("Already a .<ext> file")`. This is defensive; the menu hides it.
  - Output and commit work as in Task 7: `unique_path(with_extension(input, ext), Some(input))`, temp, `verify_output(…, duration, None)`, `commit_replace`.

- [ ] **Step 1: Write failing tests:**
  - `plan_hevc_mp4_to_mov_remux`; `plan_vp9_webm_to_mp4_reencode` (build the info directly); `plan_mp3_no_audio_errors`.
  - Integration: `convert_mp4_to_mov_keeps_all_tracks` (multitrack fixture → `.mov` with 3 audio streams, original recycled); `convert_webm_to_mp4` (lavfi VP9 + Opus fixture → `.mp4`, video codec h264); `convert_to_mp3_mixes` (multitrack → `.mp3` with 1 stream); `convert_video_only_to_mp3_errors`.
- [ ] **Step 2:** Run `cargo.exe test convert` and confirm the tests fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo.exe test convert` and confirm the tests pass.
- [ ] **Step 5: Commit.** `git commit -m "feat: convert to mp4/mov (remux or re-encode) and mp3"`

### Task 9: Serial job queue, single instance, and progress window

**Files:**
- Create: `src-tauri/src/jobs/queue.rs`, `src-tauri/src/app.rs` (replace the stub), `src/lib/jobs.ts`, `src/progress/{main.ts,ProgressWindow.svelte,format.ts}`, `progress.html`
- Modify: `src-tauri/src/main.rs`
- Test: unit tests in `queue.rs`; `src/progress/format.test.ts`

**Interfaces:**
- Consumes: `Action` (Task 2), `compress` (Task 7), `convert` (Task 8).
- Produces:
  ```rust
  pub enum JobKind { Compress, Convert(Target) }
  pub struct JobSpec { pub id: u64, pub kind: JobKind, pub input: PathBuf }
  #[serde(tag = "type")] pub enum JobEvent {
      Queued { snapshot: QueueSnapshot }, Started { id: u64, input: String, input_bytes: u64 },
      Progress { id: u64, pass: u8, passes: u8, fraction: f64, eta_s: Option<f64> },
      NeedsConfirm { id: u64, width: u32, height: u32, fps: f64 },
      Done { id: u64, output: String, bytes: u64 }, Unchanged { id: u64, reason: String },
      Failed { id: u64, message: String }, Drained }
  pub struct QueueSnapshot { pub current_index: usize, pub total: usize }      // "File 3 of 12"
  pub struct JobQueue { … }
  impl JobQueue { pub fn new(runner: Box<dyn JobRunner>, emit: Box<dyn Fn(JobEvent) + Send>) -> Self;
                  pub fn enqueue(&self, kind: JobKind, inputs: Vec<PathBuf>); pub fn cancel(&self, id: u64);
                  pub fn confirm(&self, id: u64, proceed: bool) }
  pub trait JobRunner: Send { fn run(&self, spec: &JobSpec, confirmed: bool, cancel: &CancelToken, progress: &mut dyn FnMut(JobProgress)) -> Result<JobOutcome, JobError>; }
  ```
  - Tauri event name: `"hasami://job"` carrying `JobEvent`.
  - Commands: `cancel_job(id: u64)` and `confirm_low_res(id: u64, proceed: bool)`.
  - TS mirrors these types exactly in `src/lib/jobs.ts`.
  - The single-instance plugin forwards argv to the running instance. `app.rs` collects actions that arrive within **400 ms** of each other into one `enqueue` call per kind.
  - `Trim` actions show the message "Quick Trim arrives in the next version" (temporary, until Plan 2).
  - The process exits when the queue emits `Drained` and no window is open.
- Progress window: label `progress`, 440×190, not resizable, created on the first job and reused. Matches the mockup at `docs/superpowers/specs/assets/quick-trim-mockup.html` (".mini" block):
  - Title "HASAMI *compress*" or "HASAMI *convert*".
  - The filename.
  - `<input size> → ≤ 20 MB · <W>×<H> · <fps> fps` (compress) or `→ .mov` (convert).
  - A 2px bar with bone fill.
  - `Pass 1 of 2 · 38% · about 0:24 left`, plus `File 3 of 12` when total > 1, and a **Cancel** button.
  - **NeedsConfirm** state: "This would end up at 426×240. Continue?" with **Continue** and **Cancel**.
  - **Failed** state: the message, the line "The original file was not changed.", and a **Close** button. The window stays open.
  - Closes itself 1.5 s after `Drained` when nothing failed.

- [ ] **Step 1: Write failing tests:**
  - Rust, with a fake `JobRunner` that records start/stop timestamps:
    - `queue_runs_serially`: 3 jobs never overlap, in enqueue order.
    - `enqueue_while_running_appends` (**Review Focus 5, part 2**): a second `enqueue` during job 1 runs after it, and the snapshot total becomes 2.
    - `cancel_current_moves_on`: cancel job 1 → `Failed { message: "Cancelled" }`, then job 2 starts.
    - `needs_confirm_waits_then_proceeds`.
    - `drained_emitted_once`.
  - TS: `formatBytes(412_000_000) === "412 MB"`, `formatBytes(19_400_000) === "19.4 MB"`, `formatEta(24) === "0:24"`, `formatEta(125) === "2:05"`, `formatPercent(0.384) === "38%"`.
- [ ] **Step 2:** Run `cargo.exe test queue::` and `cmd.exe /c npm test`, and confirm they fail.
- [ ] **Step 3:** Implement the queue (one worker thread plus a channel), the `app.rs` wiring, and the Svelte window. ETA is computed from the fraction rate over the current pass.
- [ ] **Step 4:** Run the same commands and confirm they pass.
- [ ] **Step 5: Manual check.** Run `cargo.exe tauri dev -- -- compress "C:\…\fixture.mp4"` against a copy of a real clip. The window matches the mockup, no console window flashes, and the file ends up ≤ 20 MB.
- [ ] **Step 6: Commit.** `git commit -m "feat: serial job queue, single-instance batching, progress window"`

### Task 10: Context menu registration and installer

**Files:**
- Create: `src-tauri/src/shell_menu.rs`, `src-tauri/nsis/hooks.nsh`
- Modify: `src-tauri/src/main.rs` (handle `Register`/`Unregister` and exit without starting the UI)
- Test: unit tests in `shell_menu.rs`

**Interfaces:**
- Produces:
  ```rust
  pub fn register(exe: &Path, classes_root: &str) -> Result<(), MenuError>     // classes_root default: r"Software\Classes"
  pub fn unregister(classes_root: &str) -> Result<(), MenuError>
  ```
- Layout. For each `ext` in `[mp4, mov, mkv, webm]`, under `HKCU\<classes_root>\SystemFileAssociations\.<ext>\shell\Hasami`:
  - `MUIVerb="Hasami"`, `SubCommands=""`, `Icon="<exe>,0"`.
  - Subkeys under `shell\`:
    - `01trim` ("Quick Trim", `"<exe>" trim "%1"`)
    - `02compress` ("Compress", `compress`)
    - `03mp4` ("Convert to .mp4", `convert-mp4`), **omitted for .mp4**
    - `04mov` ("Convert to .mov", `convert-mov`), **omitted for .mov**
    - `05mp3` ("Convert to .mp3", `convert-mp3`)
  - Each verb has `MUIVerb`, `MultiSelectModel="Player"`, and a `command` subkey whose default value is the command line above.
  - `unregister` deletes each `…\shell\Hasami` tree and ignores missing keys.
- `hooks.nsh`: `NSIS_HOOK_POSTINSTALL` runs `"$INSTDIR\hasami.exe" register`; `NSIS_HOOK_PREUNINSTALL` runs `"$INSTDIR\hasami.exe" unregister`.

- [ ] **Step 1: Write failing tests** (with `classes_root = r"Software\HasamiTest\Classes"`, cleaned up in a drop guard):
  - `register_writes_cascade`: the `.mkv` menu has `MUIVerb == "Hasami"` and all 5 verbs; `02compress\command` default equals `"C:\x\hasami.exe" compress "%1"`.
  - `mp4_menu_omits_convert_mp4`: the `.mp4` menu has no `03mp4`, and `.mov` has no `04mov`.
  - `unregister_removes_all_and_is_idempotent`: calling it twice is `Ok`, and the keys are gone.
- [ ] **Step 2:** Run `cargo.exe test shell_menu::` and confirm the tests fail.
- [ ] **Step 3:** Implement with `winreg`, and wire up `main.rs`.
- [ ] **Step 4:** Run `cargo.exe test shell_menu::` and confirm the tests pass.
- [ ] **Step 5: Build and smoke test.** Run `cargo.exe tauri build`, install the produced NSIS setup per-user, and check:
  - Right-clicking a copy of a real clip in Explorer shows **Hasami ▸** with the right items.
  - Selecting 3 copies → **Compress** gives one progress window showing "File 1 of 3", and all 3 end up ≤ 20 MB with the originals in the Recycle Bin. No Windows dialog appears at any point, including the Recycle Bin.
  - Uninstalling removes the menu.
- [ ] **Step 6: Commit.** `git commit -m "feat: register Explorer context menu via NSIS hooks"`

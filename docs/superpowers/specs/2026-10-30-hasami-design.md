# Hasami — Design

**Date:** 2026-10-30
**Status:** Approved by Dang (2026-10-30)
**Mockup:** [assets/quick-trim-mockup.html](assets/quick-trim-mockup.html), the approved Quick Trim editor and progress window. Open it in a browser. Click a palette under the editor to see the alternatives that were considered; **Sage + ochre** is the chosen one.

---

## 1. Why this exists

Dang records gameplay with OBS's replay buffer. `C:\Users\ledan\Videos\Clips` holds **600 raw clips totaling 530 GB**, about 884 MB each. Most clips hold a few seconds worth keeping inside ~90 seconds of footage. Trimming them in DaVinci Resolve is too heavy for the job, so the raws pile up.

Hasami (Japanese for "scissors") is a small Windows app that lives in Explorer's right-click menu. You right-click a clip, cut it down to the moment, and it replaces the raw. It can also squeeze any video under Discord's 20 MB upload limit in one click, and convert between common formats.

**Typical session:** select a batch of raw clips → right-click → **Hasami ▸ Quick Trim** → for each clip, find the play, cut, then Quick Export, Discard, or Skip → the next clip loads → repeat.

### Success looks like

- From right-click to a finished trimmed clip in well under a minute. The window opens fast enough that it feels like part of Explorer.
- A trimmed clip looks the same as the raw.
- Compress **always** produces a file of 20 MB or less, whatever the input.
- A raw is never lost because an export failed.

## 2. Goals, non-goals, anti-goals

**Goals**
- Explorer right-click submenu **Hasami ▸** for `.mp4 .mov .mkv .webm`, offering Quick Trim, Compress, and Convert to .mp4, .mov, or .mp3.
- A minimal, DaVinci-like trim editor: one video track, independent audio tracks, cut, delete, extend, snap, per-track volume and mute.
- Batch operation when several files are selected.

**Non-goals (v1)**
- Multi-clip projects, titles, effects, transitions, fades, color, speed changes.
- Moving clips along the timeline. Clips can only be cut, deleted, and edge-dragged.
- Renaming tracks, a settings screen, or a configurable size limit (20 MB is fixed).
- The Windows 11 compact context menu. Dang uses the classic menu, and it's already enabled on the machine.
- macOS and Linux.
- HandBrake. Dang suggested it but explicitly made it optional; the requirement is the ≤20 MB guarantee and that no third-party UI ever appears.

**Anti-goals (failure even if it "works")**
- Deleting or moving an original before its replacement is fully written and verified.
- A Compress result over 20 MB, ever.
- Any external tool's window appearing (HandBrake, a console window, etc.).
- A sluggish launch that makes right-click → edit feel slower than opening DaVinci.
- Quick Export output that's visibly worse than the source.

## 3. Facts about Dang's setup (from recon, 2026-10-30)

| | |
|---|---|
| OS | Windows 11 Pro, classic context menu enabled (the `{86ca1aa0-…}` InprocServer32 override is set) |
| GPU / CPU | AMD Radeon RX 7900 GRE (AMF encoders: `hevc_amf`, `h264_amf`, `av1_amf`) · Ryzen 7 7800X3D · plus a Parsec virtual display adapter |
| Toolchain on Windows | Rust via scoop rustup (`cargo.exe`, `rustc.exe`), Node.js |
| Clips | 597 `.mp4`, 3 `.webm`. Sampled clips: **HEVC Main, 1920×1080, 60 fps, yuv420p**, ~86–90 s long |
| Video bitrate | Varies widely: **0.6 Mbps to 152 Mbps**. Some 90 s clips are 1.4–1.6 GB |
| Audio | **3 to 6 AAC-LC stereo 48 kHz tracks** per clip. None are named (`handler_name=SoundHandler`, no title). Several are **empty** (~2 kbps average bitrate) |
| Naming | OBS default: `Replay YYYY-MM-DD HH-MM-SS.mp4` |

Design for the general case too: H.264, HEVC, AV1, VP9, ProRes in `.mov`, 1 to 6+ audio tracks.

## 4. Right-click menu

Classic-menu cascading entry **Hasami ▸** on `.mp4`, `.mov`, `.mkv`, `.webm`:

- **Quick Trim**: opens the editor (§5). With several files selected, it opens a trim queue (§5.8).
- **Compress**: shrinks the file to 20 MB or less (§7) and replaces it.
- **Convert to .mp4** / **Convert to .mov** / **Convert to .mp3**: replaces the file with the new format.

Behavior:
- **Replace semantics (Dang's choice):** Compress and Convert replace the original. The original goes to the **Recycle Bin**, never a permanent delete, and only after the new file is verified (§8).
- **Naming:**
  - Compress keeps the filename. The extension becomes `.mp4` if it wasn't already (*my call*).
  - Convert keeps the filename and changes only the extension (*my call*).
  - Convert to .mp3 mixes all audio tracks into one stereo track. The original video goes to the Recycle Bin, because Dang chose replace.
- *My call:* the menu hides "Convert to .mp4" on `.mp4` files, and so on for each format.
- *My call:* Convert to .mp4/.mov **remuxes** (no re-encode) when the codecs are legal in the target container, and re-encodes otherwise. It keeps all audio tracks.
- **Multi-select:** Compress and Convert run on every selected file, one after another, in a single progress window showing "File 3 of 12".
- **Progress window (Dang's choice: a small window).** Shown in the mockup. It contains:
  - the app wordmark with the action ("HASAMI *compress*") and the filename;
  - input size → target (e.g. "412 MB → ≤ 20 MB"), plus the chosen output resolution and fps;
  - a progress bar, the pass number, percent, time remaining, and a **Cancel** button.

  The window closes itself on success. On failure it stays open and shows the error, and the original is untouched.
- **Install:** registered per-user under `HKCU` (no admin rights). The uninstaller removes the keys.

## 5. Quick Trim editor

Layout follows the mockup: a top export bar, the viewer with transport controls, the timeline toolbar, track headers on the left with lanes to the right, and a status bar.

### 5.1 Top bar
- Wordmark: **HASAMI** *quick trim*.
- Filename, plus a red dot and "EDITED" once there are unsaved edits.
- Metadata line: resolution · fps · codec · audio track count · duration · size.
- Right side:
  - the **Fit to 20 MB** toggle;
  - **Export audio** (text link);
  - **Export + save raw** (outlined button);
  - **Quick export** (outlined button with a red border; the primary action);
  - window controls.
- In queue mode, add "3 of 30" plus **Skip** and **Discard** (§5.8).

### 5.2 Viewer and transport
- The video sits on pure black.
- Transport: current timecode (`HH:MM:SS:FF`), jump to start, previous frame, play/pause, next frame, jump to end, and total timeline duration.
- A speed badge (e.g. "2×") appears while shuttling.
- Playback follows the **edited timeline**, skipping deleted ranges seamlessly, with each audio track's volume and mute applied live.

### 5.3 Timeline model
- **Video track (V1), exactly one:** always contiguous from 0 to the end of the last segment. Deleting a segment closes the gap (ripple). Trimming the end removes that time from the timeline.
- **Audio tracks (A1…An):** one per *non-empty* source audio track, labeled A1, A2… (sources have no names).
  - Audio segments are independent of each other and of the video.
  - Deleting an audio segment on its own **leaves silence** and doesn't ripple, so sync holds.
  - Audio can run past the end of the video. That stretch is shown hatched and dimmed, labeled "Video ends", and dropped on export.
- **Empty source tracks (my call):** detected on open and hidden behind a "+N empty tracks" toggle under the last audio track.
- **Ctrl+B** cuts every track at the playhead.
- **Delete** removes the selection.
- **Dragging a segment edge** restores previously deleted source material (extends the segment).
  - Video extends ripple the following video segments.
  - *My call:* an audio edge can't overlap a neighboring audio segment; it stops at the neighbor.
  - Nothing can extend past the source's real start or end.
- **Linked selection (Dang's choice: linked, with a toggle):** on by default. Clicking a video segment also selects the audio pieces created by the same cuts. Deleting or edge-dragging applies to all of them, so sync is kept. With the toggle off, video and audio are selected and edited separately, and desync is the user's call.
- **Snapping (Dang's choice):** to the playhead, to cut points and segment edges on any track, and to the end of the video. There's a toolbar toggle. No time-grid snapping.
- **Zoom:** Ctrl+scroll, plus a zoom slider in the toolbar.
- **Undo/redo** covers every edit: cuts, deletes, edge drags, volume, mute, and toggles.

### 5.4 Track headers
- Tag (V1, A1…), a small color square for audio tracks, and the track name.
- For audio tracks: a mute button and one horizontal volume slider with a dB readout. *My call:* range −∞ to +6 dB, default 0 dB.
- Muted tracks render in grey and are left out of exports.

### 5.5 Keyboard (Dang asked to match DaVinci Resolve)
| Key | Action |
|---|---|
| Space | Play / pause |
| J / K / L | Shuttle reverse / stop / forward. Repeated taps on J or L go 1× → 2× → 4× (*my call:* → 8× max, like DaVinci) |
| ← / → | One frame back / forward |
| Shift + ← / → | One second back / forward (DaVinci default) |
| ↑ / ↓ | Previous / next cut point (DaVinci default) |
| Home / End | Timeline start / end |
| Ctrl+B | Cut at playhead, all tracks |
| Shift+V | Select the video clip under the playhead, with its linked audio when Linked is on (on a cut, the clip that starts there; no video there, no change). *Not a Resolve default: added in the Task 12 accessibility review so clips can be selected and deleted without a mouse.* |
| Delete / Backspace | Remove selection |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| N | Toggle snapping (DaVinci default) |
| Ctrl+Shift+L | Toggle linked selection (DaVinci default) |
| Ctrl+scroll | Zoom timeline |

### 5.6 Status bar
Cut count and selection. A live **estimated export size** at source quality, and with Fit to 20 MB on. Abbreviated shortcut hints.

### 5.7 Exports from the editor
All exports render the edited timeline. Every **unmuted** audio track is mixed at its set volume into **one** stereo track.

| Button | Output | Original |
|---|---|---|
| **Quick export** | `[CLIPPED] <name>.<ext>` beside the original | Recycle Bin, after verify |
| **Export + save raw** | `[CLIPPED] <name>.<ext>` beside the original | Moved into a `Raw\` folder next to the original (created if missing) |
| **Export audio** | `[CLIPPED] <name>.mp3` beside the original (*my call on the name*) | Untouched. The editor stays open |

- **"Identical to source" (Dang's choice) applies to both video exports:**
  - re-encode with the **same video codec, resolution, and frame rate** at about the **source's average video bitrate**, for frame-accurate cuts with no visible loss;
  - use the hardware encoder when available (AMF on this machine; NVENC or QSV elsewhere), with software fallback;
  - keep the source container. *Builder's call* for codec/container combos that need it, e.g. Opus audio in `.webm`.
  - Expect big files from high-bitrate raws: a 10 s trim of a 152 Mbps clip is about 190 MB. That's accepted.
- **Fit to 20 MB** toggle (Dang's choice: one toggle that applies to both video export buttons). It switches the encode to the Compress pipeline (§7), so the output is `[CLIPPED] <name>.mp4` at 20 MB or less.
- **Exports run in the background (Dang's choice).** Clicking a video export hands the job to Hasami's job queue (§10) right away. The window then closes, or loads the next clip in queue mode, without waiting. The original is only recycled or moved after its export verifies (§8).
- **Export audio** also stops at the end of the video, consistent with every other export (*my call*).
- Closing with unsaved edits asks for confirmation (*my call*).
- *My call:* the `[CLIPPED]` prefix is never doubled. A name collision appends " (2)", " (3)", and so on. If the source already sits in a folder named `Raw`, outputs go to that folder's parent and Save Raw leaves the original where it is.

### 5.8 Trim queue (multi-select → Quick Trim)
- Opens the first file, with "3 of 30" in the top bar.
- After Quick Export or Export + save raw, the next file loads.
- **Skip** leaves the current file untouched and moves on.
- **Discard** sends the current raw to the Recycle Bin and moves on (Dang's choice).
- While exports are still running, a small strip in the top bar shows them (e.g. "EXPORTING 2").
- After the last file, the window switches to a short summary instead of closing if any export is still running or has failed. It lists each file's result; a failed export says what went wrong and confirms the raw was kept. If everything is done and succeeded, it closes.

## 6. Preview and media pipeline (guidance; details are the builder's)

- **Video preview:** play the source directly in WebView2's `<video>` when it can decode it. HEVC needs hardware support or the HEVC extension, and this machine should have it. If it can't, generate a fast temporary H.264 proxy and show a brief "Preparing preview" state.
- **Audio preview:** WebView2 plays only the default audio track and has no per-track control. So, on open, extract each non-empty audio track to its own temp file (stream copy is near-instant) and play them through Web Audio, kept in sync with a muted video element. This is what makes per-track volume and mute work during playback.
- **Waveforms and thumbnails:** precompute with ffmpeg on open. Show the UI immediately and fill these in as they arrive.
- **Empty-track detection:** e.g. a volume or peak analysis. Treat near-silent tracks as empty.
- **Temp files:** live in a per-session temp folder, deleted on close.
- **Exports:** run ffmpeg in the background with progress in the window. No console windows ever appear.

## 7. Compress and Fit to 20 MB

**Requirement:** output is **≤ 20 MB, always**. *My call:* define that as ≤ 20,000,000 bytes, the stricter of MB and MiB.

- **Output format (my call):** H.264 + AAC in `.mp4`, so it plays inline on Discord on any device. All unmuted tracks are mixed into one, since Discord only plays the first track anyway.
- **Approach (builder may refine):**
  1. Compute the total bit budget with a container-overhead margin.
  2. Reserve audio: ~128 kbps, dropping to 96 or 64 kbps for long inputs.
  3. Pick resolution and frame rate from a ladder by bits-per-pixel: 1080p60 → 1080p30 → 720p60 → 720p30 → 480p30 → lower.
  4. Encode with an accurate rate-controlled method (e.g. two-pass libx264; the 7800X3D makes this quick).
  5. **Verify the file size and retry at a lower bitrate if over.**
- **Warn first (Dang's choice):** if the chosen resolution would fall **below 480p** (*my call on the threshold*), show the resulting resolution before encoding and let the user cancel or continue. Otherwise Compress just runs.
- **Never invoke HandBrake.**
- **Floor (my call):** the ladder bottoms out at about 240p / 15 fps, with audio stepping down to 64 kbps and then 32 kbps mono. If even the floor can't fit in 20 MB (only multi-hour inputs), Compress fails with a clear message and leaves the original untouched. It never produces an oversize file.

## 8. File safety rules (non-negotiable)

1. Write output to a temp file in the same folder (e.g. `.<name>.hasami-tmp`).
2. Verify it: ffprobe opens it, the duration matches the expected timeline length within tolerance, and for size-limited encodes the size is ≤ the limit.
3. Only then rename the temp file to its final name, and only after that move or recycle the original. When the output takes the original's own name, the order is: check the original can be deleted → recycle the original under its real name (so Restore brings back the right file) → rename the temp file with a non-replacing rename. If that rename fails after the original is recycled, the new file is kept where it is and the error says where it is and that the original is in the Recycle Bin. Plan 2's Quick Export reuses this same commit routine (`fsops::commit_replace`).
4. On any failure or cancel, delete the temp file and leave the original exactly as it was. Report the error in plain language.
5. A file locked by another program fails cleanly with a message. Nothing is half-done.
6. **Recycle Bin limits (Dang's choice):** the Recycle Bin is treated as a short undo window, not a backup. If Windows drops old raws once the bin fills, that's accepted. Hasami itself always recycles silently and never calls a permanent delete. For a file that's too big for the bin, or a drive with no bin, use whatever Windows does with no prompt shown. If Windows would show a dialog, the builder should suppress it so no UI appears (*builder's call on the exact flags*).

## 9. Visual design

- **Visual authority:** `docs/branding/DESIGN.md` ("black gallery wall, blood-red punctuation"), adapted for a dense, task-focused editor. `docs/frame.io/` was tried first and rejected; don't use it.
- **Approved deviations and specifics (all from the mockup review):**
  - Red muted to **`#d43545`**, replacing `#fe1e34`. Use it in exactly three places: the playhead, the **Quick export** border, and the "edited" dot.
  - Selected clips get a **bone-white** 1.5px outline, not red.
  - **Audio track colors:** A1 sage `#7fa58c`, A2 ochre `#c9a063`, A3 dusty blue `#7f9cc4`, A4 mauve `#b08bb3`, A5 teal `#4fa8a2`, A6 amber `#d89a3e` (*my call for A3+*). Clip fill ≈ 15% of the track color over near-black, a 1px border at ≈ 38%, and the waveform near full color. Selected clips are brighter.
  - **Fonts (Dang's choice: free stand-ins):** **Inter** for all UI, and **Instrument Serif** italic only in the wordmark connective ("quick trim", "compress"). Both are OFL and bundled, so no network font loading.
  - Small labels are uppercase and letter-spaced, at a **minimum 11px**. Timecodes use tabular numerals.
  - No gradients, glows, or filled buttons. Buttons are 8px-radius outlines with a drawn diagonal-arrow icon. Small controls use a 3px radius.
  - Surfaces: void `#080808`, viewer `#000`, headers `#171617`, dividers ~`#1f1e1f`. Text: bone `#fcfcfc`, pebble `#b5b2b2` for secondary.
- **Front-end process:** during implementation, use the `impeccable` skill (run its init to capture PRODUCT.md, keep to the craft floor, run its detector on UI changes) and review against the Vercel web-design-guidelines. Theme the browser defaults the user can still see: scrollbars, focus rings, selection.

## 10. How it's built

- **Tauri 2** (Rust backend, web UI in WebView2), Windows only (*Dang approved this as my recommendation*). Small install, fast start.
- **Frontend framework:** builder's choice, in TypeScript.
- **ffmpeg + ffprobe** bundled as sidecar binaries, run hidden.
- **Single-instance handling:** Explorer launches one process per selected file. Aggregate them through Tauri's single-instance mechanism, with a short collection window, into one batch or queue.
- **One running instance, one job queue (my call):**
  - Every right-click goes to the single running Hasami process.
  - **Quick Trim** files that arrive while an editor is open are added to the end of its queue; otherwise a new editor opens.
  - **Compress, Convert, and background exports** from the editor all go into one shared job queue. Jobs run **one at a time** so encodes don't fight over the GPU or disk.
  - Compress and Convert jobs show in the small progress window, which lists what's queued.
  - The process stays alive until the queue is empty, even after every window has closed.
- **Installer:** NSIS, per-user, registers the context menu, with uninstall support. No code signing for now.
- **Dev setup (Dang's choice):** the repo lives at `C:\Users\ledan\Documents\Projects\hasami` and builds and runs with the **Windows** Rust/Node toolchain, so testing uses the real WebView2, codecs, and right-click menu. Agents working from WSL use the `/mnt/c/...` path with Windows interop (`cargo.exe`, `npm.cmd`).
- **Audience:** just Dang for now, open source later. So:
  - no hardcoded user paths or secrets;
  - license-clean dependencies: OFL fonts; ffmpeg build licensing must be chosen knowingly (a GPL build is required for libx264, and the binary is shipped as a separate executable);
  - a README-level build guide when it goes public.

## 11. Left to the builder

- Frontend framework, state management, and project structure.
- Exact ffmpeg arguments, encoder selection logic, and ladder thresholds, within §7's guarantees.
- Preview proxy strategy and audio sync implementation, within §6.
- Empty-track detection threshold.
- Exact timing of the multi-select collection window.
- Test strategy. Expect real-media fixtures, including a multi-track HEVC sample and a high-bitrate one.

## 12. Calls made without Dang's explicit input (review these)

1. Compress keeps the filename; Convert changes only the extension; both send the original to the Recycle Bin.
2. Each "Convert to" item is hidden on files already in that format; Convert remuxes when possible and keeps all audio tracks.
3. Empty audio tracks are hidden behind a toggle; tracks are labeled A1…An.
4. Audio edges can't overlap their neighbors; volume range −∞ to +6 dB.
5. J/L shuttle tops out at 8×; Shift+arrows, ↑/↓, N, and Ctrl+Shift+L follow DaVinci defaults.
6. Export audio is named `[CLIPPED] <name>.mp3` and keeps the editor open.
7. After a video export the window closes (or the queue advances); closing with unsaved edits asks first.
8. Never double the `[CLIPPED]` prefix; collisions get " (2)"; sources already in a `Raw` folder export to its parent.
9. Fit to 20 MB and Compress output is H.264/AAC `.mp4` with one mixed audio track, at ≤ 20,000,000 bytes.
10. Warn before Compress when the output would fall below 480p.
11. Audio colors for tracks A3–A6.
12. Compress floor at ~240p/15 fps and 32 kbps mono; it fails with a message only if even that can't fit.
13. Export audio stops at the video's end like the other exports.
14. One running Hasami process: new Quick Trim files join the open queue, and every encode runs one at a time through a shared job queue.
15. After a trim queue ends, a summary screen appears only if exports are still running or any failed.

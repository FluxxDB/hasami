# Product

<!-- impeccable:product-schema 1 -->

> Written during Plan 2 Task 9 from [the design spec](specs/2026-10-30-hasami-design.md) §1–2 (approved 2026-10-30). No interview was possible from inside the task, so everything here is taken from the approved spec; lines marked *(inferred)* are the implementer's reading and should be confirmed.

## Platform

web *(inferred: a Tauri 2 desktop app for Windows whose windows are WebView2 pages; the design language is its own, not native Windows)*

## Users

People who record gameplay with OBS's replay buffer and end up with a folder of hundreds of raw clips, often close to 1 GB each, where most hold only a few seconds worth keeping inside ~90 s of footage. They work through them in batches from Explorer's right-click menu. One primary user at first *(inferred: no other audience is named in the spec)*.

## Product Purpose

Hasami ("scissors") is a small Windows app that lives in Explorer's right-click menu. Right-click a clip, cut it down to the moment, and it replaces the raw. It also squeezes any video under Discord's 20 MB upload limit in one click, and converts between common formats.

Success:
- right-click to a finished trimmed clip in well under a minute; the window opens fast enough to feel like part of Explorer;
- a trimmed clip looks the same as the raw;
- Compress always produces a file of 20 MB or less;
- a raw is never lost because an export failed.

## Positioning

Trimming in DaVinci Resolve is too heavy for this job, so the raws pile up. Hasami is the DaVinci-like trim that is only a right-click away: one video track, independent audio tracks, cut, delete, extend, snap, per-track volume and mute, and nothing else.

## Operating Context

- Typical session: select a batch of raw clips → right-click → **Hasami ▸ Quick Trim** → for each clip, find the play, cut, then Quick export, Discard or Skip → the next clip loads.
- Keyboard follows DaVinci Resolve (J/K/L, Ctrl+B, arrows, Home/End), at the owner's request.
- Results go to Discord (hence the 20 MB limit) or stay on disk as `[CLIPPED] <name>`.

## Capabilities and Constraints

- Explorer submenu **Hasami ▸** for `.mp4 .mov .mkv .webm`: Quick Trim, Compress, Convert to .mp4, .mov or .mp3.
- Batch operation when several files are selected.
- Non-goals (v1): multi-clip projects, titles, effects, transitions, fades, color, speed changes; moving clips along the timeline; renaming tracks; a settings screen; a configurable size limit; the Windows 11 compact menu; macOS and Linux; HandBrake.
- Anti-goals: deleting or moving an original before its replacement is written and verified; a Compress result over 20 MB; any external tool's window appearing; a sluggish launch; a Quick export visibly worse than the source.

## Brand Commitments

- Name: Hasami. Wordmark "HASAMI *quick trim*" / "HASAMI *compress*".
- Visual authority is [`docs/branding/DESIGN.md`](../branding/DESIGN.md) plus spec §9 and the approved mockup [`specs/assets/quick-trim-mockup.html`](specs/assets/quick-trim-mockup.html) (recorded here only as references; the visual rules live there).

## Evidence on Hand

- The approved mockup above. No testimonials, metrics or third-party proof exist, and none should be invented.

## Product Principles

*(inferred from §1–2)*
1. Never lose a raw: safety beats speed whenever they conflict.
2. Fast enough to feel like part of Explorer.
3. Do one job: cut a clip down; refuse editor scope creep.
4. Match what users already know from DaVinci instead of inventing new conventions.

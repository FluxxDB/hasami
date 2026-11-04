<h1 align="center">Hasami</h1>

<p align="center">
  Trim, compress and convert videos straight from Explorer's right-click menu.
</p>

<p align="center">
  <a href="https://github.com/FluxxDB/hasami/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/FluxxDB/hasami"></a>
  <a href="#license"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue"></a>
</p>

<p align="center">
  <img src="docs/screenshots/editor.png" width="900" alt="The Quick Trim editor: viewer, transport and a timeline with one video track and two audio tracks">
</p>

Hasami (はさみ, "scissors") is a small Windows app for `.mp4`, `.mov`, `.mkv` and
`.webm` files. Right-click a clip, pick **Hasami ▸**, and it does the job without
opening an editor you have to learn.

## What it does

- **Quick Trim:** a DaVinci-like trim editor with per-track volume, master volume and Dim, that exports a Discord-safe H.264 `.mp4`, removes black borders, and can save a 20 MB copy.
- **Compress:** writes a `[20MB]` copy of the video, always 20 MB or less, to your Videos folder and keeps the original.
- **Convert:** to `.mp4`, `.mov` or `.mp3`, remuxing without re-encoding when it can. The original goes to the Recycle Bin only after the new file checks out.

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/export.png" alt="An export in progress in the editor's top bar"></td>
    <td width="25%"><img src="docs/screenshots/menu.png" alt="The Hasami submenu in Explorer's right-click menu"></td>
    <td width="25%"><img src="docs/screenshots/progress.png" alt="The small Compress progress window"></td>
  </tr>
  <tr>
    <td align="center">Exporting from the editor</td>
    <td align="center">The right-click menu</td>
    <td align="center">Compress progress</td>
  </tr>
</table>

Hasami never permanently deletes a file. Every output is written to a temp file,
checked with ffprobe, and only then put in place; replaced originals go to the Recycle Bin.

## Install

1. Download `Hasami_<version>_x64-setup.exe` from [Releases](https://github.com/FluxxDB/hasami/releases/latest).
2. Run it. It installs for your user only, with no admin prompt.
3. The installer isn't code-signed yet, so SmartScreen may warn you: click **More info → Run anyway**.

Then right-click a video. On Windows 11 the entries are under **Show more options → Hasami**.
Uninstalling from Settings › Apps removes the menu entries too.

Hasami is built for Windows 11 x64 (Windows 10 should work) and runs on the WebView2 runtime that ships with Windows.

## Editor shortcuts

The keys follow DaVinci Resolve.

| Key | Action | Key | Action |
|---|---|---|---|
| Space | Play / pause | Ctrl+B | Cut at the playhead |
| J / K / L | Shuttle back / stop / forward | Delete | Remove the selection |
| ← / → | One frame back / forward | Shift+V | Select the clip under the playhead |
| Shift+← / → | One second back / forward | Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| ↑ / ↓ | Previous / next cut | N | Toggle snapping |
| Home / End | Timeline start / end | Ctrl+Shift+L | Toggle linked selection |

## How it works

A Rust backend (Tauri 2) owns every file and every ffmpeg run, and two small Svelte windows talk to it through commands and events. More detail, including a Quick export step by step, is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

```mermaid
flowchart TB
  subgraph win["Windows"]
    explorer["Explorer<br/>right-click ▸ Hasami"]
    registry[("HKCU registry<br/>SystemFileAssociations")]
    installer["NSIS installer<br/>hooks.nsh"]
  end

  subgraph rust["Rust backend · hasami.exe"]
    cli["cli.rs<br/>verb + files"]
    single["single-instance<br/>+ 400 ms collector"]
    flow["app/flow · dispatcher<br/>windows, queue, exit rule"]
    commands["app/commands<br/>Tauri commands"]
    events["app/events<br/>JobEvent · EditorEvent"]
    queue["jobs/queue<br/>serial job queue"]
    jobs["jobs/compress · convert · export<br/>compress/plan (20 MB ladder)"]
    sessions["editor/session<br/>per-clip work folders"]
    prep["editor/media · border · peaks<br/>tracks, waveforms, thumbs, proxy"]
    graph_["editor/request<br/>filter graph, H.264 encoder"]
    folder["editor/folder<br/>Prev / Next by date"]
    media["media/run · probe<br/>hidden ffmpeg / ffprobe"]
    fsops["fsops + verify<br/>temp → verify → commit → recycle"]
    menu["shell_menu.rs<br/>register / unregister"]
  end

  subgraph ui["Svelte windows"]
    editor["Quick Trim editor<br/>EditorStore · timeline · history"]
    engine["engine.ts<br/>video + Web Audio per track"]
    progress["Progress window<br/>Compress / Convert"]
  end

  subgraph ext["Bundled tools and outputs"]
    ffmpeg[["ffmpeg / ffprobe<br/>(GPL sidecars)"]]
    files[("[CLIPPED] · [20MB]<br/>Raw folder · Recycle Bin")]
  end

  installer --> menu --> registry
  explorer --> registry
  registry -- "hasami.exe trim | compress | convert-*" --> cli
  cli --> single --> flow
  flow -- "Compress / Convert" --> queue
  flow -- "Quick Trim" --> editor
  editor <-- "invoke" --> commands
  commands --> sessions --> prep
  commands --> folder
  commands -- "editor_export" --> queue
  queue --> jobs
  jobs --> graph_
  jobs --> media
  prep --> media
  media --> ffmpeg
  jobs --> fsops --> files
  queue --> events
  events -- "hasami://job" --> progress
  events -- "hasami://editor" --> editor
  editor --> engine

  click cli "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/cli.rs"
  click single "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/app/collector.rs"
  click flow "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/app/flow.rs"
  click commands "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/app/commands.rs"
  click events "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/app/events.rs"
  click queue "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/jobs/queue.rs"
  click jobs "https://github.com/FluxxDB/hasami/tree/main/src-tauri/src/jobs"
  click sessions "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/editor/session.rs"
  click prep "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/editor/media.rs"
  click graph_ "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/editor/request.rs"
  click folder "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/editor/folder.rs"
  click media "https://github.com/FluxxDB/hasami/tree/main/src-tauri/src/media"
  click fsops "https://github.com/FluxxDB/hasami/tree/main/src-tauri/src/fsops"
  click menu "https://github.com/FluxxDB/hasami/blob/main/src-tauri/src/shell_menu.rs"
  click installer "https://github.com/FluxxDB/hasami/blob/main/src-tauri/nsis/hooks.nsh"
  click editor "https://github.com/FluxxDB/hasami/blob/main/src/editor/store.svelte.ts"
  click engine "https://github.com/FluxxDB/hasami/blob/main/src/editor/engine.ts"
  click progress "https://github.com/FluxxDB/hasami/blob/main/src/progress/ProgressWindow.svelte"

  classDef red stroke:#d43545,stroke-width:2px
  class fsops,queue red
```

## Build from source

You need Windows 10/11 x64, [Rust](https://rustup.rs) 1.89 or newer (MSVC toolchain),
[Node.js](https://nodejs.org) 20 and PowerShell.

```powershell
git clone https://github.com/FluxxDB/hasami
cd hasami
powershell -ExecutionPolicy Bypass -File scripts/fetch-ffmpeg.ps1   # pinned ffmpeg, SHA256-checked
npm ci
npm run tauri build
```

The installer lands in `src-tauri/target/release/bundle/nsis/`.

Run the checks with:

```powershell
npm test               # frontend unit tests (Vitest)
npm run check          # svelte-check / TypeScript
cd src-tauri; cargo test   # Rust unit and integration tests (they use the fetched ffmpeg)
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the dev loop and an architecture map, and
[docs/design](docs/design) for the design spec and implementation plans.

## License

Hasami is licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT), at your option.

The installer also ships FFmpeg (GPLv3, run as a separate program) and the Inter
and Instrument Serif fonts (SIL OFL 1.1). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

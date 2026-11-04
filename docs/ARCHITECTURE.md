# Architecture

Hasami is one Tauri 2 app. A Rust backend owns every file and every ffmpeg run, and two
small Svelte windows (the Quick Trim editor and the progress window) talk to it through
Tauri commands and events. Click a box in either chart to open its source.

## Components

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

- **One process.** Every right-click launches `hasami.exe`. Later launches hand their files
  to the first one (single-instance), and a 400 ms collector turns a multi-select into one batch.
- **One queue.** Compress, Convert and editor exports run one at a time in `jobs/queue`, so
  encodes never fight over the GPU or disk. The app exits once no window is open and the queue is empty.
- **File safety.** Every output goes to a temp file, is checked with ffprobe, and only then is
  committed with a non-replacing rename. Originals are recycled or moved only after that, and
  nothing is ever deleted permanently (spec §8).

## A Quick export

```mermaid
sequenceDiagram
  autonumber
  participant U as You
  participant E as Editor (Svelte)
  participant C as Tauri commands
  participant Q as Job queue
  participant X as Export job
  participant F as ffmpeg
  participant S as fsops

  U->>E: Ctrl+B, Delete, Quick export
  E->>C: editor_export(request: cuts, volumes, master, crop)
  C->>Q: enqueue Export (returns job id)
  Q-->>E: ExportStarted / ExportProgress (clip locked, progress bar)
  Q->>X: run
  X->>F: -filter_complex trim/concat, amix, crop → H.264 .mp4 temp
  X->>F: ffprobe the temp (duration, size)
  X->>S: commit [CLIPPED] name.mp4 (non-replacing rename)
  S->>S: recycle the original (Quick) or move it into Raw\ (Save raw)
  opt 20 MB COPY on
    X->>F: two-pass encode of [CLIPPED] → [20MB] copy in Videos
  end
  Q-->>E: ExportDone (Saved [CLIPPED] name.mp4)
```

See [design/specs](design/specs/2026-10-30-hasami-design.md) for the full behavior spec.

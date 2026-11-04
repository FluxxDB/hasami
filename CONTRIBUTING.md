# Contributing to Hasami

Thanks for helping. Hasami is small on purpose, so please open an issue before
starting on a large change or a new feature, and check it against the non-goals
in the [design spec](docs/design/specs/2026-10-30-hasami-design.md) §2.

## Setup

Hasami is Windows-only, and it's built and tested with the **Windows** toolchain so
that tests use the real WebView2, codecs and Explorer menu.

- Windows 10/11 x64, PowerShell
- [Rust](https://rustup.rs) 1.89+ with the MSVC target (`x86_64-pc-windows-msvc`) and the Visual Studio C++ build tools
- [Node.js](https://nodejs.org) 20

```powershell
powershell -ExecutionPolicy Bypass -File scripts/fetch-ffmpeg.ps1   # once: ffmpeg/ffprobe + license into src-tauri/binaries/
npm ci
```

**WSL users:** keep the repo on a Windows drive (`/mnt/c/...`) and call the Windows
tools through interop, never WSL's own cargo, node or ffmpeg:

```sh
cmd.exe /c npm ci
cmd.exe /c npm test
(cd src-tauri && cargo.exe test)
```

## Dev loop

```powershell
npm run tauri -- dev -- -- trim "C:\path\to\a copy of a clip.mp4"   # the editor
npm run tauri -- dev -- -- compress "C:\path\to\a copy of a clip.mp4"
npx vite --port 5182   # then open http://localhost:5182/editor.html?mock=1 for the UI without Tauri
```

The app takes the same verbs Explorer passes it: `trim`, `compress`, `convert-mp4`,
`convert-mov`, `convert-mp3` (followed by files), and `register` / `unregister` for the
right-click menu. Work on **copies** of clips: exports recycle or move the original.

## Checks

Run all of these before you send a pull request:

```powershell
npm test         # Vitest
npm run check    # svelte-check / TypeScript
cd src-tauri
cargo test       # unit + integration tests; they generate fixtures with the fetched ffmpeg
```

## Architecture map

| Path | What lives there |
|---|---|
| `src-tauri/src/` | The Rust side. `app.rs` (Tauri setup, single-instance batching, windows, commands), `cli.rs`, `shell_menu.rs` (HKCU registry menu), `media/` (finding and running ffmpeg/ffprobe, probing), `fsops.rs` + `verify.rs` (temp file → verify → commit → recycle), `compress/` (the 20 MB planner), `jobs/` (Compress, Convert and Export jobs, the serial queue), `editor/` (Quick Trim sessions, media prep and export graphs) |
| `src-tauri/tests/` | Integration tests against real ffmpeg output |
| `src-tauri/nsis/hooks.nsh` | Installer hooks that register and unregister the menu |
| `src/editor/` | The Quick Trim editor (Svelte 5): timeline model, playback engine, keyboard map, components |
| `src/progress/` | The small progress window for Compress and Convert |
| `src/lib/` | Shared TypeScript types and the bundled fonts |
| `docs/design/` | The design spec (behavior, file-safety rules, visual rules), the implementation plans, product notes, and the approved mockup |
| `docs/branding/` | The visual system the UI follows |

The spec's §8 file-safety rules are non-negotiable: never move or recycle an
original before its replacement is written and verified, and never delete anything
permanently.

## Third-party licenses

The installer ships `THIRD_PARTY_LICENSES_RUST.md` (every crate compiled into
`hasami.exe`) and `THIRD_PARTY_LICENSES_NPM.md` (every npm package bundled into the
UI). When you add, remove or update a dependency, regenerate and commit them:

```powershell
cargo install cargo-about --locked --features cli   # once
powershell -ExecutionPolicy Bypass -File scripts/licenses.ps1
```

If `cargo about` rejects a new license, add it to `accepted` in `scripts/about.toml`
only if it's compatible with shipping Hasami as `MIT OR Apache-2.0`, and mention it
in `THIRD_PARTY_NOTICES.md`.

## Releasing

1. Bump the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`.
2. Run `scripts/licenses.ps1` and the checks above.
3. `npm run tauri build`, then smoke-test the installer from `src-tauri/target/release/bundle/nsis/`.
4. Create the GitHub release with the installer **and** the FFmpeg source tarball
   (`ffmpeg-9.0.2.tar.xz` from <https://ffmpeg.org/releases/>, or the version
   `scripts/fetch-ffmpeg.ps1` pins). `THIRD_PARTY_NOTICES.md` promises that tarball
   on every release.

## Commits and pull requests

- Write plain, one-line summaries that say what changed for the user or the code,
  e.g. `Compress keeps the original and writes the copy to Videos`. A `fix:` /
  `feat:` / `docs:` prefix is fine but not required.
- Keep each commit focused, and include tests with behavior changes.
- Unless you explicitly state otherwise, any contribution you intentionally submit
  for inclusion in Hasami shall be dual licensed as `MIT OR Apache-2.0`, without any
  additional terms or conditions.

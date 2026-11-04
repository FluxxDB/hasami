# Third-party notices

Hasami's own code is licensed under `MIT OR Apache-2.0` (see `LICENSE-MIT` and
`LICENSE-APACHE`). The Hasami installer also ships the third-party software
below, each under its own license. The installer copies this file and the
license texts it names into the `licenses` folder next to `hasami.exe`.

## FFmpeg (`ffmpeg.exe`, `ffprobe.exe`)

| | |
|---|---|
| Software | FFmpeg 9.0.2, the gyan.dev "full" static build for Windows x64 (version string `9.0.2-full_build-www.gyan.dev`) |
| Copyright | The FFmpeg developers and the authors of the libraries listed in the build's README |
| License | **GNU General Public License, version 3**. The build enables GPL and version-3 components such as libx264 and libx265 |
| Binary | `ffmpeg-9.0.2-full_build.zip` from <https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2>, SHA256 `759d0a9831c436a0eb331ad36f236c06cb04aaa0005da46571f0e9d3d9206f6b` |
| Installed as | `ffmpeg.exe` and `ffprobe.exe` next to `hasami.exe`, with `licenses\ffmpeg\LICENSE.txt` (the GPLv3 text from the build) and `licenses\ffmpeg\README.txt` (the build's configuration, enabled libraries and their versions) |

**How Hasami uses it.** ffmpeg and ffprobe are shipped unmodified, as separate
programs that Hasami starts as child processes. Hasami doesn't link to any
FFmpeg library, so Hasami's own code stays under its own license. You may
replace the two executables with any other compatible ffmpeg build.

**Corresponding source.** The build was made from FFmpeg release 9.0.2, git
tag `n9.0.2` (commit `946fcce07b6dcd0331c8cc609192aeff5e1924f8`):

- release tarball: <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>
- git: <https://git.ffmpeg.org/ffmpeg.git> (mirror: <https://github.com/FFmpeg/FFmpeg/tree/n9.0.2>)
- the external libraries compiled into the build, with their exact versions, are
  listed in `licenses\ffmpeg\README.txt`; the build page at
  <https://www.gyan.dev/ffmpeg/builds/> links to each library's upstream source.

A copy of the FFmpeg source tarball is also attached to each Hasami release on
GitHub (<https://github.com/FluxxDB/hasami/releases>). To get the same binaries
for a build of your own, run `scripts/fetch-ffmpeg.ps1`, which downloads the
archive above and checks its SHA256.

## Fonts

Both fonts are bundled into the app's UI as WOFF2 Latin subsets and are
licensed under the **SIL Open Font License, version 1.1**. The full license
texts are in `src/lib/fonts/` and are installed under `licenses\fonts\`.

| Font | Files | Copyright |
|---|---|---|
| Inter | `inter-latin-{400,500,700}-normal.woff2` | Copyright 2016 The Inter Project Authors (<https://github.com/rsms/inter>) |
| Instrument Serif | `instrument-serif-latin-400-italic.woff2` | Copyright 2022 The Instrument Serif Project Authors (<https://github.com/Instrument/instrument-serif>) |

## Rust crates (compiled into `hasami.exe`)

Hasami's direct dependencies:

| Crate | License |
|---|---|
| `tauri`, `tauri-build`, `tauri-plugin-single-instance` | MIT OR Apache-2.0 |
| `serde`, `serde_json` | MIT OR Apache-2.0 |
| `thiserror` | MIT OR Apache-2.0 |
| `winreg` | MIT |

Their transitive dependencies (see `src-tauri/Cargo.lock`) are almost all
`MIT OR Apache-2.0` or `MIT`. The others are:

- **Apache-2.0:** `tao`
- **Unicode-3.0:** the ICU4X crates (`icu_*`, `zerovec`, `yoke`, `zerofrom`, `tinystr`, `litemap`, `writeable`, `potential_utf`, `zerotrie`)
- **MPL-2.0:** `cssparser`, `selectors`, `dtoa-short`, `option-ext`, used unmodified; their source is on <https://crates.io>
- **BSD-3-Clause:** `brotli`, `brotli-decompressor`, `alloc-no-stdlib`, `alloc-stdlib`
- **Zlib:** `foldhash`, `zlib-rs`

For a full per-crate list with license texts, run
[`cargo about`](https://github.com/EmbarkStudios/cargo-about) or
[`cargo license`](https://github.com/onur/cargo-license) in `src-tauri/`.

## npm packages (bundled into the UI)

| Package | License |
|---|---|
| `svelte` and its runtime helpers (`clsx`, `esm-env`, `devalue`) | MIT |
| `@tauri-apps/api` | Apache-2.0 OR MIT |

Build tools (Vite, TypeScript, svelte-check, Vitest, the Tauri CLI) are used to
build Hasami and aren't shipped.

## Microsoft Edge WebView2

Hasami's windows run in the WebView2 runtime that comes with Windows. It isn't
part of Hasami; if it's missing, the installer downloads Microsoft's bootstrapper,
which installs it under Microsoft's own terms.

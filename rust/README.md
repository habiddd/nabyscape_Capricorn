# NabyScape (Rust)

The Rust port of NabyScape, a dock for Windows 10 (build 19041+) and Windows 11. It is one
native executable, `nabyscape.exe`, with no .NET, no web view, no GPU API and no async runtime.
The C# application in `src/NabyScape/` stays the reference; this workspace reads and writes the
same `%LOCALAPPDATA%\NabyScape` files.

## Build, run, test

Everything runs from this `rust/` folder.

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo clippy -p naby-win --target x86_64-pc-windows-gnu -- -D warnings
```

Windows (MSVC, the release toolchain):

```
cargo build --release -p naby-win --target x86_64-pc-windows-msvc    # or aarch64-pc-windows-msvc
target\x86_64-pc-windows-msvc\release\nabyscape.exe
```

Linux/macOS: the pure logic and the UI toolkit build and test on every platform. A Windows binary
can be cross-built with `--target x86_64-pc-windows-gnu` (mingw; `.cargo/config.toml` holds the
linker). The release profile is `opt-level 3`, fat LTO, one codegen unit, `panic = "abort"`,
stripped.

Command line of `nabyscape.exe`:

| Argument | Meaning |
| --- | --- |
| *(none)* | start the dock; a second launch opens Settings in the running one |
| `--background` | started at sign-in: no "ready" notification |
| `--settings` | open Settings after start |
| `--exit` | ask the running instance to quit |
| `--install [--silent] [--launch] [--install-dir <folder>]` | per-user installer (also used when the file name ends with `-Setup`) |
| `--uninstall [--quiet] [--keep-data]` | remove the app (and, by default, its data and models) |
| `--update-handoff` | the update helper (started by the app, never by hand) |
| `--update-started <token>` | acknowledges a start to the update helper |
| `--self-test [--report <file>] [--require-payload]` | store, defaults, fonts, version resource, Whistle files; exit code 0 = healthy |
| `--widget-mcp` | serve the widget pages over MCP on stdio |

## Architecture

Four crates; only `naby-win` contains `unsafe` (each block has a `// SAFETY:` line).

| Crate | Role |
| --- | --- |
| `naby-core` | Pure model and rules, no OS calls: settings and presets, JSON store with schema sidecars and recovery, session state, i18n, notifications, media/weather/lyrics parsing, productivity data, assistant providers and storage, the update protocol (`updates`: release parsing, 24 h throttle, checksum, download, PE inspection). Everything is tested with injected clocks, HTTP and file systems. |
| `naby-render` | CPU rendering: a `tiny-skia` canvas, `fontdue` text with a byte-bounded glyph cache, icons, image decode, and an immediate-mode UI toolkit with keyboard focus and scrolling. Produces BGRA pixels; snapshot tests write PNGs. |
| `naby-ui` | The screens as pure views over snapshots: dock, panels, Settings, welcome guide, assistant. They emit `Command`s and never touch Windows. |
| `naby-win` | The Windows host: layered dock window, message loop, tray, hotkeys, monitors, WinRT media, WASAPI audio, clipboard, downloads, launcher, assistant window, and the installer / updater / uninstaller (`install`, `update/`, `uninstall`, `winops`). |

### Installer, updates, uninstall

* **No separate setup binary.** `nabyscape.exe --install` is the installer, as in the C# releases
  where `NabyScape-<rid>-Setup.exe` is the program itself. A Setup file is `nabyscape.exe` with a
  ZIP appended after the PE image (the Whistle files and notices); installing copies the exe byte
  for byte to `%LOCALAPPDATA%\Programs\NabyScape` (or a chosen `...\NabyScape` folder) and unpacks
  the payload next to it. Because the installed exe is identical to the Setup, the updater's
  SHA-256 check of the installed copy, and updates from or to a C# release, keep working.
* **Registration** matches C#: `HKCU\...\Uninstall\NabyScape` (`DisplayName`, `DisplayVersion`,
  `Publisher`, `InstallLocation`, `DisplayIcon`, `UninstallString`, `QuietUninstallString`,
  `NoModify`, `NoRepair`, `EstimatedSize`), Start-menu shortcuts, the `Run` value
  `"<exe>" --background`, and clean-up of LumaIsland / BillyNabilaIsland leftovers.
* **Updates** (`update/`): an automatic check at start (at most every 24 h, `AutoCheckUpdates`), a
  notice when a release or a verified download is ready (`NotifyUpdates`), and the Settings
  buttons Check / Download / Cancel / Install and restart / Restore previous / Release notes.
  Downloads run on a worker thread with progress; the setup must match the pinned size, SHA-256,
  PE machine, version resource and (if signed) Authenticode. Unsigned releases are accepted, like
  the C# updater. Install saves the settings, copies the exe to
  `NabyScape-Updates\NabyScape-UpdateHelper.exe` and quits; the helper saves a recovery copy of
  the exe and the profile's JSON files, runs the staged setup silently, starts the new app and
  restores the recovery copy if setup or the first start fails.
* **Uninstall** (`uninstall.rs`) is native Rust: the program copies itself to `%TEMP%` (it cannot
  delete itself), asks whether to also remove data and downloaded models, stops owned processes,
  and removes saved API keys, data roots, the startup value, shortcuts, installed files and the
  registry entry. Every path is checked against its root and rejected if a link is involved; the
  whole plan is preflighted before the first deletion.

## Memory design

The goal is a small private working set while the dock is closed or hidden:

* No timer runs while nothing animates or counts down; the message loop sleeps in
  `MsgWaitForMultipleObjectsEx`. A single one-shot timer is armed for the next frame only.
* Pixel buffers of hidden surfaces are dropped (`Canvas::release`, window DIB, glyph cache);
  caches are byte-bounded (4 MiB glyphs, thumbnail cache by bytes).
* Background services are plain threads that sleep on channels; HTTP uses one shared `ureq`
  agent. No runtime, no GPU context, no browser engine.
* Whistle (below) is loaded only while voice input is active and unloaded afterwards.
* The release profile strips symbols and uses `panic = "abort"`.

### Measuring RAM

`tools\measure-memory.ps1` reads the private working set (RAM only this process uses; shared DLL
pages excluded) of `nabyscape.exe` once per second:

```
.\tools\measure-memory.ps1 -Executable ..\target\x86_64-pc-windows-msvc\release\nabyscape.exe -Start -Seconds 60 -Label closed-idle -Report closed-idle.csv
```

Measure each state separately in a release build: dock hidden, closed idle, closed with media
playing, panel open, Settings open, voice active. It prints min / median / p95 / max and writes a
CSV. Task Manager's default "Memory" column is the private working set too.

## How Whistle is packaged

The default listening model is Cactus Compute Whistle: `whistle.cact` (model) run by the Cactus
Needle engine `libneedle.dll` (per architecture). Both ship next to `nabyscape.exe`, so there is
no first-run download. `whistle.lock` (written by the voice task) is a JSON array of
`{ "name", "arch" ("any"|"x64"|"arm64"), "url", "sha256" }`.

`build-release.ps1 -Tag vX.Y.Z` builds both architectures, downloads the pinned files, verifies
each SHA-256 (it stops if the lock file is missing or a hash differs), checks the DLL's PE
machine, runs the self-test with `--require-payload` where the host can execute the binary, and
writes into a new empty folder:

```
NabyScape-win-x64-Setup.exe      NabyScape-win-arm64-Setup.exe     (exe + appended payload ZIP)
NabyScape-win-x64-Portable.zip   NabyScape-win-arm64-Portable.zip  (plain exe + files + notices)
NabyScape-Notices.zip            SHA256SUMS
```

The asset names are those of the C# releases, so the same GitHub release feed and updater work.
Notices contain `LICENSE`, `THIRD-PARTY-NOTICES.txt` (from `cargo about` when installed with an
`about.hbs`/`about.toml`, otherwise generated from `cargo metadata`) and `WHISTLE-NOTICE.txt`
(Cactus Whistle / Needle, Apache-2.0).

## Known gaps

*(To be filled in by the whole-branch review.)*

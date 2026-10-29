# NabyScape Rust port — design

## Goal

Re-implement NabyScape 2.24.0 (C#/WPF, `src/NabyScape`) in Rust with the same features,
the same per-user data files, and a much smaller memory footprint.

The C# sources are the functional spec. Every task in the plan names the C# files it ports;
behavior, defaults, clamps, copy and edge cases come from those files and from the C# tests in
`src/NabyScape/Tests`. The user-facing description is `README.md` and `docs/guide.md`.

## Why the WPF build is heavy, and what replaces each part

| WPF cost | Rust replacement |
| --- | --- |
| .NET runtime + JIT + GC heaps (tens of MB before any UI) | Native binary, no GC, `panic = "abort"`, LTO |
| WPF retained visual tree, DirectX composition per window | Immediate-mode views painted on demand with tiny-skia into a DIB, shown via `UpdateLayeredWindow` |
| Font/glyph caches per window, XAML resources | One `fontdue` rasterizer over system Segoe UI, bounded glyph cache |
| `async`/`Task`, thread pool, dispatcher timers always ticking | Plain worker threads per job, results posted to the UI thread; timers armed only while something animates or counts |
| Decoded artwork/icons at source size | Decode straight to display size, byte-bounded LRU |
| WinForms `NotifyIcon`/`Screen` | `Shell_NotifyIconW`, `EnumDisplayMonitors` |
| NAudio | WASAPI through the `windows` crate |
| System.Speech | `Windows.Media.SpeechRecognition` (WinRT) |

Budget: idle dock (closed, music playing, waveform) ≤ 30 MB private working set and ~0% CPU when
nothing moves; surfaces that are hidden release their pixel buffers.

## Crates

```
rust/
  crates/naby-core    platform-free: settings, store, session, productivity, media logic (lyrics,
                      spectrum DSP, palette), weather, updates, notifications, system formatting,
                      motion math, i18n, assistant protocol. Tested on Linux.
  crates/naby-render  tiny-skia canvas, fontdue text, Lucide icons, image decode, immediate-mode
                      widget toolkit. Snapshot-tested to PNG on Linux.
  crates/naby-ui      overlay state machine and every view (closed/hover/open dock, widget pages,
                      notifications, settings, welcome, assistant). Consumes snapshots, emits
                      `Command`s. Snapshot-tested on Linux.
  crates/naby-win     the `nabyscape.exe` host: message loop, layered windows, tray, hotkeys,
                      SMTC media, WASAPI audio, system sensors, clipboard, downloads watcher,
                      launcher, credentials, updates/installer. Built for x86_64-pc-windows-gnu here,
                      MSVC in release CI.
```

Data flow: host services produce snapshot structs (`naby_core::media::MediaSnapshot`, …) on worker
threads → posted to the UI thread → the active view repaints only when its inputs changed → views
emit `naby_core::command::Command` → the host executes it.

## Compatibility

* Reads and writes the same JSON documents in `%LOCALAPPDATA%\NabyScape` with the same PascalCase
  keys and schema sidecars, so an existing user switches builds without losing anything. Unknown
  keys are preserved on save.
* Same hotkeys, presets (`docs/presets/*.json`), widget ids and defaults.
* API keys stay in Windows Credential Manager under the same target names.

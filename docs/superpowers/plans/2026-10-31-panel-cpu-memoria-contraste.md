# Panel CPU, Settings memory, contrast and tooltip Implementation Plan

> **For agentic workers:** Work task by task, in order. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Fix the four problems found by driving the real app: 18.9% CPU moving the mouse over the panel, +34 MB with Settings open, 2.92:1 text contrast in the panel, sidebar tooltip covering the menu.

**Architecture:** Small independent fixes in the existing Rust crates. No new dependencies, modules or settings.

**Tech Stack:** Rust workspace in `rust/` (naby-ui, naby-render, naby-win), tiny-skia CPU rendering, Windows MSVC.

**Spec:** the measured findings above; there is no separate spec.

## Global Constraints

- Do NOT commit. The tree already holds uncommitted work on `windows-voice-whistle-piper`; never revert or reformat it beyond `cargo fmt --all`.
- Do NOT move the mouse, send input or launch the GUI. The lead does the real-app check.
- All commands run from `rust/`.
- Finish with: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, all clean.
- Match surrounding code; comments only for a why the code cannot show.

## Review Focus

1. Bright album cover: panel text must stay legible, not only with the white fallback.
2. Dark or saturated cover: the glow must keep its colour and not get dimmed needlessly.
3. Pointer entering the panel far from the last light position: the reflection must still jump to it.
4. Settings opened, closed, reopened: fonts must keep working after sharing.
5. Settings and the island painting alternately: shared glyph cache must stay within its bound.

---

### Task 1: Pointer reflection stops invalidating the glass cache

**Cause:** `crates/naby-ui/src/overlay/state.rs:658` updates `light` on a change above 0.001 (under 1 px). Every update misses the glass cache, a ~5 ms frame. The reflection peaks at alpha 35 over a radius of `0.85 * w`, so 10 px changes it by under one colour level: invisible.

**Files:**
- Modify: `crates/naby-ui/src/overlay/state.rs:658`
- Test: `crates/naby-ui/src/overlay/tests.rs`

- [ ] **Step 1: Write the failing test** `small_pointer_moves_keep_the_cached_reflection`, using the `Rig` with an open glass panel: move the pointer inside the panel so `light` settles, record `light`, move by 2 px, assert `light` is unchanged; move by 100 px, assert it changed.
- [ ] **Step 2:** Run `cargo test -p naby-ui --lib small_pointer_moves`. Expected: FAIL on the 2 px case.
- [ ] **Step 3:** Change the threshold to `0.02`, with the one-line why.
- [ ] **Step 4:** Run the test. Expected: PASS.

### Task 2: Panel glow keeps text legible

**Cause:** with no cover the glow falls back to the accent (white by default), painted by `paint_band` in `crates/naby-ui/src/overlay/surface.rs:274` up to gray 87 behind text. Dim text (160,165,173) reaches only 2.92:1. A bright cover has the same problem.

**Decision:** limit the glow by luminance, not per colour: 4.5:1 for that text needs a background relative luminance of at most `0.0433`. Scale down only when the band would exceed it, so dark or saturated colours are untouched.

**Files:**
- Modify: `crates/naby-ui/src/overlay/surface.rs` (`paint_band`)
- Test: `crates/naby-ui/src/overlay/tests.rs`

- [ ] **Step 1: Write failing tests**, painting `paint_band` over a black canvas with gain 1.0 and the fade the panel really uses:
  - `a_white_glow_stays_dark_enough_for_panel_text`: the highest pixel relative luminance is `<= 0.0433` (+0.002 rounding margin).
  - `a_dark_glow_is_not_dimmed`: a band whose peak is already under the limit paints the same pixels as before the limit (compare with the expected unscaled composite, tolerance 1 level).
- [ ] **Step 2:** Run them. Expected: the white case FAILS.
- [ ] **Step 3:** Add `fn legible_gain(band: &ColorBand, fade: &BottomFade, gain: f64) -> f64` in `surface.rs`: takes the brighter of `band.left` / `band.right` at the fade's peak alpha times `gain`, returns the gain capped so the composite over black meets the limit. Use it at the top of `paint_band`. Both Linear and Radial shapes go through it. Named const `MAX_GLOW_LUMINANCE: f64 = 0.0433`.
- [ ] **Step 4:** Run the tests; then `cargo test -p naby-ui --lib`. If a snapshot or ambient test changes because of the cap, update its expected value only when the change is the cap itself, and list it in the report.

### Task 3: Settings memory: measure, then share fonts if they are the cost

**Cause to confirm:** `crates/naby-win/src/settings_host.rs:713` calls `Fonts::load_system()` again, on top of `app.rs:489`. `crates/naby-render/src/text.rs:116` reads whole font files, and each `Fonts` has its own glyph cache. The rest of the +34 MB is the canvas and present buffers (~3.4 MB each at 1090x775).

**Files:**
- Modify: `crates/naby-win/src/settings_host.rs`, `crates/naby-win/src/app.rs`
- Test: `crates/naby-render/src/text.rs` (existing `glyph_cache_stays_bounded`)

- [ ] **Step 1:** Measure with a temporary ignored test in `text.rs`: total font bytes held by `Fonts::load_system()` and the glyph cache limit. Print them, then delete the test.
- [ ] **Step 2: Decide.** If font bytes plus cache are under 8 MB: STOP this task, change nothing, and report the numbers. Otherwise continue.
- [ ] **Step 3:** Keep one `Rc<Fonts>` in the app and pass a clone to `SettingsHost::open`; the host field becomes `fonts: Rc<Fonts>`. Do this only if all users are on the UI thread (`Fonts` is single-threaded by design); if `SettingsHost` runs on another thread, STOP and report. Do not touch `assistant_host.rs` or other hosts.
- [ ] **Step 4:** Run `cargo test -p naby-win` and `cargo build --release -p naby-win`. Expected: pass.

### Task 4: Sidebar tooltip stops covering the menu

**Cause:** `crates/naby-ui/src/settings/nav.rs:132-133` shows the category description under the pointer, on top of the next item. The same text is the page subtitle.

**Files:**
- Modify: `crates/naby-ui/src/settings/nav.rs:132-133`

- [ ] **Step 1:** Delete those two lines. Keep `description` where the page subtitle uses it.
- [ ] **Step 2:** Run `cargo test -p naby-ui --lib settings::`. Expected: PASS. A deletion needs no new test.

### Final

- [ ] Run the three commands from Global Constraints and `cargo build --release -p naby-win`.
- [ ] Report per task: what changed, test output, the font numbers and decision of Task 3, anything skipped.

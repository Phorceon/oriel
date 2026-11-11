# Oriel

Oriel is a macOS app that brings Chrome-style tabs to real desktop applications.
Instead of a window manager or a virtualized environment, Oriel runs as a thin tab
bar. Each tab corresponds to a real installed app, and that app's real window is
glued below the bar. When you switch tabs, Oriel hides the previous app's window and
shows the new one.

The idea is a clean, single-focus workspace. You keep the windows of the apps you
are using, and inactive apps are hidden so Mission Control stays tidy. The bar is a transient panel: Mission Control and App Exposé hide it
and show only the glued app window.

## Install

Apple Silicon, macOS 13 or later. This repository is also the Homebrew tap. Because the
repository is named `oriel` and not `homebrew-oriel`, the tap needs its URL:

```
brew tap phorceon/oriel https://github.com/Phorceon/oriel
brew install --cask oriel
```

On first run:

1. Oriel is not notarized by Apple, so macOS blocks it. Open System Settings > Privacy &
   Security and click "Open Anyway".
2. Grant Accessibility in System Settings > Privacy & Security > Accessibility, so Oriel can
   move other apps' windows.

To uninstall:

```
brew uninstall --cask oriel
```

To also delete the saved tabs, use `--zap`:

```
brew uninstall --cask --zap oriel
```

## How it behaves

- One tab per window. A tab represents a real window of an installed app.
- The "+" button adds an installed application to the bar.
- Closing a tab closes that app's window; when Oriel closes its last window, the app
  quits.
- Windows of the same app that are not active are minimized.
- Mouse first: click a tab (it switches on the press), drag a tab to reorder it, middle-click
  to close it, drag empty bar space to move the window, double-click it to zoom.
- Split view: right-click a tab and choose "Split with Current Tab" to show it side by side
  with the tab you are on, under the one bar. Drag the thin line between the two windows to
  change their sizes. Click a tab outside the pair to see it at full size (the pair stays
  joined); click either tab of the pair to bring both windows back. Right-click a tab of the
  pair and choose "End Split View" to end it. Closing one of the two tabs ends it too, and the
  other tab stays. A split is not remembered when Oriel quits.
- Tabs are remembered between runs in `~/Library/Application Support/Oriel/state.json`.
- A tab you close is forgotten, however you close it: with the tab's "x", with Cmd+W or the red
  button in the app, or by quitting the app. It does not come back at the next start. Oriel
  waits about 4 seconds first (the tab looks dimmed) in case the app only closes a splash
  window or restarts itself; a window that opens in that time keeps the tab. Closing the last
  window in the app does not quit the app. Tabs that are still open when Oriel exits are
  remembered, and so is an app you added that has not started yet.

## Requirements

- macOS.
- Rust 1.99.1.
- Accessibility permission. Oriel drives other apps through the macOS Accessibility
  API, so it must be granted access under System Settings > Privacy & Security >
  Accessibility. Without it, Oriel cannot see or control other apps' windows. Run
  `scripts/signing-setup` once (see "Signing") so the grant survives rebuilds.

## Build and run

Run from the repository root:

```
cargo run -p oriel-app
```

or build a proper `.app` bundle:

```
scripts/bundle --install --open
```

Caveat: `.cargo/config.toml` pins `SDKROOT` to one specific Command Line Tools SDK path. This
is a workaround for the author's machine. On another Mac, remove or adjust that line if the
build cannot find the SDK.

## Signing

Run `scripts/signing-setup` once. It creates a self-signed code signing identity named
`Oriel Local Signing` in your login keychain. `scripts/bundle` then signs the app with
it (identifier `app.oriel.Oriel`), so the app keeps the same designated requirement
across rebuilds and the Accessibility grant survives them. Without the identity, the
bundle is signed ad hoc and the grant is lost after every rebuild. Set `ORIEL_SIGN`
to use a different identity.

If you grant Accessibility before running `scripts/signing-setup`, remove the old
entry once: `tccutil reset Accessibility app.oriel.Oriel`, then grant it again.

To remove the identity:

```
security delete-identity -c "Oriel Local Signing" ~/Library/Keychains/login.keychain-db
```

## Releasing

`scripts/release` builds a path-scrubbed, signed bundle and zips it as `dist/Oriel-<version>.zip`.
It then rewrites the tracked file `Casks/oriel.rb` in place from `packaging/homebrew/oriel.rb.in`.
The version comes from `[workspace.package]` in the root `Cargo.toml`. It needs the
`Oriel Local Signing` identity. At the end it prints the commands to publish: commit and push
`Casks/oriel.rb`, then `gh release create v<version>` on `Phorceon/oriel` with the zip. The
script never publishes by itself.

## Workspace layout

The repository is a Cargo workspace with three crates:

- `crates/oriel-core` — the platform-free core: shared data types, the state model,
  and disk persistence.
- `crates/oriel-mac` — the macOS layer: the Accessibility integration, window
  handling, and the real platform backend.
- `crates/oriel-app` — the Rune UI; the binary is named `oriel`.

The UI is built with Rune (<https://github.com/anomalyco/rune>).

## Icon

The app icon master is `assets/icon/oriel.svg`; `scripts/make-icon [svg]` rebuilds `assets/icon/Oriel.icns` from it (other concepts are in `assets/icon/concepts/`).

## Known limits

- macOS cannot truly embed another app's window. Oriel drags the bar itself and moves the
  app's real window in the same step (a thread of its own, one AX call per frame). Measured
  on a 60 Hz display, the window stays on the bar in every sampled frame of a bar drag; when
  the user drags the app's window instead, the bar is sometimes one frame (about 16 ms)
  behind. Switching between two windows of the same app plays the system's un-minimize
  animation (about 0.35 s to place, 0.55 s to settle).

## Measuring

`scripts/harness/` holds the measurement harness (no synthetic input, no screenshots: it reads
window bounds from the window list). `scripts/harness/run.sh <oriel binary> <out dir>` runs one
short session with TextEdit and Calculator only; `analyze.py <out dir>` prints startup, drag,
and tab switch numbers, `drift.py` checks alignment after many drags, and `timeline.py` prints
the windows over time. `ORIEL_DEBUG_SCRIPT` (see `crates/oriel-app/src/debug.rs`) drives the same
code paths as the pointer.
- Full-screen apps and multiple displays are not handled yet.

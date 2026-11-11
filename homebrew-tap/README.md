# Phorceon tap

Homebrew tap for Oriel, a macOS app that brings Chrome-style tabs to real desktop apps.

## Install

```
brew install --cask phorceon/tap/oriel
```

Oriel needs an Apple Silicon Mac.

## First run

1. Oriel is not notarized by Apple. macOS blocks it on first open. Open System Settings >
   Privacy & Security and click "Open Anyway" for Oriel.
2. Grant Accessibility: System Settings > Privacy & Security > Accessibility. Oriel needs
   it to move other apps' windows.

## Uninstall

```
brew uninstall --cask oriel
```

To also delete saved tabs:

```
brew uninstall --zap --cask oriel
```

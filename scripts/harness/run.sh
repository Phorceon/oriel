#!/usr/bin/env bash
# One short, real-platform measurement run of Oriel with TextEdit and Calculator only.
#
#   scripts/harness/run.sh <oriel binary> <out dir> [script]
#
# - Refuses to start when TextEdit, Calculator or Oriel already run (Oriel would take over
#   their windows).
# - Opens TextEdit with two temp .txt files (two windows, so two tabs) and Calculator, all in
#   the background, with a temp state file (ORIEL_STATE) that lists only those two apps.
# - Samples the window list (winwatch) for the whole run, runs Oriel with a debug script
#   (ORIEL_DEBUG_SCRIPT), moves the glued TextEdit window through AX when the script says
#   `mark glued-move`, then quits Oriel (its own quit path), TextEdit and Calculator, and
#   checks that nothing is left hidden, minimized or running.
# No synthetic input, no screenshots.
set -euo pipefail

bin="${1:?oriel binary}"
out="${2:?out dir}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ww="${WINWATCH:-$out/winwatch}"
default_script="3 drag 1000 300 3; 7.5 activate 2; 9.5 activate 0; 11.5 activate 1; 13.5 activate 0; 15.5 mark glued-move; 20 quit"
script="${3:-$default_script}"
duration="${DURATION:-22}"

for app in TextEdit Calculator oriel; do
    if pgrep -x "$app" >/dev/null; then
        echo "refusing: $app is running" >&2
        exit 1
    fi
done

mkdir -p "$out"
if [ ! -x "$ww" ]; then
    SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk swiftc -O "$here/winwatch.swift" -o "$ww"
fi
docs="$(mktemp -d)"
printf 'Oriel harness document A\n' >"$docs/a.txt"
printf 'Oriel harness document B\n' >"$docs/b.txt"
state="$docs/state.json"
cat >"$state" <<EOF
{
  "apps": [
    {"bundle_id": "com.apple.TextEdit", "name": "TextEdit", "path": "/System/Applications/TextEdit.app"},
    {"bundle_id": "com.apple.calculator", "name": "Calculator", "path": "/System/Applications/Calculator.app"}
  ],
  "frame": {"x": 100.0, "y": 120.0, "w": 900.0, "h": 600.0},
  "active_app": "com.apple.TextEdit"
}
EOF

cleanup() {
    cp "$state" "$out/state.json" 2>/dev/null || true
    pkill -TERM -x oriel 2>/dev/null || true
    sleep 0.5
    for app in TextEdit Calculator; do
        osascript -e "tell application \"$app\" to quit saving no" >/dev/null 2>&1 || true
    done
    sleep 1
    for app in TextEdit Calculator oriel; do
        if pgrep -x "$app" >/dev/null; then
            echo "warning: $app still runs; sending SIGTERM" >&2
            pkill -TERM -x "$app" || true
        fi
    done
    rm -rf "$docs"
}
trap cleanup EXIT

open -g -a TextEdit "$docs/a.txt" "$docs/b.txt"
open -g -a Calculator
sleep 2.5

"$ww" sample "$duration" "$out/samples.csv" oriel TextEdit Calculator &
sampler=$!
sleep 0.2

start="$(python3 -c 'import time; print("%.3f" % (time.time()*1000))')"
echo "oriel-mark $start launch" >"$out/marks.txt"
ORIEL_STATE="$state" ORIEL_DEBUG_MARKS=1 ORIEL_DEBUG_SCRIPT="$script" "$bin" 2>>"$out/stderr.txt" &
oriel=$!

# The glued-window move: start when the script prints its mark.
(
    for _ in $(seq 1 400); do
        if grep -q 'glued-move' "$out/stderr.txt" 2>/dev/null; then
            "$ww" move TextEdit 3 1000 300 >>"$out/marks-move.txt"
            break
        fi
        sleep 0.05
    done
) &
mover=$!

wait "$oriel" || true
wait "$mover" 2>/dev/null || true
wait "$sampler" || true
grep '^oriel-mark' "$out/stderr.txt" >>"$out/marks.txt" || true
if [ -f "$out/marks-move.txt" ]; then
    sed -n 's/^move-start \([0-9.]*\).*/oriel-mark \1 move-start/p; s/^move-end \([0-9.]*\)/oriel-mark \1 move-end/p' "$out/marks-move.txt" >>"$out/marks.txt"
fi
echo "done: $out"

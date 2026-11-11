#!/usr/bin/env python3
"""Drift check for a run with many drags: after each `drag-end` mark (+600 ms), is the glued
window still exactly under the bar (same x and width, top edge at the bar's bottom)? Also
compares the saved state's frame with the bar on screen at the end."""
import json
import sys
sys.path.insert(0, __import__("os").path.dirname(__file__))
from analyze import load, bar_of, glued_of, final_alignment

out = sys.argv[1]
times, samples, marks = load(out)
ends = [t for t, m in marks if m == "drag-end"]
worst = 0.0
for i, t in enumerate(ends):
    fa = final_alignment(times, samples, t + 600)
    if fa is None:
        print(f"drag {i + 1}: no sample")
        continue
    bar_w = fa["bar"][2]
    win_w = fa["win"][2]
    if abs(win_w - bar_w) < 1:
        err = max(abs(fa["dx"]), abs(fa["dy"]), abs(fa["dw"]))
    else:  # a refusing window: centered (fit), top at the bar's bottom
        err = max(abs(fa["dy"]), abs((fa["win"][0] - fa["bar"][0]) - (bar_w - win_w) / 2))
    worst = max(worst, err)
    print(f"drag {i + 1:2d}: bar {fa['bar']} win {fa['win']} error {err:.1f} pt")
print(f"worst error after {len(ends)} drags: {worst:.1f} pt")
try:
    saved = json.load(open(f"{out}/state.json"))["frame"]
    last = final_alignment(times, samples, times[-1] - 2000)
    print(f"saved frame {saved}; bar on screen near the end {last['bar'] if last else None}")
except Exception as e:  # noqa: BLE001
    print("no saved state:", e)

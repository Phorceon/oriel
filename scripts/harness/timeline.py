#!/usr/bin/env python3
"""Print bar and app windows over time: timeline.py <out dir> <from mark> <ms before> <ms after> [step ms]"""
import sys
sys.path.insert(0, __import__("os").path.dirname(__file__))
from analyze import load, bar_of, apps_of, mark_time
out, name, before, after = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
step = float(sys.argv[5]) if len(sys.argv) > 5 else 16
times, samples, marks = load(out)
t0 = mark_time(marks, name)
last = -1e9
for t in times:
    if t < t0 - before or t > t0 + after or t - last < step:
        continue
    last = t
    rows = samples[t]
    b = bar_of(rows)
    bs = f"bar {b['x']:7.1f},{b['y']:6.1f} w{b['w']:5.0f} a{b['alpha']:.2f}" if b else "bar   -"
    ws = " ".join(f"{r['owner'][:4]}#{r['wid']}@{r['x']:.0f},{r['y']:.0f} {r['w']:.0f}x{r['h']:.0f}" for r in apps_of(rows))
    print(f"{t - t0:8.1f} {bs} | {ws}")

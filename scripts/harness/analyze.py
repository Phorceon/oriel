#!/usr/bin/env python3
"""Analyze one harness run (scripts/harness/run.sh): samples.csv + marks.txt.

Reports, in points and milliseconds:
- startup: launch -> bar on screen, launch -> glued window in place under the bar;
- bar drag: error between the glued window and the bar during a steady drag (max, p95),
  the lag (time shift that best aligns the window's path with the bar's), and the time
  from the end of the drag until both are aligned;
- glued-window drag (AX move from outside, as a user drag): the bar's error and lag;
- tab switches: activate mark -> new window in place, and whether a sample exists with
  no app window under the bar (a "flash" of the desktop).
"""
import bisect
import csv
import sys
from collections import defaultdict

BAR_H = 40.0


def load(out):
    samples = defaultdict(list)  # t -> rows
    with open(f"{out}/samples.csv") as f:
        for row in csv.DictReader(f):
            t = float(row["t_ms"])
            if row["owner"] == "-":
                samples[t]
                continue
            samples[t].append(
                dict(owner=row["owner"], wid=int(row["wid"]), layer=int(row["layer"]),
                     order=int(row["order"]), x=float(row["x"]), y=float(row["y"]),
                     w=float(row["w"]), h=float(row["h"]), alpha=float(row.get("alpha") or 1)))
    marks = []
    with open(f"{out}/marks.txt") as f:
        for line in f:
            parts = line.split()
            if len(parts) >= 3 and parts[0] == "oriel-mark":
                marks.append((float(parts[1]), " ".join(parts[2:])))
    marks.sort()
    times = sorted(samples)
    return times, samples, marks


def bar_of(rows):
    for r in rows:
        if r["owner"] == "oriel" and abs(r["h"] - BAR_H) < 1.5 and r.get("alpha", 1) > 0.01:
            return r
    return None


def apps_of(rows):
    """App windows big enough to be a document/calculator window, front to back."""
    return [r for r in rows if r["owner"] in ("TextEdit", "Calculator") and r["layer"] == 0 and r["h"] > 100]


def glued_of(rows, bar):
    """The app window whose top edge is nearest the bar's bottom edge."""
    if bar is None:
        return None
    best = None
    for r in apps_of(rows):
        d = abs(r["y"] - (bar["y"] + BAR_H)) + abs(r["x"] - bar["x"]) * 0.0
        if best is None or d < best[0]:
            best = (d, r)
    return best[1] if best else None


def pct(values, p):
    if not values:
        return float("nan")
    v = sorted(values)
    k = min(len(v) - 1, max(0, int(round(p / 100.0 * (len(v) - 1)))))
    return v[k]


def mark_time(marks, name, after=0.0):
    for t, m in marks:
        if t >= after and m.startswith(name):
            return t
    return None


def interp(ts, xs, t):
    i = bisect.bisect_left(ts, t)
    if i <= 0:
        return xs[0]
    if i >= len(ts):
        return xs[-1]
    t0, t1 = ts[i - 1], ts[i]
    if t1 == t0:
        return xs[i]
    return xs[i - 1] + (xs[i] - xs[i - 1]) * (t - t0) / (t1 - t0)


def follow(times, samples, t0, t1, leader, follower_fn, fit_x=True):
    """Errors and lag of `follower` against `leader` in [t0, t1]. Returns a dict."""
    lt, lx, ft, fx, errs, errs_y = [], [], [], [], [], []
    for t in times:
        if t < t0 or t > t1:
            continue
        rows = samples[t]
        bar = bar_of(rows)
        win = glued_of(rows, bar)
        if bar is None or win is None:
            continue
        lead, foll = (bar, win) if leader == "bar" else (win, bar)
        lt.append(t)
        lx.append(lead["x"])
        ft.append(t)
        fx.append(foll["x"])
        offset = 0.0
        errs.append(win["x"] - bar["x"] - offset)
        errs_y.append(win["y"] - (bar["y"] + BAR_H))
    if len(lt) < 10:
        return None
    # Window may be narrower (a fit): its x is offset by a constant at rest. Remove the
    # median offset of the first samples before the move starts in earnest.
    base = sorted(errs[:5])[2]
    abs_err = [abs(e - base) for e in errs]
    best = (float("inf"), 0.0)
    for lag in range(0, 400):
        acc = 0.0
        n = 0
        for t, x in zip(ft, fx):
            if t - lag < lt[0]:
                continue
            acc += (x - (interp(lt, lx, t - lag) + (base if leader == "bar" else -base))) ** 2
            n += 1
        if n:
            mse = acc / n
            if mse < best[0]:
                best = (mse, lag)
    return dict(n=len(lt), max=max(abs_err), p95=pct(abs_err, 95), mean=sum(abs_err) / len(abs_err),
                max_y=max(abs(e) for e in errs_y), lag_ms=best[1], rms_after_lag=best[0] ** 0.5)


def step_latency(times, samples, t0, t1, leader):
    """For each step of the leader's x, the time until the follower shows the same offset
    again (ms), and the share of samples where the two disagree."""
    lat, bad, n = [], 0, 0
    pending = None
    base = None
    for t in times:
        if t < t0 or t > t1:
            continue
        rows = samples[t]
        bar = bar_of(rows)
        win = glued_of(rows, bar)
        if bar is None or win is None:
            continue
        off = win["x"] - bar["x"]
        if base is None:
            # A window as wide as the bar sits exactly under it; a narrower one (a fit) keeps
            # its first offset.
            base = 0.0 if abs(win["w"] - bar["w"]) < 1 else off
        lead = bar["x"] if leader == "bar" else win["x"]
        n += 1
        aligned = abs(off - base) < 1
        if not aligned:
            bad += 1
            if pending is None:
                pending = t
        elif pending is not None:
            lat.append(t - pending)
            pending = None
    if not lat:
        return dict(steps=0, p50=0.0, p95=0.0, max=0.0, misaligned=bad / max(n, 1))
    return dict(steps=len(lat), p50=pct(lat, 50), p95=pct(lat, 95), max=max(lat), misaligned=bad / max(n, 1))


def settle(times, samples, t_end, limit=2000):
    """Time after t_end until bar and glued window stay aligned (<1 pt) to the end of window."""
    last_bad = t_end
    base = None
    for t in times:
        if t < t_end - 400 or t > t_end + limit:
            continue
        rows = samples[t]
        bar = bar_of(rows)
        win = glued_of(rows, bar)
        if bar is None or win is None:
            continue
        ex = win["x"] - bar["x"]
        ey = win["y"] - (bar["y"] + BAR_H)
        if t < t_end:
            base = ex if base is None else base
            continue
        if base is None:
            base = 0.0
        if abs(ey) >= 1 or (abs(ex - base) >= 1 and abs(ex) >= 1):
            last_bad = t
    return last_bad - t_end


def final_alignment(times, samples, t):
    i = bisect.bisect_left(times, t)
    for tt in times[i:]:
        rows = samples[tt]
        bar = bar_of(rows)
        win = glued_of(rows, bar)
        if bar and win:
            return dict(dx=win["x"] - bar["x"], dy=win["y"] - (bar["y"] + BAR_H), dw=win["w"] - bar["w"],
                        bar=(bar["x"], bar["y"], bar["w"]), win=(win["x"], win["y"], win["w"], win["h"]))
    return None


def switch(times, samples, t0, owner, limit=3000):
    """From t0: when is a window of `owner` on screen right under the bar (and the front app
    window), and was there a sample with no app window under the bar?"""
    in_place_at = None
    gap_samples = 0
    gap_ms = 0.0
    prev_t = None
    first_wid = None
    for t in times:
        if t < t0:
            continue
        if t > t0 + limit:
            break
        rows = samples[t]
        bar = bar_of(rows)
        apps = apps_of(rows)
        if bar is None:
            prev_t = t
            continue
        under = [r for r in apps if abs(r["y"] - (bar["y"] + BAR_H)) < 1.5]
        if not under:
            gap_samples += 1
            if prev_t is not None:
                gap_ms += t - prev_t
        if in_place_at is None and under and under[0]["owner"] == owner:
            if first_wid is None:
                first_wid = under[0]["wid"]
            others = [r for r in apps if r["owner"] != owner]
            if not others or owner == "TextEdit":
                in_place_at = t
        prev_t = t
    return dict(in_place_ms=None if in_place_at is None else in_place_at - t0, gap_samples=gap_samples, gap_ms=gap_ms)


def same_app_switch(times, samples, t0, limit=3000):
    """Same-app switch: the window id under the bar changes; when is the new one in place and
    the old one gone?"""
    i = bisect.bisect_left(times, t0)
    start_rows = samples[times[i]] if i < len(times) else []
    bar = bar_of(start_rows)
    old = glued_of(start_rows, bar)
    old_wid = old["wid"] if old else None
    in_place = None
    old_gone = None
    gap = 0
    for t in times[i:]:
        if t > t0 + limit:
            break
        rows = samples[t]
        bar = bar_of(rows)
        if bar is None:
            continue
        apps = [r for r in apps_of(rows) if r["owner"] == "TextEdit"]
        under = [r for r in apps if abs(r["y"] - (bar["y"] + BAR_H)) < 1.5 and abs(r["x"] - bar["x"]) < 1.5]
        if not under:
            gap += 1
        if in_place is None and any(r["wid"] != old_wid for r in under):
            in_place = t - t0
        if old_gone is None and not any(r["wid"] == old_wid for r in apps):
            old_gone = t - t0
    return dict(new_in_place_ms=in_place, old_gone_ms=old_gone, gap_samples=gap)


def main():
    out = sys.argv[1]
    times, samples, marks = load(out)
    launch = mark_time(marks, "launch")
    print(f"run: {out}  samples={len(times)}  rate={len(times) / ((times[-1] - times[0]) / 1000):.0f} Hz")
    # Startup.
    bar_at = next((t for t in times if t >= launch and bar_of(samples[t])), None)
    glued_at = None
    for t in times:
        if t < launch:
            continue
        rows = samples[t]
        bar = bar_of(rows)
        win = glued_of(rows, bar)
        if bar and win and abs(win["y"] - bar["y"] - BAR_H) < 1 and abs(win["x"] - bar["x"]) < 1:
            glued_at = t
            break
    print(f"startup: bar visible {bar_at - launch if bar_at else None:.0f} ms; "
          f"glued in place {glued_at - launch if glued_at else float('nan'):.0f} ms")

    def drag_report(start_name, end_name, leader, label):
        t0 = mark_time(marks, start_name)
        t1 = mark_time(marks, end_name, after=t0 or 0)
        if t0 is None or t1 is None:
            print(f"{label}: no marks")
            return
        r = follow(times, samples, t0 + 150, t1, leader, None)
        if r is None:
            print(f"{label}: not enough samples")
            return
        s = settle(times, samples, t1)
        fa = final_alignment(times, samples, t1 + 1500)
        sl = step_latency(times, samples, t0 + 150, t1, leader)
        print(f"  {label} steps: misaligned in {sl['misaligned'] * 100:.1f}% of samples; time to realign p50 "
              f"{sl['p50']:.1f} ms, p95 {sl['p95']:.1f} ms, max {sl['max']:.1f} ms over {sl['steps']} episodes")
        print(f"{label}: n={r['n']} err max {r['max']:.1f} pt, p95 {r['p95']:.1f} pt, mean {r['mean']:.1f} pt; "
              f"|dy| max {r['max_y']:.1f} pt; lag {r['lag_ms']} ms (rms after lag {r['rms_after_lag']:.1f} pt); "
              f"settled {s:.0f} ms after end; final {fa}")

    drag_report("drag", "drag-end", "bar", "bar drag")
    drag_report("move-start", "move-end", "win", "glued drag")
    # Tab switches.
    for t, m in marks:
        if not m.startswith("activate"):
            continue
        idx = int(m.split()[1])
        target = "Calculator" if idx == 2 else "TextEdit"
        before = bisect.bisect_left(times, t)
        rows = samples[times[before]] if before < len(times) else []
        bar = bar_of(rows)
        cur = glued_of(rows, bar)
        if cur and cur["owner"] == target == "TextEdit":
            print(f"{m}: same-app {same_app_switch(times, samples, t)}")
        else:
            print(f"{m}: cross-app -> {target} {switch(times, samples, t, target)}")


if __name__ == "__main__":
    main()

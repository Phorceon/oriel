// winwatch: measurement helper for Oriel. No screenshots, no synthetic input.
//
//   winwatch sample <seconds> <out.csv> <owner>...
//       Samples CGWindowListCopyWindowInfo as fast as it can (about 300-500 Hz) and writes
//       one line per on-screen window of the named owners (process names) per sample:
//       t_ms,owner,pid,wid,layer,order,x,y,w,h   (t_ms: wall clock, ms since the epoch)
//       Only windows of the named owners are written; nothing else is recorded.
//   winwatch move <app name> <seconds> <speed pt/s> [<span pt>]
//       Moves the first standard window of that app (TextEdit or Calculator only) back and
//       forth along x through Accessibility at 120 Hz, as a user dragging its title bar would.
//       Prints "move-start <t_ms>" and "move-end <t_ms>".
//   winwatch frames <app name>
//       Prints the AX frame of each window of the app.

import AppKit
import ApplicationServices
import Foundation

func nowMs() -> Double {
    var ts = timespec()
    clock_gettime(CLOCK_REALTIME, &ts)
    return Double(ts.tv_sec) * 1000.0 + Double(ts.tv_nsec) / 1_000_000.0
}

func sample(seconds: Double, out: String, owners: Set<String>) {
    FileManager.default.createFile(atPath: out, contents: nil)
    guard let fh = FileHandle(forWritingAtPath: out) else { fatalError("cannot write \(out)") }
    fh.write("t_ms,owner,pid,wid,layer,order,x,y,w,h,alpha\n".data(using: .utf8)!)
    let end = nowMs() + seconds * 1000.0
    var buf = ""
    var n = 0
    while nowMs() < end {
        let t = nowMs()
        let opts: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
        guard let list = CGWindowListCopyWindowInfo(opts, kCGNullWindowID) as? [[String: Any]] else { continue }
        for (order, w) in list.enumerated() {
            guard let owner = w[kCGWindowOwnerName as String] as? String, owners.contains(owner) else { continue }
            let pid = w[kCGWindowOwnerPID as String] as? Int ?? 0
            let wid = w[kCGWindowNumber as String] as? Int ?? 0
            let layer = w[kCGWindowLayer as String] as? Int ?? 0
            guard let b = w[kCGWindowBounds as String] as? [String: Any] else { continue }
            let x = b["X"] as? Double ?? 0, y = b["Y"] as? Double ?? 0
            let ww = b["Width"] as? Double ?? 0, hh = b["Height"] as? Double ?? 0
            let alpha = w[kCGWindowAlpha as String] as? Double ?? 1
            buf += String(format: "%.3f,%@,%d,%d,%d,%d,%.1f,%.1f,%.1f,%.1f,%.2f\n", t, owner, pid, wid, layer, order, x, y, ww, hh, alpha)
        }
        // A sample with none of the owners on screen still needs a row: "-" marks it.
        buf += String(format: "%.3f,-,0,0,0,0,0,0,0,0,0\n", t)
        n += 1
        if buf.utf8.count > 1 << 16 {
            fh.write(buf.data(using: .utf8)!)
            buf = ""
        }
        usleep(1500)
    }
    fh.write(buf.data(using: .utf8)!)
    fh.closeFile()
    FileHandle.standardError.write("winwatch: \(n) samples\n".data(using: .utf8)!)
}

let allowed: Set<String> = ["TextEdit", "Calculator"]

func appWindow(_ name: String) -> (pid_t, AXUIElement)? {
    guard allowed.contains(name) else { fatalError("only TextEdit or Calculator") }
    guard let app = NSWorkspace.shared.runningApplications.first(where: { $0.localizedName == name }) else { return nil }
    let el = AXUIElementCreateApplication(app.processIdentifier)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(el, kAXWindowsAttribute as CFString, &value) == .success,
          let wins = value as? [AXUIElement] else { return nil }
    for w in wins {
        var sub: CFTypeRef?
        AXUIElementCopyAttributeValue(w, kAXSubroleAttribute as CFString, &sub)
        var minimized: CFTypeRef?
        AXUIElementCopyAttributeValue(w, kAXMinimizedAttribute as CFString, &minimized)
        if (sub as? String) == "AXStandardWindow" && (minimized as? Bool) != true { return (app.processIdentifier, w) }
    }
    return nil
}

func frameOf(_ w: AXUIElement) -> CGRect? {
    var p: CFTypeRef?, s: CFTypeRef?
    guard AXUIElementCopyAttributeValue(w, kAXPositionAttribute as CFString, &p) == .success,
          AXUIElementCopyAttributeValue(w, kAXSizeAttribute as CFString, &s) == .success else { return nil }
    var pt = CGPoint.zero, sz = CGSize.zero
    AXValueGetValue(p as! AXValue, .cgPoint, &pt)
    AXValueGetValue(s as! AXValue, .cgSize, &sz)
    return CGRect(origin: pt, size: sz)
}

func move(name: String, seconds: Double, speed: Double, span: Double) {
    guard let (_, w) = appWindow(name), let start = frameOf(w) else { fatalError("no window") }
    let t0 = nowMs()
    print(String(format: "move-start %.3f %.1f %.1f", t0, start.origin.x, start.origin.y))
    var k = 0
    while true {
        let t = nowMs()
        if t - t0 > seconds * 1000 { break }
        // Triangle wave: out by `span` at `speed`, then back.
        let travel = speed * (t - t0) / 1000.0
        let phase = travel.truncatingRemainder(dividingBy: 2 * span)
        let dx = phase < span ? phase : 2 * span - phase
        var p = CGPoint(x: start.origin.x + dx, y: start.origin.y)
        let v = AXValueCreate(.cgPoint, &p)!
        AXUIElementSetAttributeValue(w, kAXPositionAttribute as CFString, v)
        k += 1
        let next = t0 + Double(k) * (1000.0 / 120.0)
        let wait = next - nowMs()
        if wait > 0 { usleep(useconds_t(wait * 1000)) }
    }
    // Back to the start, and hold still.
    var p = start.origin
    AXUIElementSetAttributeValue(w, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &p)!)
    print(String(format: "move-end %.3f", nowMs()))
}

let args = CommandLine.arguments
guard args.count >= 2 else { fatalError("usage: winwatch sample|move|frames ...") }
switch args[1] {
case "sample":
    sample(seconds: Double(args[2])!, out: args[3], owners: Set(args[4...]))
case "move":
    move(name: args[2], seconds: Double(args[3])!, speed: Double(args[4])!, span: args.count > 5 ? Double(args[5])! : 300)
case "frames":
    if let (_, w) = appWindow(args[2]), let f = frameOf(w) { print(f) }
default:
    fatalError("unknown mode")
}

// minexp: experiments on TextEdit only, to choose the same-app switch mechanism.
// Opens nothing itself: run it with TextEdit already showing two documents.
//   E1: set the frame of a minimized window, then un-minimize: where does it land, how fast?
//   E2: minimize a window of a hidden app: instant? Still minimized after unhide?
//   E3: un-minimize a window of a hidden app, then unhide: animation?
import AppKit
import ApplicationServices

func nowMs() -> Double {
    var ts = timespec(); clock_gettime(CLOCK_REALTIME, &ts)
    return Double(ts.tv_sec) * 1000 + Double(ts.tv_nsec) / 1e6
}
func ax<T>(_ el: AXUIElement, _ a: String) -> T? {
    var v: CFTypeRef?
    guard AXUIElementCopyAttributeValue(el, a as CFString, &v) == .success else { return nil }
    return v as? T
}
func frame(_ w: AXUIElement) -> CGRect {
    var p = CGPoint.zero, s = CGSize.zero
    if let v: AXValue = ax(w, kAXPositionAttribute) { AXValueGetValue(v, .cgPoint, &p) }
    if let v: AXValue = ax(w, kAXSizeAttribute) { AXValueGetValue(v, .cgSize, &s) }
    return CGRect(origin: p, size: s)
}
func setFrame(_ w: AXUIElement, _ r: CGRect) {
    var s = r.size, p = r.origin
    AXUIElementSetAttributeValue(w, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &s)!)
    AXUIElementSetAttributeValue(w, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &p)!)
    AXUIElementSetAttributeValue(w, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &s)!)
}
func setMin(_ w: AXUIElement, _ m: Bool) {
    AXUIElementSetAttributeValue(w, kAXMinimizedAttribute as CFString, (m ? kCFBooleanTrue : kCFBooleanFalse)!)
}
var windowIdOf: [CFHashCode: CGWindowID] = [:]
@_silgen_name("_AXUIElementGetWindow") func _AXUIElementGetWindow(_ el: AXUIElement, _ id: UnsafeMutablePointer<CGWindowID>) -> AXError
func wid(_ w: AXUIElement) -> CGWindowID { var id: CGWindowID = 0; _ = _AXUIElementGetWindow(w, &id); return id }
func cgBounds(_ id: CGWindowID) -> CGRect? {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else { return nil }
    for w in list where (w[kCGWindowNumber as String] as? Int) == Int(id) {
        let b = w[kCGWindowBounds as String] as! [String: Double]
        return CGRect(x: b["X"]!, y: b["Y"]!, width: b["Width"]!, height: b["Height"]!)
    }
    return nil
}
/// Sample a window's on-screen bounds until it rests at `target` (or `ms` pass).
func watch(_ id: CGWindowID, _ label: String, until target: CGRect?, ms: Double = 1500) {
    let t0 = nowMs(); var last = "", restAt: Double? = nil, firstOn: Double? = nil
    while nowMs() - t0 < ms {
        let b = cgBounds(id)
        let s = b.map { "\(Int($0.minX)),\(Int($0.minY)) \(Int($0.width))x\(Int($0.height))" } ?? "off"
        if b != nil && firstOn == nil { firstOn = nowMs() - t0 }
        if s != last { print(String(format: "  %@ %7.1f %@", label, nowMs() - t0, s)); last = s }
        if let t = target, let b = b, abs(b.minX - t.minX) < 1, abs(b.minY - t.minY) < 1, abs(b.width - t.width) < 1 {
            restAt = restAt ?? nowMs() - t0
        } else { restAt = nil }
        usleep(2000)
    }
    print(String(format: "  %@ first on screen %@ ms, at target from %@ ms", label,
                 firstOn.map { String(format: "%.0f", $0) } ?? "-", restAt.map { String(format: "%.0f", $0) } ?? "-"))
}

guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.TextEdit").first else { fatalError("no TextEdit") }
let appEl = AXUIElementCreateApplication(app.processIdentifier)
let wins: [AXUIElement] = (ax(appEl, kAXWindowsAttribute) ?? []).filter { (ax($0, kAXSubroleAttribute) as String?) == "AXStandardWindow" || (ax($0, kAXMinimizedAttribute) as Bool?) == true }
guard wins.count >= 2 else { fatalError("need two TextEdit windows, have \(wins.count)") }
let (a, b) = (wins[0], wins[1])
let (ida, idb) = (wid(a), wid(b))
let target = CGRect(x: 100, y: 160, width: 900, height: 560)
print("windows a=\(ida) b=\(idb)")
app.activate()
setFrame(a, target); setFrame(b, CGRect(x: 300, y: 300, width: 500, height: 400))
usleep(300_000)

print("E1: minimize b, set its frame while minimized, un-minimize")
var t = nowMs(); setMin(b, true); print(String(format: "  setMin(true) returned after %.1f ms", nowMs() - t))
watch(idb, "b-min", until: nil, ms: 700)
t = nowMs(); setFrame(b, target); print(String(format: "  setFrame on minimized took %.1f ms; AX frame now %@; AXMinimized=%@", nowMs() - t, "\(frame(b))", "\((ax(b, kAXMinimizedAttribute) as Bool?) ?? false)"))
t = nowMs(); setMin(b, false); print(String(format: "  setMin(false) returned after %.1f ms; AXMinimized=%@", nowMs() - t, "\((ax(b, kAXMinimizedAttribute) as Bool?) ?? false)"))
watch(idb, "b-unmin", until: target, ms: 900)
print("  AX frame after: \(frame(b))")

print("E2: hide app, minimize a while hidden, unhide")
app.hide(); usleep(300_000)
t = nowMs(); setMin(a, true); print(String(format: "  setMin(true) while hidden: %.1f ms; AXMinimized=%@", nowMs() - t, "\((ax(a, kAXMinimizedAttribute) as Bool?) ?? false)"))
usleep(100_000)
app.unhide()
watch(ida, "a-after-unhide", until: nil, ms: 600)
print("  a AXMinimized=\((ax(a, kAXMinimizedAttribute) as Bool?) ?? false)")

print("E3: hide app, set a's frame, un-minimize a while hidden, unhide")
app.hide(); usleep(300_000)
setFrame(a, target)
t = nowMs(); setMin(a, false); print(String(format: "  setMin(false) while hidden: %.1f ms; AXMinimized=%@", nowMs() - t, "\((ax(a, kAXMinimizedAttribute) as Bool?) ?? false)"))
watch(ida, "a-hidden", until: nil, ms: 300)
t = nowMs(); app.unhide()
watch(ida, "a-unhide", until: target, ms: 900)

print("E4: un-minimize-free switch: b at target behind a, raise b")
setMin(b, false); usleep(800_000); setFrame(b, target); setFrame(a, target)
AXUIElementPerformAction(a, kAXRaiseAction as CFString); usleep(200_000)
t = nowMs(); AXUIElementPerformAction(b, kAXRaiseAction as CFString); print(String(format: "  raise took %.1f ms", nowMs() - t))
print("done")

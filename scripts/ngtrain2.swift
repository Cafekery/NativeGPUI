// ngtrain2 <pid> <x> <y> <px-per-tick> <hz> <secs> [reverse-every-ticks]
// Posts a paced pixel scroll train from a HID source at (x,y); refuses unless <pid> is frontmost.
import AppKit
let a = CommandLine.arguments
guard a.count >= 7, let pid = Int32(a[1]), let x = Double(a[2]), let y = Double(a[3]),
      let step = Int32(a[4]), let hz = Double(a[5]), let secs = Double(a[6]) else {
    FileHandle.standardError.write("usage: ngtrain2 <pid> <x> <y> <px> <hz> <secs> [reverse-every]\n".data(using: .utf8)!); exit(2)
}
let reverseEvery = a.count > 7 ? Int(a[7]) ?? 0 : 0
let env = ProcessInfo.processInfo.environment
let phases = env["GESTURE"] != nil || env["PHASES"] != nil
let continuous = env["GESTURE"] != nil || env["CONT"] != nil
func frontmost() -> Bool { NSWorkspace.shared.frontmostApplication?.processIdentifier == pid }
guard frontmost() else { print("NGTRAIN-REFUSED want=\(pid) have=\(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1)"); exit(3) }
let source = CGEventSource(stateID: .hidSystemState)
let point = CGPoint(x: x, y: y)
CGEvent(mouseEventSource: source, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
usleep(60_000)
let ticks = Int(hz * secs)
let interval = 1.0 / hz
var sign: Int32 = -1
var posted = 0
let t0 = CACurrentMediaTime()
for tick in 0..<ticks {
    if reverseEvery > 0, tick > 0, tick % reverseEvery == 0 { sign = -sign }
    if tick % 30 == 0, !frontmost() { print("NGTRAIN-ABORTED tick=\(tick)"); exit(4) }
    if let e = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 1, wheel1: sign * step, wheel2: 0, wheel3: 0) {
        e.location = point
        if phases {
            // kCGScrollPhaseBegan = 1, Changed = 2, Ended = 4, laid out like a real trackpad gesture
            let phase: Int64 = tick == 0 ? 1 : (tick == ticks - 1 ? 4 : 2)
            e.setIntegerValueField(.scrollWheelEventScrollPhase, value: phase)
        }
        if continuous { e.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1) }
        e.post(tap: .cghidEventTap)
        posted += 1
    }
    let target = t0 + Double(tick + 1) * interval
    let now = CACurrentMediaTime()
    if target > now { usleep(UInt32((target - now) * 1e6)) }
}
print("NGTRAIN-DONE posted=\(posted) step=\(step) hz=\(hz) secs=\(secs) reverse_every=\(reverseEvery) took=\(String(format: "%.3f", CACurrentMediaTime() - t0))")

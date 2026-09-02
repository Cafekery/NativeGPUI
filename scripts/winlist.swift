import CoreGraphics
import Foundation

// Window-server enumeration, not Accessibility: AX cannot see an empty-title window.
let wantPid = CommandLine.arguments.count > 1 ? Int(CommandLine.arguments[1]) : nil

guard
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]]
else {
    FileHandle.standardError.write("no window list\n".data(using: .utf8)!)
    exit(2)
}

var printed = 0
for info in list {
    guard let pid = info[kCGWindowOwnerPID as String] as? Int else { continue }
    if let want = wantPid, pid != want { continue }
    let number = info[kCGWindowNumber as String] as? Int ?? -1
    let owner = info[kCGWindowOwnerName as String] as? String ?? "?"
    let title = info[kCGWindowName as String] as? String ?? ""
    let layer = info[kCGWindowLayer as String] as? Int ?? -1
    var x = 0.0, y = 0.0, w = 0.0, h = 0.0
    if let b = info[kCGWindowBounds as String] as? [String: Any] {
        x = b["X"] as? Double ?? 0
        y = b["Y"] as? Double ?? 0
        w = b["Width"] as? Double ?? 0
        h = b["Height"] as? Double ?? 0
    }
    print("\(number)\t\(pid)\t\(layer)\t\(Int(x)),\(Int(y)),\(Int(w)),\(Int(h))\t\(owner)\t\(title)")
    printed += 1
}
if printed == 0 { exit(1) }

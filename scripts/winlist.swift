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
    let b = (info[kCGWindowBounds as String] as? NSDictionary).flatMap { CGRect(dictionaryRepresentation: $0 as CFDictionary) } ?? .zero
    print("\(number)\t\(pid)\t\(layer)\t\(Int(b.minX)),\(Int(b.minY)),\(Int(b.width)),\(Int(b.height))\t\(owner)\t\(title)")
    printed += 1
}
if printed == 0 { exit(1) }

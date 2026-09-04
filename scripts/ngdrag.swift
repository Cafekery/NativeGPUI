// usage: ngdrag <x0> <y0> <x1> <y1> <steps> <ms per step> — a real left-button drag across the
// screen, as a window-edge resize needs: the window server only resizes for events it routed.
import CoreGraphics
import Foundation

let args = CommandLine.arguments.dropFirst().compactMap(Double.init)
guard args.count == 6 else {
    FileHandle.standardError.write(Data("usage: ngdrag <x0> <y0> <x1> <y1> <steps> <ms>\n".utf8))
    exit(2)
}
let (x0, y0, x1, y1) = (args[0], args[1], args[2], args[3])
let steps = max(1, Int(args[4]))
let gap = UInt32(args[5] * 1000)
let source = CGEventSource(stateID: .hidSystemState)

func post(_ type: CGEventType, _ x: Double, _ y: Double) {
    CGEvent(
        mouseEventSource: source,
        mouseType: type,
        mouseCursorPosition: CGPoint(x: x, y: y),
        mouseButton: .left
    )?.post(tap: .cghidEventTap)
    usleep(gap)
}

post(.mouseMoved, x0, y0)
post(.leftMouseDown, x0, y0)
for step in 1...steps {
    let travelled = Double(step) / Double(steps)
    post(.leftMouseDragged, x0 + (x1 - x0) * travelled, y0 + (y1 - y0) * travelled)
}
post(.leftMouseUp, x1, y1)

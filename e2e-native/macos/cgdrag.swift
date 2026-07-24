// Synthetic mouse gesture via CoreGraphics events — the piece AppleScript can't
// do. Used by the macOS smoke driver to DRAG the real titlebar (catching a dead
// drag region) and CLICK the real red close button (catching close interception).
//
//   cgdrag drag  x1 y1 x2 y2   — press at (x1,y1), interpolate-drag to (x2,y2), release
//   cgdrag click x  y          — press + release at (x,y)
//
// Coordinates are global, top-left origin — the same frame AX `position` reports,
// so the driver feeds AX-derived points straight in.
import CoreGraphics
import Foundation

func post(_ type: CGEventType, _ p: CGPoint) {
    CGEvent(
        mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: .left
    )?.post(tap: .cghidEventTap)
}

func sleepMs(_ ms: UInt32) { usleep(ms * 1000) }

let args = CommandLine.arguments
guard args.count >= 2 else {
    FileHandle.standardError.write("usage: cgdrag drag|click ...\n".data(using: .utf8)!)
    exit(2)
}

switch args[1] {
case "drag":
    guard args.count == 6, let x1 = Double(args[2]), let y1 = Double(args[3]),
        let x2 = Double(args[4]), let y2 = Double(args[5])
    else { exit(2) }
    let start = CGPoint(x: x1, y: y1)
    let end = CGPoint(x: x2, y: y2)
    post(.mouseMoved, start)
    sleepMs(80)
    post(.leftMouseDown, start)
    sleepMs(120)
    // Interpolate so the window server tracks a genuine drag, not a teleport.
    let steps = 24
    for i in 1...steps {
        let t = Double(i) / Double(steps)
        post(.leftMouseDragged, CGPoint(x: x1 + (x2 - x1) * t, y: y1 + (y2 - y1) * t))
        sleepMs(16)
    }
    post(.leftMouseUp, end)
    sleepMs(80)
case "click":
    guard args.count == 4, let x = Double(args[2]), let y = Double(args[3]) else { exit(2) }
    let p = CGPoint(x: x, y: y)
    post(.mouseMoved, p)
    sleepMs(80)
    post(.leftMouseDown, p)
    sleepMs(90)
    post(.leftMouseUp, p)
    sleepMs(80)
default:
    exit(2)
}

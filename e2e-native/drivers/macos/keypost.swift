// Milestone PT — deliver ONE keyboard chord straight to the app under test.
//
//     keypost <pid> <chord>        e.g. `keypost 4321 cmd+shift+l`
//
// WHY THIS EXISTS
//
// The harness types through appium-mac2-driver (`macos: keys`), which injects at
// the HID layer — the most faithful path there is, and the one every step uses
// first. But on macOS ANY application may claim a chord as a global hot key
// through `RegisterEventHotKey`, and the WindowServer then consumes it before
// the frontmost app is ever offered it. On this machine ⌘⇧L is Amphetamine's
// start/stop hot key (`com.if.Amphetamine` → `StartStopHotkey` = keyCode 37,
// modifiers ⌘⇧) and something else owns ⌘⇧K, so two of MDReader's own shortcuts
// never reach it. That is a fact about the machine, not about the product: the
// same chord posted into the app's event queue opens the surface immediately.
//
// So this exists ONLY as the harness's second attempt, after the real HID path
// has been tried and demonstrably produced nothing — see `drivers/runner.ts`,
// which reports when it had to fall back rather than absorbing the difference.
// `CGEvent.postToPid` puts a genuine keyboard event into that process's queue:
// the same event AppKit would deliver from the keyboard, minus another
// application's claim on the combination.
//
// FAILS CLOSED: an unparseable chord, an unmapped key, or a keycode this file
// does not know exits 2 with a message. Posting nothing and exiting 0 would let
// a step read "the app ignored the shortcut" when nothing was ever sent.
import CoreGraphics
import Foundation

/// ANSI virtual keycodes for the keys the shared `Chord` vocabulary uses. Listed
/// explicitly rather than derived, so an unknown key is a hard error and never a
/// silently-wrong keystroke.
let KEYCODES: [String: CGKeyCode] = [
    "a": 0, "b": 11, "c": 8, "d": 2, "e": 14, "f": 3, "g": 5, "h": 4, "i": 34,
    "j": 38, "k": 40, "l": 37, "m": 46, "n": 45, "o": 31, "p": 35, "q": 12,
    "r": 15, "s": 1, "t": 17, "u": 32, "v": 9, "w": 13, "x": 7, "y": 16, "z": 6,
    "enter": 36, "return": 36, "escape": 53, "esc": 53, "tab": 48, "space": 49,
]

func fail(_ message: String) -> Never {
    FileHandle.standardError.write("KEYPOST_ERROR: \(message)\n".data(using: .utf8)!)
    exit(2)
}

let args = CommandLine.arguments
guard args.count == 3, let pid = Int32(args[1]) else {
    fail("usage: keypost <pid> <chord>   (chord e.g. \"cmd+shift+l\", \"escape\")")
}

var flags: CGEventFlags = []
var key: CGKeyCode?
for part in args[2].lowercased().split(separator: "+").map(String.init) {
    switch part {
    case "cmd", "command", "meta": flags.insert(.maskCommand)
    case "shift": flags.insert(.maskShift)
    case "alt", "option": flags.insert(.maskAlternate)
    case "ctrl", "control": flags.insert(.maskControl)
    default:
        guard let code = KEYCODES[part] else { fail("unknown key \"\(part)\" in chord \"\(args[2])\"") }
        guard key == nil else { fail("chord \"\(args[2])\" names more than one key") }
        key = code
    }
}
guard let code = key else { fail("chord \"\(args[2])\" names no key") }

let source = CGEventSource(stateID: .hidSystemState)
guard let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true),
    let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false)
else {
    fail("could not synthesize a key event for \"\(args[2])\"")
}
down.flags = flags
up.flags = flags
down.postToPid(pid)
// A human holds a key for tens of milliseconds; a down/up posted in the same
// instant can be coalesced or dropped by the receiving app's event handling.
usleep(60_000)
up.postToPid(pid)

// Milestone PT — the macOS Accessibility (AX) query helper.
//
// WKWebView exposes no WebDriver endpoint, but it DOES publish its DOM to the
// macOS Accessibility tree — and, crucially for a test harness, WebKit surfaces
// each element's CSS class list on the private `AXDOMClassList` attribute. That
// lets the smoke driver assert the SAME semantic contract the WebdriverIO leg
// asserts against the live DOM (count `.filetree__row` rows, read the status-bar
// text, match a rendered heading), instead of a blurry "the pixels changed"
// heuristic. The AXUIElement API is used directly (not AppleScript System Events)
// because it walks the deep web-content subtree reliably and fast.
//
//   axquery count    <pid> <cssClass>   — print the number of elements whose
//                                          AXDOMClassList contains <cssClass>.
//   axquery text     <pid> [cssClass]   — print the text (AXValue/AXTitle/
//                                          AXDescription) of every element, or,
//                                          if <cssClass> is given, only of the
//                                          subtree(s) rooted at an element whose
//                                          class list contains it. One node per
//                                          line.
//   axquery headings <pid>              — print the text of every AXHeading, one
//                                          per line (a rendered <h1..h6>).
//
// Reads only; never mutates the UI. Requires the controlling process to hold the
// Accessibility (TCC) permission.
//
// EXIT CODES — the helper FAILS CLOSED so the caller (makeAx) can tell a genuine
// empty result from a broken read:
//   0  success. Output may be empty / `0` — but ONLY because the query genuinely
//      found nothing (the tree isn't populated yet, or no element carries the
//      class). Every AXUIElementCopyAttributeValue on the path returned success,
//      or a *legitimately-absent* attribute (kAXErrorNoValue /
//      kAXErrorAttributeUnsupported).
//   2  usage error (bad arguments).
//   3  a REAL AX read error (kAXErrorCannotComplete / kAXErrorAPIDisabled /
//      kAXErrorInvalidUIElement / kAXErrorNotImplemented / …). We do NOT print
//      `0` and exit 0 in this case — a broken read must never masquerade as
//      "count 0 / not found". The distinct `AX_READ_ERROR:` marker goes to
//      stderr and the nonzero exit makes the TS side THROW.
import ApplicationServices
import Foundation

/// A genuine AX read failure — distinct from a legitimately-absent attribute.
struct AXReadError: Error {
    let attribute: String
    let code: AXError
}

/// Copy attribute `name` off `el`, checking the AXError return code at THIS single
/// call site (every read below routes through here, so every call site is
/// covered). Returns nil ONLY when the attribute is legitimately absent
/// (`kAXErrorNoValue` — supported but unset; `kAXErrorAttributeUnsupported` — the
/// element doesn't have it, e.g. a menu item has no `AXDOMClassList`). THROWS on
/// any other non-success code, which are real errors (the app is unreachable,
/// the element ref is stale, the AX API is disabled, …).
func attr(_ el: AXUIElement, _ name: String) throws -> AnyObject? {
    var value: AnyObject?
    let code = AXUIElementCopyAttributeValue(el, name as CFString, &value)
    switch code {
    case .success:
        return value
    case .noValue, .attributeUnsupported:
        return nil
    default:
        throw AXReadError(attribute: name, code: code)
    }
}

func string(_ el: AXUIElement, _ name: String) throws -> String? { try attr(el, name) as? String }

func classList(_ el: AXUIElement) throws -> [String] {
    (try attr(el, "AXDOMClassList") as? [String]) ?? []
}

func children(_ el: AXUIElement) throws -> [AXUIElement] {
    (try attr(el, "AXChildren") as? [AXUIElement]) ?? []
}

func role(_ el: AXUIElement) throws -> String { (try string(el, "AXRole")) ?? "" }

/// The visible text a node contributes, from the AX attributes WebKit populates
/// for it (value for inputs/text, title/description for labelled controls).
func nodeText(_ el: AXUIElement) throws -> String {
    [try string(el, "AXValue"), try string(el, "AXTitle"), try string(el, "AXDescription")]
        .compactMap { $0 }
        .filter { !$0.isEmpty }
        .joined(separator: " ")
}

let args = CommandLine.arguments
guard args.count >= 3, let pid = Int32(args[2]) else {
    FileHandle.standardError.write(
        "usage: axquery count|text|headings <pid> [cssClass]\n".data(using: .utf8)!)
    exit(2)
}
let app = AXUIElementCreateApplication(pid_t(pid))

do {
    switch args[1] {
    case "count":
        guard args.count == 4 else { exit(2) }
        let cls = args[3]
        var count = 0
        func walk(_ el: AXUIElement) throws {
            if try classList(el).contains(cls) { count += 1 }
            for child in try children(el) { try walk(child) }
        }
        try walk(app)
        print(count)

    case "text":
        let scope = args.count > 3 ? args[3] : ""
        var lines: [String] = []
        func collect(_ el: AXUIElement) throws {
            let text = try nodeText(el)
            if !text.isEmpty { lines.append(text) }
            for child in try children(el) { try collect(child) }
        }
        if scope.isEmpty {
            try collect(app)
        } else {
            // Descend to each element whose class list contains `scope`, then gather
            // all text beneath it. Does not recurse past a match (a scope root
            // already yields its whole subtree).
            func find(_ el: AXUIElement) throws {
                if try classList(el).contains(scope) {
                    try collect(el)
                } else {
                    for child in try children(el) { try find(child) }
                }
            }
            try find(app)
        }
        print(lines.joined(separator: "\n"))

    case "headings":
        var lines: [String] = []
        func walk(_ el: AXUIElement) throws {
            if try role(el) == "AXHeading" {
                let text = try nodeText(el)
                if !text.isEmpty { lines.append(text) }
            }
            for child in try children(el) { try walk(child) }
        }
        try walk(app)
        print(lines.joined(separator: "\n"))

    default:
        FileHandle.standardError.write(
            "usage: axquery count|text|headings <pid> [cssClass]\n".data(using: .utf8)!)
        exit(2)
    }
} catch let error as AXReadError {
    // A REAL AX error — surface it distinctly and exit NONZERO so the caller
    // throws instead of reading a false 0 / empty.
    FileHandle.standardError.write(
        "AX_READ_ERROR: \(error.attribute) code=\(error.code.rawValue)\n".data(using: .utf8)!)
    exit(3)
} catch {
    FileHandle.standardError.write("AX_READ_ERROR: \(error)\n".data(using: .utf8)!)
    exit(3)
}

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
//                                          Exits 5 rather than printing 0 when the
//                                          class-list channel itself is not proven
//                                          alive (see EXIT CODES).
//   axquery text     <pid> [cssClass]   — print the text (AXValue/AXTitle/
//                                          AXDescription) of every element, or,
//                                          if <cssClass> is given, only of the
//                                          subtree(s) rooted at an element whose
//                                          class list contains it. One node per
//                                          line.
//   axquery headings <pid>              — print the text of every AXHeading, one
//                                          per line (a rendered <h1..h6>).
//   axquery frame    <pid> <cssClass> <nth> [substring]
//                                       — print `x y w h` (screen coordinates,
//                                          top-left origin) of the <nth>
//                                          (0-based, document order) element
//                                          whose class list contains <cssClass>
//                                          and whose own subtree text contains
//                                          <substring>, when given. Exits 4 when
//                                          there is no such element — a MISSING
//                                          control is not a zero-size one, and a
//                                          caller must never click an invented
//                                          point.
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
//   4  `frame` found no such element (or it has no position/size). Distinct
//      from 3 so a caller can say "that control is not on screen" rather than
//      "the AX read broke", and distinct from 0 so it can never be mistaken for
//      a usable rectangle.
//   5  `count` came out 0 while the CLASS-LIST CHANNEL ITSELF is not proven
//      alive: the app publishes no `AXWebArea` at all, not one element in its
//      whole tree carries a non-empty `AXDOMClassList`, or the walk could not
//      see the whole tree (it loops, or is deeper than MAX_DEPTH). Every AX call
//      succeeded, so this is not a
//      3 — but a 0 from a read path that could not have counted anything is not
//      evidence of absence, and `jumpToHit` asserts an absence (`.mdsearch`
//      count == 0) on exactly this number. This is not hypothetical: with the
//      login session LOCKED, macOS vends this app no window and no web area (its
//      `AXWindows[0]` is the application element itself, whose children loop back
//      to it), so every class count reads 0 while the app is running fine. See
//      `docs/testing/gui-e2e.md`. `AX_UNREADABLE:` goes to stderr; the TS side
//      retries briefly (the web area lands a beat after the window) and then
//      THROWS. A NON-ZERO count needs no witness — it is its own proof.
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

/// The element's screen rectangle, or nil when WebKit publishes neither
/// attribute for it (an off-screen / unrendered node).
func frame(_ el: AXUIElement) throws -> CGRect? {
    guard let positionValue = try attr(el, kAXPositionAttribute as String),
        let sizeValue = try attr(el, kAXSizeAttribute as String)
    else { return nil }
    var origin = CGPoint.zero
    var size = CGSize.zero
    guard AXValueGetValue(positionValue as! AXValue, .cgPoint, &origin),
        AXValueGetValue(sizeValue as! AXValue, .cgSize, &size)
    else { return nil }
    return CGRect(origin: origin, size: size)
}

/// How deep any walk below will descend before it stops and says so.
///
/// The AX tree is not always a tree. macOS publishes a DEGENERATE, SELF-LOOPING
/// one for an app whose windows it will not vend — observed here whenever the
/// login session is locked: the application element's `AXChildren[0]` is the
/// application element again, forever. An unguarded recursive walk follows that
/// until the stack dies (`SIGSEGV`, "Thread stack size exceeded due to excessive
/// recursion"), which is a crash where the caller deserves a diagnosis. The real
/// tree is nothing like this deep: the app's whole shell measures 9 levels below
/// the window, its menu bar 7, so 256 is two orders of margin.
let MAX_DEPTH = 256

/// Set when a walk had to stop short of the whole tree: it hit {@link MAX_DEPTH},
/// or it reached an element that is its own ancestor. Either way it did not see
/// everything, so a `count` of 0 taken under this flag is not evidence of
/// absence.
var treeTruncated = false

/// Has `el` already been entered on the path from the root — i.e. does following
/// it loop? The depth cap alone is not enough: a self-loop one level up from a
/// big subtree (an application element that lists itself AND a menu bar) makes
/// the walk re-enumerate that subtree once per level, which is 256 x the whole
/// menu tree — minutes of work for an answer that is already meaningless. This
/// cuts it at the first repeat instead. Cheap: the path is 9 deep in the real
/// tree, and `CFEqual` on an AXUIElement is a local comparison.
func loops(_ el: AXUIElement, _ path: [AXUIElement]) -> Bool {
    path.contains { CFEqual($0, el) }
}

let args = CommandLine.arguments
guard args.count >= 3, let pid = Int32(args[2]) else {
    FileHandle.standardError.write(
        "usage: axquery count|text|headings|frame <pid> [cssClass] [nth] [substring]\n".data(using: .utf8)!)
    exit(2)
}
let app = AXUIElementCreateApplication(pid_t(pid))

do {
    switch args[1] {
    case "count":
        guard args.count == 4 else { exit(2) }
        let cls = args[3]
        var count = 0
        // Two WITNESSES that the class-list channel is alive, gathered on the same
        // walk: WebKit's content root, and any element at all publishing a
        // non-empty class list. They cost nothing once found.
        var webAreaSeen = false
        var classListSeen = false
        var path: [AXUIElement] = []
        func walk(_ el: AXUIElement, _ depth: Int) throws {
            if depth > MAX_DEPTH || loops(el, path) {
                treeTruncated = true
                return
            }
            let classes = try classList(el)
            if !classes.isEmpty { classListSeen = true }
            if classes.contains(cls) { count += 1 }
            if !webAreaSeen, try role(el) == "AXWebArea" { webAreaSeen = true }
            path.append(el)
            for child in try children(el) { try walk(child, depth + 1) }
            path.removeLast()
        }
        try walk(app, 0)
        // FAIL CLOSED (exit 5): "no element carries this class" and "this process
        // cannot see element classes at all" are different facts, and only the
        // first one is an answer. Never print a 0 the caller would read as absence.
        if count == 0 && !(webAreaSeen && classListSeen && !treeTruncated) {
            FileHandle.standardError.write(
                "AX_UNREADABLE: count(\(cls)) = 0 but the class-list channel is not proven alive "
                    .appending("(AXWebArea seen: \(webAreaSeen), any AXDOMClassList seen: \(classListSeen), ")
                    .appending("whole tree walked: \(!treeTruncated)) ")
                    .appending("— refusing to report 0 as \"absent\"\n").data(using: .utf8)!)
            exit(5)
        }
        print(count)

    case "text":
        let scope = args.count > 3 ? args[3] : ""
        var lines: [String] = []
        var path: [AXUIElement] = []
        func collect(_ el: AXUIElement, _ depth: Int) throws {
            if depth > MAX_DEPTH || loops(el, path) {
                treeTruncated = true
                return
            }
            let text = try nodeText(el)
            if !text.isEmpty { lines.append(text) }
            path.append(el)
            for child in try children(el) { try collect(child, depth + 1) }
            path.removeLast()
        }
        if scope.isEmpty {
            try collect(app, 0)
        } else {
            // Descend to each element whose class list contains `scope`, then gather
            // all text beneath it. Does not recurse past a match (a scope root
            // already yields its whole subtree).
            func find(_ el: AXUIElement, _ depth: Int) throws {
                if depth > MAX_DEPTH || loops(el, path) {
                    treeTruncated = true
                    return
                }
                if try classList(el).contains(scope) {
                    try collect(el, depth)
                } else {
                    path.append(el)
                    for child in try children(el) { try find(child, depth + 1) }
                    path.removeLast()
                }
            }
            try find(app, 0)
        }
        print(lines.joined(separator: "\n"))

    case "headings":
        var lines: [String] = []
        var path: [AXUIElement] = []
        func walk(_ el: AXUIElement, _ depth: Int) throws {
            if depth > MAX_DEPTH || loops(el, path) {
                treeTruncated = true
                return
            }
            if try role(el) == "AXHeading" {
                let text = try nodeText(el)
                if !text.isEmpty { lines.append(text) }
            }
            path.append(el)
            for child in try children(el) { try walk(child, depth + 1) }
            path.removeLast()
        }
        try walk(app, 0)
        print(lines.joined(separator: "\n"))

    case "frame":
        guard args.count >= 5, let nth = Int(args[4]), nth >= 0 else { exit(2) }
        let cls = args[3]
        // Optional substring filter over the candidate's own subtree text, so a
        // caller can say "the jump button that cites catdb-core.md" instead of
        // guessing an ordinal that shifts whenever the report changes.
        let needle = args.count > 5 ? args[5] : ""
        var matches: [AXUIElement] = []
        func subtreeText(_ el: AXUIElement) throws -> String {
            var lines: [String] = []
            var inner: [AXUIElement] = []
            func collect(_ node: AXUIElement, _ depth: Int) throws {
                if depth > MAX_DEPTH || loops(node, inner) {
                    treeTruncated = true
                    return
                }
                let text = try nodeText(node)
                if !text.isEmpty { lines.append(text) }
                inner.append(node)
                for child in try children(node) { try collect(child, depth + 1) }
                inner.removeLast()
            }
            try collect(el, 0)
            return lines.joined(separator: " ")
        }
        var path: [AXUIElement] = []
        func walk(_ el: AXUIElement, _ depth: Int) throws {
            if depth > MAX_DEPTH || loops(el, path) {
                treeTruncated = true
                return
            }
            if try classList(el).contains(cls) {
                if try needle.isEmpty || subtreeText(el).contains(needle) { matches.append(el) }
            }
            path.append(el)
            for child in try children(el) { try walk(child, depth + 1) }
            path.removeLast()
        }
        try walk(app, 0)
        // FAIL CLOSED: no such element, or one the window server places
        // nowhere, is exit 4 — never a fabricated rectangle the caller would
        // then click.
        guard nth < matches.count, let rect = try frame(matches[nth]) else {
            FileHandle.standardError.write(
                "AX_NO_ELEMENT: \(cls)[\(nth)] matching \(needle.isEmpty ? "<any>" : needle) "
                    .appending("— \(matches.count) candidate(s)\n").data(using: .utf8)!)
            exit(4)
        }
        print("\(Int(rect.origin.x)) \(Int(rect.origin.y)) \(Int(rect.width)) \(Int(rect.height))")

    default:
        FileHandle.standardError.write(
            "usage: axquery count|text|headings|frame <pid> [cssClass] [nth] [substring]\n".data(using: .utf8)!)
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

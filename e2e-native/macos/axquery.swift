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
// Reads only; never mutates the UI. Exit 0 on success (possibly empty output —
// the app's AX tree may not be populated yet, which the driver handles by
// polling), exit 2 on a usage error. Requires the controlling process to hold
// the Accessibility (TCC) permission.
import ApplicationServices
import Foundation

func attr(_ el: AXUIElement, _ name: String) -> AnyObject? {
    var value: AnyObject?
    return AXUIElementCopyAttributeValue(el, name as CFString, &value) == .success ? value : nil
}

func string(_ el: AXUIElement, _ name: String) -> String? { attr(el, name) as? String }

func classList(_ el: AXUIElement) -> [String] {
    (attr(el, "AXDOMClassList") as? [String]) ?? []
}

func children(_ el: AXUIElement) -> [AXUIElement] {
    (attr(el, "AXChildren") as? [AXUIElement]) ?? []
}

func role(_ el: AXUIElement) -> String { string(el, "AXRole") ?? "" }

/// The visible text a node contributes, from the AX attributes WebKit populates
/// for it (value for inputs/text, title/description for labelled controls).
func nodeText(_ el: AXUIElement) -> String {
    [string(el, "AXValue"), string(el, "AXTitle"), string(el, "AXDescription")]
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

switch args[1] {
case "count":
    guard args.count == 4 else { exit(2) }
    let cls = args[3]
    var count = 0
    func walk(_ el: AXUIElement) {
        if classList(el).contains(cls) { count += 1 }
        for child in children(el) { walk(child) }
    }
    walk(app)
    print(count)

case "text":
    let scope = args.count > 3 ? args[3] : ""
    var lines: [String] = []
    func collect(_ el: AXUIElement) {
        let text = nodeText(el)
        if !text.isEmpty { lines.append(text) }
        for child in children(el) { collect(child) }
    }
    if scope.isEmpty {
        collect(app)
    } else {
        // Descend to each element whose class list contains `scope`, then gather
        // all text beneath it. Does not recurse past a match (a scope root
        // already yields its whole subtree).
        func find(_ el: AXUIElement) {
            if classList(el).contains(scope) {
                collect(el)
            } else {
                for child in children(el) { find(child) }
            }
        }
        find(app)
    }
    print(lines.joined(separator: "\n"))

case "headings":
    var lines: [String] = []
    func walk(_ el: AXUIElement) {
        if role(el) == "AXHeading" {
            let text = nodeText(el)
            if !text.isEmpty { lines.append(text) }
        }
        for child in children(el) { walk(child) }
    }
    walk(app)
    print(lines.joined(separator: "\n"))

default:
    FileHandle.standardError.write(
        "usage: axquery count|text|headings <pid> [cssClass]\n".data(using: .utf8)!)
    exit(2)
}

// Isolated fixture driver: actual Quartz input, never DOM-dispatched events.
import Foundation
import CoreGraphics
import AppKit
import ApplicationServices
let args = CommandLine.arguments
if args[1] == "activate" {
    guard let app = NSRunningApplication(processIdentifier: pid_t(Int32(args[2])!)) else { exit(3) }
    app.activate(options: [])
    if AXIsProcessTrusted() {
        let target = AXUIElementCreateApplication(pid_t(Int32(args[2])!))
        _ = AXUIElementSetAttributeValue(target, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
    }
    for _ in 0..<30 {
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        if NSWorkspace.shared.frontmostApplication?.processIdentifier == pid_t(Int32(args[2])!) { print("Fixture active"); exit(0) }
    }
    fputs("native_fixture_activation_unavailable\n", stderr); exit(3)
}
if args[1] == "window" {
    let pid = Int(args[2])!
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    guard let window = list.first(where: { ($0[kCGWindowOwnerPID as String] as? Int) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }),
          let bounds = window[kCGWindowBounds as String] as? [String: Any] else { exit(3) }
    var geometry = bounds
    geometry["Frontmost"] = NSWorkspace.shared.frontmostApplication?.processIdentifier == pid_t(pid)
    let data = try! JSONSerialization.data(withJSONObject: geometry)
    print(String(data: data, encoding: .utf8)!); exit(0)
}
if !CGPreflightPostEventAccess() {
    fputs("native_input_unavailable: process has no macOS event posting permission\n", stderr)
    exit(2)
}
if args[1] == "probe" { print("Quartz event posting authorized"); exit(0) }
let source = CGEventSource(stateID: .hidSystemState)
if args[1] == "escape" {
    // Native WebKit drops back-to-back synthesized transitions. Preserve a
    // measured physical key press interval; this is driver delivery timing,
    // not a delay used to pretend the picker's cleanup has succeeded.
    let down = CGEvent(keyboardEventSource: source, virtualKey: 53, keyDown: true)
    let up = CGEvent(keyboardEventSource: source, virtualKey: 53, keyDown: false)
    down?.flags = []; up?.flags = []
    down?.post(tap: .cghidEventTap)
    usleep(80000)
    up?.post(tap: .cghidEventTap)
    usleep(80000)
} else {
    let point = CGPoint(x: Double(args[2])!, y: Double(args[3])!)
    CGEvent(mouseEventSource: source, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
    usleep(80000)
    for index in 1...(args[1] == "double" ? 2 : 1) {
        let down = CGEvent(mouseEventSource: source, mouseType: .leftMouseDown, mouseCursorPosition: point, mouseButton: .left)
        down?.setIntegerValueField(.mouseEventClickState, value: Int64(index)); down?.post(tap: .cghidEventTap)
        usleep(80000)
        let up = CGEvent(mouseEventSource: source, mouseType: .leftMouseUp, mouseCursorPosition: point, mouseButton: .left)
        up?.setIntegerValueField(.mouseEventClickState, value: Int64(index)); up?.post(tap: .cghidEventTap)
        usleep(100000)
    }
}

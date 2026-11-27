import AppKit
import ScreenCaptureKit

private let logger = ShotLog("ShotRenderer")

/// A scene's window the window server does not show. `capture` turns it into
/// `ShotError.windowNotDrawn` with the state the window server and the
/// displays hold, which only the main actor reads.
struct WindowNotDrawn: Error {
    let sceneId: String
    let frame: CGRect
}

extension ShotRenderer {
    /// Put `window` wholly on `display`, at the top left of its free area:
    /// the screen capture takes only what is on a display, and would cut off
    /// or refuse a window past a display's edge, or a title bar under the
    /// menu bar.
    ///
    /// AppKit can shrink a titled window that is larger than its display
    /// as it sizes it, so a window whose content is no longer the
    /// scene's size did not fit either, and its capture would draw the scene
    /// at a size its board never has.
    static func place(
        _ window: NSWindow,
        on display: ShotDisplay,
        scene: ShotScene
    ) throws {
        let screen = try display.screen()
        let room = try display.visibleFrame()
        logger.info(
            "The renderer is placing scene \(scene.id), window \(window.windowNumber) with frame \(window.frame), "
                + "on display \(display.id) with frame \(screen.frame) at \(screen.backingScaleFactor)x "
                + "and free area \(room)."
        )
        guard let shown = window.contentView?.frame.size else {
            throw ShotError.noContentView(scene.id)
        }
        guard
            shown == scene.size,
            window.frame.width <= room.width,
            window.frame.height <= room.height
        else {
            throw ShotError.windowTooLarge(
                scene.id,
                content: scene.size,
                window: window.frame.size,
                shown: shown,
                room: room.size
            )
        }
        window.setFrameOrigin(
            NSPoint(x: room.minX, y: room.maxY - window.frame.height)
        )
    }

    /// Orders `window` in. The window is a non-activating panel, key and
    /// main without the app taking the focus from the app that holds it,
    /// unless the scene is drawn inactive, which leaves it neither.
    static func show(_ window: NSWindow, for scene: ShotScene) {
        if scene.inactive {
            window.orderFront(nil)
        }
        else {
            window.makeKeyAndOrderFront(nil)
            window.makeMain()
        }
    }

    /// Closes each window still up once a scene's own window is closed, and
    /// returns what each was: the scene that left them fails. An alert raised
    /// before its view is in its window hangs from the key window, which a
    /// sheet left up would be, and every capture after it would draw the
    /// alert over that sheet.
    static func closeWindowsLeftUp() -> [String] {
        // A sheet's presentation window can be gone from AppKit's view while
        // the window server still shows it. SwiftUI keeps the window once it
        // is down, so it counts only while one of the two shows it.
        NSApplication.shared.windows
            .filter {
                $0.isVisible
                    || $0.isSheetPresentation && $0.mayBeOnScreen
            }
            .map { stray in
                let described =
                    "\(stray.className) \"\(stray.title)\" with frame "
                    + "\(stray.frame), sheet of "
                    + "\(stray.sheetParent?.title ?? "none")"
                if let parent = stray.sheetParent {
                    parent.endSheet(stray)
                }
                stray.orderOut(nil)
                stray.close()
                return described
            }
    }

    /// Which view holds the keyboard in `window` when the scene is taken, so
    /// a field that draws focused is explained by the log.
    static func logFocus(of window: NSWindow, scene: ShotScene) {
        logger.info(
            "scene \(scene.id) first responder \(String(describing: window.firstResponder)) key \(window.isKeyWindow)"
        )
    }

    /// Wait for the window or its top sheet or alert to be key, asking
    /// for that again whenever it loses it: the window server draws a title
    /// bar and toolbar as inactive otherwise, which the app never shows in
    /// use. The window and the alert panels over it are non-activating
    /// panels, key while another app holds the focus.
    /// Fails, naming the front app, when the window never becomes key.
    static func becomeKey(
        _ window: NSWindow,
        dressing: any ShotDressing,
        scene: ShotScene
    ) async throws {
        // A scene drawn inactive leaves its window without the key.
        guard !scene.inactive else { return }
        let changes = [
            NSWindow.didBecomeKeyNotification,
            NSWindow.didResignKeyNotification,
            NSApplication.didBecomeActiveNotification,
            NSApplication.didResignActiveNotification,
        ]
        .map { Wait.Notice($0, of: nil) }
        do {
            try await Wait.until(notices: changes) {
                // An alert over the window holds the key, and the window draws
                // as active under it: the buttons of an alert whose panel is
                // not key draw as inactive.
                let keyed =
                    dressing.overlays.last?.panel ?? window.attachedSheet
                    ?? window
                if keyed.isKeyWindow { return true }
                keyed.makeKeyAndOrderFront(nil)
                return false
            }
        }
        catch is Wait.TimedOut {
            throw ShotError.windowNotKey(scene.id, focus: .now())
        }
        // Draw the frame as active and hand it to the window server.
        window.displayIfNeeded()
        CATransaction.flush()
    }

    /// The panels the app puts over the window, once each is shown, as the
    /// app puts an alert up: a clear panel of its own over the window, which
    /// the window server holds apart from the window it sits on.
    static func shownPanels(of dressing: any ShotDressing) async throws
        -> [NSPanel]
    {
        // The app shows an alert when the alert's view appears, a few run
        // loop passes after AppKit orders the window in.
        // An alert is a panel, which hides while its app is not active, as
        // this app never is, so each is kept up.
        try await Wait.until {
            let panels = dressing.overlays.map(\.panel)
            for panel in panels {
                panel.hidesOnDeactivate = false
                if !panel.isVisible { panel.orderFront(nil) }
            }
            return panels.allSatisfy { $0.isVisible }
        }
        return dressing.overlays.map(\.panel)
    }

    /// Wait for the window server to show each of `windows` on the screen,
    /// which it tells AppKit through each window's occlusion state: the
    /// window server lists a window it has not shown yet with an empty frame,
    /// and ScreenCaptureKit refuses a capture sized from that frame.
    static func shownOnScreen(
        _ windows: [NSWindow],
        scene: ShotScene
    ) async throws {
        let changes = windows.map {
            Wait.Notice(NSWindow.didChangeOcclusionStateNotification, of: $0)
        }
        do {
            try await Wait.until(notices: changes) {
                windows.allSatisfy { $0.occlusionState.contains(.visible) }
            }
        }
        catch is Wait.TimedOut {
            let hidden = windows.first {
                !$0.occlusionState.contains(.visible)
            }
            throw WindowNotDrawn(
                sceneId: scene.id,
                frame: hidden?.frame ?? .zero
            )
        }
    }

    /// The pixels of `window` and its overlays once the window server shows
    /// them. A window it does not show fails with what the window server and
    /// the displays hold then.
    static func windowPNG(
        of window: NSWindow,
        panels: [NSPanel],
        dressing: any ShotDressing,
        on display: ShotDisplay,
        scene: ShotScene
    ) async throws -> Data {
        let sheets = window.attachedSheet.map { [$0] } ?? []
        let shown = [window] + sheets + dressing.overlays.map(\.panel)
        do {
            try await shownOnScreen(shown, scene: scene)
            return try await windowPNG(
                CGWindowID(window.windowNumber),
                sheets: sheets.map { CGWindowID($0.windowNumber) },
                shadowed: sheets.filter(\.hasShadow)
                    .map { CGWindowID($0.windowNumber) },
                overlays: panels.map { CGWindowID($0.windowNumber) },
                on: display.id,
                sceneId: scene.id
            )
        }
        catch let missing as WindowNotDrawn {
            throw ShotError.windowNotDrawn(
                missing.sceneId,
                missing.frame,
                .read(
                    windowIDs: shown.map { CGWindowID($0.windowNumber) },
                    on: display
                )
            )
        }
    }

    /// The window, its sheets and its overlays as the window server composes
    /// them on the screen, with the shadows of the sheets that cast one
    /// (`shadowed`, laid by `ShotSheetShadow`), as PNG bytes: a window's toolbar draws its controls on glass,
    /// and sheets and alerts are windows of their own. The capture takes what
    /// the screen shows over the parent window's frame.
    /// The window server must already show every one of the windows
    /// on the screen, so it lists each with its frame. A process may capture
    /// its own windows without the screen recording permission.
    /// ScreenCaptureKit's results aren't `Sendable`, so the capture runs and
    /// encodes off the main actor and hands back only the PNG bytes.
    @concurrent
    nonisolated static func windowPNG(
        _ windowID: CGWindowID,
        sheets: [CGWindowID],
        shadowed: [CGWindowID],
        overlays: [CGWindowID],
        on displayID: CGDirectDisplayID,
        sceneId: String
    ) async throws -> Data {
        let content = try await SCShareableContent.currentProcess
        let over = sheets + overlays
        let held = ([windowID] + over)
            .compactMap { id in
                content.windows.first { $0.windowID == id }
            }
        guard
            let parent = held.first(where: { $0.windowID == windowID }),
            held.count == over.count + 1
        else {
            throw ShotError.windowNotShared(sceneId)
        }
        guard held.allSatisfy({ !$0.frame.isEmpty && $0.isOnScreen }) else {
            throw WindowNotDrawn(sceneId: sceneId, frame: parent.frame)
        }
        guard
            let display = content.displays.first(where: {
                $0.displayID == displayID
            })
        else { throw ShotError.noScreen(sceneId) }
        let filter = SCContentFilter(display: display, including: held)
        let configuration = SCStreamConfiguration()
        configuration.sourceRect = CGRect(
            x: parent.frame.minX - display.frame.minX,
            y: parent.frame.minY - display.frame.minY,
            width: parent.frame.width,
            height: parent.frame.height
        )
        configuration.width = Int(parent.frame.width) * scale
        configuration.height = Int(parent.frame.height) * scale
        configuration.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(
            contentFilter: filter,
            configuration: configuration
        )
        let scaled = try ShotImage.scaled(
            image,
            width: configuration.width,
            height: configuration.height,
            sceneId: sceneId
        )
        let shaded = try await ShotSheetShadow.cast(
            by: held.filter { shadowed.contains($0.windowID) },
            under: held.filter { overlays.contains($0.windowID) },
            on: scaled,
            of: parent,
            sceneId: sceneId
        )
        let rep = NSBitmapImageRep(cgImage: shaded)
        rep.size = parent.frame.size
        guard let data = rep.representation(using: .png, properties: [:])
        else {
            throw ShotError.pngEncodingFailed(sceneId)
        }
        return data
    }
}

extension NSWindow {
    /// SwiftUI's window for a sheet it presents.
    fileprivate var isSheetPresentation: Bool {
        className.contains("SheetPresentationWindow")
    }

    /// Whether the window server shows the window on the screen, or gives no
    /// list that says it does not.
    fileprivate var mayBeOnScreen: Bool {
        // A window with no number was never given to the window server.
        guard windowNumber > 0 else { return false }
        guard
            let listed = CGWindowListCopyWindowInfo(
                [.optionIncludingWindow],
                CGWindowID(windowNumber)
            ) as? [[String: Any]]
        else { return true }
        // The window server leaves the key out for a window that is not on
        // the screen.
        return listed.contains {
            $0[kCGWindowIsOnscreen as String] as? Bool == true
        }
    }
}

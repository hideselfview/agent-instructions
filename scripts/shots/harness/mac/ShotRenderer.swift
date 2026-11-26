import AppKit
import ScreenCaptureKit
import SwiftUI

/// Renders a `ShotScene` into a deterministic 2x PNG. The view is hosted in a
/// window dressed by the app's look, laid out, settled, then captured at
/// a fixed pixel scale regardless of the host machine's display: a scene's
/// view alone from a window past the edge of every display into a manually
/// sized bitmap, without the app ever taking the focus; a scene shown in its
/// window through the window server, which draws the toolbar's glass, from a
/// non-activating window made key on the `ShotDisplay`, without the app
/// taking the focus; an alert it shows is a non-activating panel too.
@MainActor
enum ShotRenderer {
    /// Backing-store scale for every capture. Fixed rather than read from the
    /// screen so a headless or non-Retina host produces the same pixels.
    nonisolated static let scale = ShotDisplay.scale

    /// What one capture of a scene yields: its pixels, and the elements its
    /// hosted view's accessibility tree lists, read in the same settled state.
    struct Capture {
        let png: Data
        let elements: ShotElementList
    }

    /// `scene` captured in `look`; a component scene is drawn at its size in
    /// `boardSizes`, and a scene shown in its window on `display`.
    static func render(
        _ scene: ShotScene,
        in look: some ShotLook,
        boardSizes: ShotBoardSizes,
        on display: ShotDisplay?
    ) async throws -> Capture {
        let windowScene = try WindowScene(scene: scene, display: display)
        closeStrayWindows(before: scene)
        let stage = try scene.stage()
        let dressing = try look.dress(stage.view, framedAs: scene.frame)
        defer { dressing.undress() }
        let (window, captured) = try hostWindow(
            dressing.rootView.shotStill(),
            for: scene,
            size: try frameSize(of: scene, boardSizes: boardSizes),
            windowScene: windowScene
        )
        dressing.dress(window)
        window.isReleasedWhenClosed = false
        // Close the window however the render ends, a failure to place it
        // among them: the scene's view raises its alerts once it is in the
        // window, shown or not, and an alert left up is listed and captured
        // with every scene after it.
        defer {
            if let sheet = window.attachedSheet {
                window.endSheet(sheet)
                sheet.orderOut(nil)
            }
            window.contentViewController = nil
            window.contentView = nil
            window.close()
        }
        if let windowScene {
            try place(window, on: windowScene.display, scene: scene)
            show(window, for: scene)
        }
        else {
            // The renderer makes the window key without activating the app.
            // The window sits past every display, and a focused field draws
            // its ring only in a key window.
            window.orderFront(nil)
            window.makeKey()
        }

        try await settle(
            host: captured,
            until: stage.isReady,
            drawn: stage.isDrawn
        )

        return try await capture(
            window,
            host: captured,
            scene: scene,
            windowScene: windowScene,
            dressing: dressing
        )
    }

    /// The pixels of `host` and the elements below it, once settled.
    private static func capture(
        _ window: NSWindow,
        host: NSView,
        scene: ShotScene,
        windowScene: WindowScene?,
        dressing: any ShotDressing
    ) async throws -> Capture {
        let pixels: Data
        if let windowScene {
            // The app shows the alerts a few run loop passes after AppKit
            // orders the window in, so the renderer waits for them before it
            // makes the top one key.
            let panels = try await shownPanels(of: dressing)
            try await becomeKey(window, dressing: dressing, scene: scene)
            logFocus(of: window, scene: scene)
            pixels = try await windowPNG(
                of: window,
                panels: panels,
                dressing: dressing,
                on: windowScene.display,
                scene: scene
            )
        }
        else {
            pixels = try png(of: host, size: host.bounds.size, scene: scene)
        }
        // Nothing awaits between the pixels and the tree, so both read the
        // same settled state.
        return Capture(
            png: pixels,
            elements: try ShotElementList.read(
                board: scene.id,
                window: window,
                overlays: dressing.overlays,
                size: host.bounds.size,
                scale: scale
            )
        )
    }

    /// The window `rootView` is shown in for `scene`, and the view the
    /// capture settles: the view alone in a borderless window, or for a
    /// scene shown in its window, the whole frame, title bar and toolbar
    /// included, the way the app's window controller hosts it.
    private static func hostWindow(
        _ rootView: some View,
        for scene: ShotScene,
        size: CGSize,
        windowScene: WindowScene?
    ) throws -> (NSWindow, NSView) {
        let bounds = NSRect(origin: .zero, size: size)
        if let windowScene {
            let hosting = NSHostingController(rootView: rootView)
            // A window with a hidden title bar has no toolbar either: its
            // content holds what a toolbar would.
            hosting.sceneBridgingOptions =
                windowScene.titleBar == .hidden ? [] : .toolbars
            // The window's size is the scene's, whatever the content asks for.
            hosting.sizingOptions = []
            // Made on the shot display, so AppKit sizes it against that
            // display rather than the Mac's own.
            let origin = try windowScene.display.visibleFrame().origin
            let window = shownWindow(for: scene, at: origin)
            if windowScene.titleBar == .hidden {
                // SwiftUI's hidden-title-bar window lets its content reach
                // the top edge; the hosting view otherwise keeps the title
                // bar's height as a safe area and shifts the content down.
                hosting.safeAreaRegions = []
                window.showOnlyWindowButtonsInShot()
            }
            // Ordering a window in plays its zoom-in, which the window
            // server's capture can catch partway.
            window.animationBehavior = .none
            window.contentViewController = hosting
            if windowScene.titleBar == .hidden {
                // The content reaches under the title bar, so the frame is
                // the scene's size; sizing the content would add the title
                // bar's height below it, an empty strip in the capture.
                window.setFrame(
                    NSRect(origin: window.frame.origin, size: scene.size),
                    display: false
                )
            }
            else {
                window.setContentSize(scene.size)
            }
            window.title = windowScene.title
            guard let frame = window.contentView?.superview else {
                preconditionFailure("A titled window has a frame view")
            }
            return (window, frame)
        }
        // Drawn as in the key window of the active app, as the app is
        // seen in use, without making it either: a window of an app that
        // is not active is never key, and draws its controls inactive.
        let host = NSHostingView(
            rootView: rootView.environment(\.controlActiveState, .key)
        )
        host.frame = bounds
        let window = contentWindow(for: scene, size: size)
        // Ordering a window in plays its zoom-in on a thread of its own,
        // paced by the display it is on; past every display there is
        // none, and the animation would never end.
        window.animationBehavior = .none
        window.contentView = host
        return (window, host)
    }

    /// The borderless window that holds a scene's view alone, past the edge
    /// of every display: a panel that does not activate the app when the
    /// scene needs its window key.
    private static func contentWindow(
        for scene: ShotScene,
        size: CGSize
    ) -> NSWindow {
        let contentRect = NSRect(origin: offscreenOrigin, size: size)
        guard scene.keyWindow else {
            return ShotWindow(
                contentRect: contentRect,
                styleMask: [.borderless],
                backing: .buffered,
                defer: false
            )
        }
        let panel = ShotPanel(
            contentRect: contentRect,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        // A panel hides when its app is not active, which this app never is.
        panel.hidesOnDeactivate = false
        return panel
    }

    /// The empty window of a scene shown in its window: a panel that does
    /// not activate the app.
    private static func shownWindow(
        for scene: ShotScene,
        at origin: NSPoint
    ) -> NSWindow {
        let contentRect = NSRect(origin: origin, size: scene.size)
        let titled: NSWindow.StyleMask = [
            .titled, .closable, .miniaturizable, .resizable,
        ]
        let panel = ShotPanel(
            contentRect: contentRect,
            styleMask: titled.union(.nonactivatingPanel),
            backing: .buffered,
            defer: false
        )
        // A panel hides when its app is not active, which this app never is.
        panel.hidesOnDeactivate = false
        return panel
    }

    /// The size a scene is drawn at: a component's is its board's, which
    /// the view is proposed and the shot is cut to; any other scene's is its
    /// own.
    private static func frameSize(
        of scene: ShotScene,
        boardSizes: ShotBoardSizes
    ) throws -> CGSize {
        switch scene.frame {
        case .component: try boardSizes.size(of: scene.id)
        case .content, .window: scene.size
        }
    }

    /// A point past the right edge of every display, where a window shows
    /// on none of them.
    private static var offscreenOrigin: NSPoint {
        let right = NSScreen.screens.map(\.frame.maxX).max() ?? 0
        return NSPoint(x: right + 1_000, y: 0)
    }

    /// `host`'s pixels at `size` points and `scale`, over the window
    /// background, as PNG bytes.
    private static func png(
        of host: NSView,
        size: CGSize,
        scene: ShotScene
    ) throws -> Data {
        let bounds = NSRect(origin: .zero, size: size)
        guard
            let rep = NSBitmapImageRep(
                bitmapDataPlanes: nil,
                pixelsWide: Int(size.width) * scale,
                pixelsHigh: Int(size.height) * scale,
                bitsPerSample: 8,
                samplesPerPixel: 4,
                hasAlpha: true,
                isPlanar: false,
                colorSpaceName: .deviceRGB,
                bytesPerRow: 0,
                bitsPerPixel: 0
            )?
            .retagging(with: .sRGB)
        else {
            throw ShotError.bitmapAllocationFailed(scene.id)
        }
        // Point size on a 2x-pixel backing → cacheDisplay renders at 2x.
        rep.size = size
        host.cacheDisplay(in: bounds, to: rep)
        // The app draws the window's background under what the view leaves
        // unpainted; the capture alone would leave it transparent.
        let composed = try WindowBackdrop.composite(rep, of: host, size: size)

        guard let data = composed.representation(using: .png, properties: [:])
        else {
            throw ShotError.pngEncodingFailed(scene.id)
        }
        return data
    }

    /// Wait until the scene says what it shows has arrived, then until no
    /// frame in `host` moves across an idle of the main run loop, and again
    /// if coming to rest asked for more. The awaits are what let the
    /// scene's main-actor work run; blocking the main thread would starve
    /// it.
    ///
    /// An idle can come while main-actor work is still queued, so a tree
    /// can read the same across one before a chain of main-actor steps that
    /// moves no frame has done anything; a scene whose reads start that way
    /// names in `isReady` what they bring, such as a store's list. What a view
    /// builds from a read it makes itself, which no store publishes, is
    /// named in `isDrawn`, read from the laid-out tree.
    private static func settle(
        host: NSView,
        until isReady: @MainActor () -> Bool,
        drawn isDrawn: @MainActor (NSView) -> Bool
    ) async throws {
        while true {
            try await Wait.until { isReady() }
            try await Wait.untilLaidOut(host) { isDrawn(host) }
            try await Wait.untilSteady {
                host.layoutSubtreeIfNeeded()
                return frames(below: host)
            }
            if isReady(), isDrawn(host) { return }
        }
    }

    /// The frames of every view below `view`, depth first.
    private static func frames(below view: NSView) -> [CGRect] {
        view.subviews.flatMap { [$0.frame] + frames(below: $0) }
    }
}

@MainActor
private struct WindowScene {
    let display: ShotDisplay
    let title: String
    let titleBar: ShotFrame.TitleBar

    init?(scene: ShotScene, display: ShotDisplay?) throws {
        guard case .window(let title, let titleBar) = scene.frame else {
            return nil
        }
        guard let display else {
            throw ShotDisplayError("No display lease exists for \(scene.id).")
        }
        self.display = display
        self.title = title
        self.titleBar = titleBar
    }
}

private final class ShotWindow: NSWindow {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }

    /// The window keeps the size the scene gives it. AppKit fits a window to
    /// a display as it sizes it and orders it in, and can fit it to the Mac's
    /// own display rather than the shot display the window is on; `place`
    /// refuses a window that does not fit the shot display instead.
    override func constrainFrameRect(
        _ frameRect: NSRect,
        to screen: NSScreen?
    ) -> NSRect { frameRect }
}

/// The window of a scene shown in its window, or of a scene that needs its
/// view in a key window. A panel that does not activate the app is key, and draws its title bar and toolbar as active,
/// while another app holds the focus: a window of an app that is not active
/// is never key.
private final class ShotPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }

    /// The panel keeps the size the scene gives it, as `ShotWindow` does.
    override func constrainFrameRect(
        _ frameRect: NSRect,
        to screen: NSScreen?
    ) -> NSRect { frameRect }
}

extension NSWindow {
    /// Lets the content reach the top of the window, under its buttons.
    fileprivate func showOnlyWindowButtonsInShot() {
        styleMask.insert(.fullSizeContentView)
        titlebarAppearsTransparent = true
        titleVisibility = .hidden
    }
}

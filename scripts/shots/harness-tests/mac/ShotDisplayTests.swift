import AppKit
import SwiftUI
import Testing

@testable import ShotHarness

/// A look that adds nothing to a scene's view or window.
private struct PlainLook: ShotLook {
    func dress(_ view: AnyView, framedAs frame: ShotFrame) -> any ShotDressing {
        PlainDressing(rootView: view)
    }
}

private struct PlainDressing: ShotDressing {
    let rootView: AnyView
    let overlays: [ShotOverlay] = []

    func dress(_ window: NSWindow) {}

    func undress() {}
}

/// The test process as a running background app, which `swift test`'s is
/// not until asked: AppKit lists a new display, and the accessibility API
/// gives a window's tree, only in an app that runs its event loop.
@MainActor
private enum TestApp {
    enum Failure: Error {
        case refusedBackgroundPolicy
    }

    private static var launching: Task<Void, any Error>?

    /// Return once the app runs. The first call starts it.
    static func launch() async throws {
        let task = launching ?? Task { try await start() }
        launching = task
        try await task.value
    }

    private static func start() async throws {
        guard NSApplication.shared.setActivationPolicy(.accessory) else {
            throw Failure.refusedBackgroundPolicy
        }
        await withCheckedContinuation { running in
            // A run loop block, since the event loop never returns: started
            // from the main queue it would hold the queue, and no main-actor
            // work would run again.
            CFRunLoopPerformBlock(
                CFRunLoopGetMain(),
                CFRunLoopMode.commonModes.rawValue
            ) {
                MainActor.assumeIsolated {
                    DispatchQueue.main.async { running.resume() }
                    NSApplication.shared.run()
                }
            }
            CFRunLoopWakeUp(CFRunLoopGetMain())
        }
    }
}

/// The display the tests draw on: one for the process, so that no test adds a
/// display within milliseconds of another's removal.
@MainActor
private enum TestDisplay {
    private static var adding: Task<ShotDisplay, any Error>?

    static func get() async throws -> ShotDisplay {
        let task =
            adding ?? Task { try await ShotDisplay.add(size: ShotDisplay.size) }
        adding = task
        return try await task.value
    }
}

/// What the harness does with a real window and a real display. The tests run
/// one at a time, since each reads the process's windows.
@MainActor
@Suite(.serialized)
struct ShotDisplayTests {
    init() async throws {
        try await TestApp.launch()
    }

    /// The window server lists a window of this process with its frame and
    /// layer, and `read` finds it by its number.
    @Test
    func readFindsTheWindowServersEntryForAWindowOfThisProcess() async throws {
        let display = try await TestDisplay.get()
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 120, height: 80),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.animationBehavior = .none
        window.isReleasedWhenClosed = false
        window.orderFront(nil)
        defer { window.close() }
        // The window server lists a window it has not shown yet with an empty
        // frame, which is the failure this state names.
        try await Wait.until(
            notices: [
                Wait.Notice(
                    NSWindow.didChangeOcclusionStateNotification,
                    of: window
                )
            ]
        ) {
            window.occlusionState.contains(.visible)
        }
        let number = CGWindowID(window.windowNumber)
        let state = ShotWindowState.read(
            windowIDs: [number, 0xFFFF_FFF0],
            on: display
        )
        guard case .read(let listings, _) = state.windows else {
            Issue.record("the window server gave no list: \(state)")
            return
        }
        let listed = try #require(listings.first?.entry)
        #expect(listed.id == number)
        #expect(
            listed.frame.size == CGSize(width: 120, height: 80),
            "\(listed)"
        )
        #expect(listings.last?.entry == nil)
        #expect(state.displays.contains { $0.id == display.id })
        #expect(state.holder.contains("owned by this process"))
    }

    /// A titled window lists its title before its content, as VoiceOver reads
    /// it and a board draws it, and leaves out the window's close, minimise
    /// and zoom buttons, which a board hides. The window sits past every
    /// display, so it takes no focus.
    @Test
    func listsATitledWindowsTitleFirstWithoutItsButtons() throws {
        let size = CGSize(width: 400, height: 200)
        let right = NSScreen.screens.map(\.frame.maxX).max() ?? 0
        let window = NSWindow(
            contentRect: NSRect(
                origin: NSPoint(x: right + 1_000, y: 0),
                size: size
            ),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.animationBehavior = .none
        window.isReleasedWhenClosed = false
        window.title = "Window Title"
        let body = NSTextField(labelWithString: "Body words")
        body.frame = NSRect(x: 20, y: 20, width: 200, height: 20)
        window.contentView?.addSubview(body)
        window.orderFront(nil)
        defer { window.close() }

        let list = try ShotElementList.read(
            board: "a titled window",
            window: window,
            overlays: [],
            size: size,
            scale: 2
        )
        let json = try #require(String(bytes: try list.json(), encoding: .utf8))
        #expect(
            list.elements.compactMap(\.label) == [
                "Window Title", "Body words",
            ],
            "\(json)"
        )
        #expect(!list.elements.contains { $0.role == "button" }, "\(json)")
        for (index, element) in list.elements.enumerated() {
            if let parent = element.parent {
                #expect(parent < index, "\(json)")
            }
        }
    }

    /// AppKit would shrink the window to fit the shot display; the render
    /// fails instead of capturing the scene narrower than it is.
    @Test
    func aWindowWiderThanTheShotDisplayFails() async throws {
        let display = try await TestDisplay.get()
        await expectOversizedWindowToFail(on: display)
    }

    @Test
    func removingAnotherDisplayKeepsTheShotDisplayAtItsSizeAndScale()
        async throws
    {
        let display = try await TestDisplay.get()
        let otherID: CGDirectDisplayID
        do {
            let other = try await ShotDisplay.add(size: ShotDisplay.size)
            otherID = other.id
            let scene = ShotScene(
                id: "a scene on the display that closes",
                size: CGSize(width: 300, height: 200),
                frame: .window(title: "Shot", titleBar: .hidden)
            ) {
                ShotStage(Color.clear)
            }
            _ = try await render(scene, on: other)
        }
        try await ShotDisplay.waitUntilRemoved(otherID)

        let screen = try display.screen()
        #expect(screen.frame.size == CGSize(width: 1_600, height: 1_200))
        #expect(screen.backingScaleFactor == 2)
        await expectOversizedWindowToFail(on: display)
    }

    /// A scene whose window has a sheet over it leaves no window of its own
    /// up: the next scene's alert, raised before its view is in its window,
    /// would otherwise hang from the sheet left behind.
    @Test
    func aSceneWithASheetLeavesNoWindowUp() async throws {
        let display = try await TestDisplay.get()
        let scene = ShotScene(
            id: "a scene with a sheet",
            size: CGSize(width: 600, height: 400),
            frame: .window(title: "Shot")
        ) {
            ShotStage(
                Color.clear.sheet(isPresented: .constant(true)) {
                    Text("Sheet words").frame(width: 300, height: 120)
                },
                isDrawn: { $0.window?.attachedSheet != nil }
            )
        }
        let before = visibleWindows()
        let capture = try await render(scene, on: display)
        // The list holds the sheet's words, so the sheet was up.
        #expect(
            capture.elements.elements.contains { $0.label == "Sheet words" }
        )
        let left = NSApplication.shared.windows.filter {
            $0.isVisible && !before.contains($0.windowNumber)
        }
        #expect(left.isEmpty, "\(left.map { String(describing: $0.frame) })")
    }

    /// A shot of a window with a system sheet holds the shadow the window
    /// server draws around the sheet, and neither the dark line it draws just
    /// outside the sheet nor the light one just inside it.
    @Test
    func aSheetCastsItsShadowWithoutTheWindowServersRim() async throws {
        let display = try await TestDisplay.get()
        let behind = Color(.sRGB, white: 128.0 / 255)
        let face = Color(.sRGB, white: 200.0 / 255)
        let scene = ShotScene(
            id: "a sheet over a flat window",
            size: CGSize(width: 600, height: 400),
            frame: .window(title: "Shot", titleBar: .hidden)
        ) {
            ShotStage(
                behind.ignoresSafeArea()
                    .sheet(isPresented: .constant(true)) {
                        face.frame(width: 300, height: 160)
                    },
                isDrawn: { $0.window?.attachedSheet != nil }
            )
        }
        let capture = try await render(scene, on: display)
        let image = try #require(NSBitmapImageRep(data: capture.png))
        func gray(_ x: Int, _ y: Int) -> Int {
            var pixel = [Int](repeating: 0, count: 4)
            image.getPixel(&pixel, atX: x, y: y)
            return pixel[0]
        }
        // The sheet is the one part lighter than the window behind it.
        let middle = image.pixelsWide / 2
        let sheetRows = (0..<image.pixelsHigh).filter { gray(middle, $0) > 170 }
        let top = try #require(sheetRows.first)
        let bottom = try #require(sheetRows.last)
        let row = (top + bottom) / 2
        let sheetColumns = (0..<image.pixelsWide).filter { gray($0, row) > 170 }
        let left = try #require(sheetColumns.first)

        let far = gray(10, row)
        let beside = [1, 2, 4, 8].map { gray(left - $0, row) }
        let below = [1, 2, 4, 8].map { gray(middle, bottom + $0) }
        for edge in [beside, below] {
            #expect(edge.allSatisfy { $0 < far - 3 }, "\(edge) beside \(far)")
            // A rim outside the sheet is far darker than the shadow a few
            // pixels further out.
            #expect(abs(edge[0] - edge[2]) <= 3, "\(edge)")
        }
        // Lower than the sheet is above it, the shadow is darker below.
        #expect(below[3] < gray(middle, top - 8), "\(below)")
        // A rim inside the sheet is lighter than the sheet's face.
        let inside = (0..<6).map { gray(left + $0, row) }
        #expect(
            inside.allSatisfy { abs($0 - gray(middle, row)) <= 2 },
            "\(inside)"
        )
    }

    /// A window the scene's own teardown leaves up fails the scene that left
    /// it, and is closed, so that no later capture draws it.
    @Test
    func aSceneThatLeavesAWindowUpFails() async throws {
        let display = try await TestDisplay.get()
        let stray = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 120, height: 80),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        stray.animationBehavior = .none
        stray.isReleasedWhenClosed = false
        stray.title = "Left behind"
        let scene = ShotScene(
            id: "a scene that leaves a window up",
            size: CGSize(width: 300, height: 200),
            frame: .window(title: "Shot")
        ) {
            stray.orderFront(nil)
            return ShotStage(Color.clear)
        }
        let error = await #expect(throws: ShotError.self) {
            try await render(scene, on: display)
        }
        guard case .windowsLeftUp(_, let windows, nil) = error else {
            Issue.record(
                "expected a window left up: \(String(describing: error))"
            )
            return
        }
        #expect(windows.count == 1, "\(windows)")
        #expect(windows.first?.contains("Left behind") == true, "\(windows)")
        #expect(!stray.isVisible)
    }

    /// A display lease that fails to close fails the capture, and the
    /// failures of its scenes stay in the list.
    @Test
    func aFailedLeaseCloseKeepsTheScenesFailures() async throws {
        let display = try await ShotDisplay.add(size: ShotDisplay.sharedSize)
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("lease-close-\(UUID())")
        try FileManager.default.createDirectory(
            at: folder,
            withIntermediateDirectories: true
        )
        defer {
            do { try FileManager.default.removeItem(at: folder) }
            catch { Issue.record("\(folder.path) stays: \(error)") }
        }
        // A lease tool that lends this display and fails as it is released.
        let tool = folder.appendingPathComponent("lease.py")
        try Data(
            """
            import os, sys
            print("\(display.id) 0", flush=True)
            os.close(sys.stdout.fileno())
            sys.stdin.buffer.read()
            sys.exit(3)
            """
            .utf8
        )
        .write(to: tool)
        let sizes = folder.appendingPathComponent("board-sizes.json")
        try Data("{}".utf8).write(to: sizes)
        let tooWide = ShotScene(
            id: "a window wider than its slot",
            size: CGSize(width: ShotDisplay.size.width + 100, height: 300),
            frame: .window(title: "Shot")
        ) {
            ShotStage(Color.clear)
        }
        let error = await #expect(throws: ShotRunner.Failed.self) {
            try await ShotRunner.capture(
                [tooWide],
                in: PlainLook(),
                environment: [
                    "BUILDS_SHOTS_OUT": folder.path,
                    "BUILDS_SHOTS_BOARD_SIZES": sizes.path,
                    "BUILDS_SHOTS_DISPLAY": tool.path,
                ]
            )
        }
        let failures = try #require(error).failures
        #expect(failures.count == 2, "\(failures)")
        #expect(failures.first?.hasPrefix(tooWide.id) == true, "\(failures)")
        #expect(
            failures.last?.contains("exited with status 3") == true,
            "\(failures)"
        )
    }

    private func visibleWindows() -> Set<Int> {
        Set(
            NSApplication.shared.windows.filter(\.isVisible)
                .map(\.windowNumber)
        )
    }

    private func expectOversizedWindowToFail(on display: ShotDisplay) async {
        let scene = ShotScene(
            id: "a window wider than the shot display",
            size: CGSize(width: ShotDisplay.size.width + 100, height: 300),
            frame: .window(title: "Shot")
        ) {
            ShotStage(Color.clear)
        }
        let error = await #expect(throws: ShotError.self) {
            try await render(scene, on: display)
        }
        guard case .windowTooLarge = error else {
            Issue.record(
                "expected the window to be too large: \(String(describing: error))"
            )
            return
        }
    }

    /// `scene` captured in the plain look on `display`. No scene here is a
    /// component, so the boards' sizes list none.
    private func render(
        _ scene: ShotScene,
        on display: ShotDisplay
    ) async throws -> ShotRenderer.Capture {
        let sizes = FileManager.default.temporaryDirectory
            .appendingPathComponent("no-board-sizes-\(UUID()).json")
        try Data("{}".utf8).write(to: sizes)
        defer {
            do { try FileManager.default.removeItem(at: sizes) }
            catch { Issue.record("\(sizes.path) stays: \(error)") }
        }
        return try await ShotRenderer.render(
            scene,
            in: PlainLook(),
            boardSizes: ShotBoardSizes(contentsOf: sizes),
            on: display
        )
    }
}

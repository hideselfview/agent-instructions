import AppKit
import Testing

@testable import ShotHarness

/// What the renderer says holds the focus when a scene's window never
/// becomes key.
struct FocusHolderTests {
    /// The app macOS names as the front app is the shot test itself, another
    /// app with its name and pid, or no app.
    @Test
    @MainActor
    func theFrontAppIsThisAppAnotherAppOrNone() throws {
        let dock = try #require(
            NSRunningApplication.runningApplications(
                withBundleIdentifier: "com.apple.dock"
            )
            .first
        )
        #expect(FocusHolder(NSRunningApplication.current) == .thisApp)
        #expect(
            FocusHolder(dock)
                == .other(name: "Dock", pid: dock.processIdentifier)
        )
        #expect(FocusHolder(nil) == .noApp)
    }

    /// The failure names the app that kept the focus and its pid, so whoever
    /// reads it knows what to close.
    @Test
    func theFailureNamesTheAppThatKeptTheFocus() {
        let scene = "mac.welcome.empty"
        #expect(
            ShotError.windowNotKey(
                scene,
                focus: .other(name: "UserNotificationCenter", pid: 812)
            )
            .description
                == "UserNotificationCenter (pid 812) kept the focus, so "
                + "scene mac.welcome.empty would draw inactive"
        )
        #expect(
            ShotError.windowNotKey(
                scene,
                focus: .other(name: nil, pid: 812)
            )
            .description
                == "an app with no name (pid 812) kept the focus, so "
                + "scene mac.welcome.empty would draw inactive"
        )
        #expect(
            ShotError.windowNotKey(scene, focus: .thisApp).description
                == "the shot test is the front app, but scene "
                + "mac.welcome.empty's window never became key, so it would "
                + "draw inactive"
        )
        #expect(
            ShotError.windowNotKey(scene, focus: .noApp).description
                == "macOS names no front app, and scene mac.welcome.empty's "
                + "window never became key, so it would draw inactive"
        )
    }
}

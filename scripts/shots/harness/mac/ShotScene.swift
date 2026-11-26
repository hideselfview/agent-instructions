import AppKit
import SwiftUI

/// One screen of the app to shoot: the name of the design board it is
/// compared with (`<app>.<area>.<screen>[.<state>]`), the point size to
/// render at, and a builder for the composed production view and what it
/// waits for.
///
/// `builds shots` reads which scenes an app declares from its source, so a
/// scene's id is written as a string literal of its board name, and a scene
/// shown in its window or taking the keyboard focus says so among the
/// arguments of its `ShotScene(` call: `frame: .window(title: …)`,
/// `takesFocus: true`. A window frame or focus marker the checker cannot tie
/// to such an id fails its reading.
@MainActor
struct ShotScene {
    let id: String
    /// The view's size, in points; a scene shown in its window adds the
    /// window's title bar.
    let size: CGSize
    var frame: ShotFrame = .content
    /// Whether the scene's view alone is drawn in a key window, so a field
    /// the scene focuses draws focused. A scene's view alone is in a window
    /// of an app that is not active, which is never key; this one is in a
    /// panel that does not activate the app.
    var keyWindow = false
    /// Whether a scene shown in its window is drawn inactive, as macOS draws
    /// a window behind another app's window: the window is neither key nor
    /// main.
    var inactive = false
    /// Whether drawing the scene takes the keyboard focus from the person,
    /// so only a run with `BUILDS_SHOTS_WINDOWS` set takes it.
    var takesFocus = false
    let stage: () throws -> ShotStage
}

/// A scene's view, built with its fixture, and whether what the view shows
/// has arrived: the reads it makes off the main thread answered and taken
/// up, an image decoded. The scene knows which reads it needs, so it says
/// when they are done; a scene whose views read nothing off the main thread
/// is ready at once.
@MainActor
struct ShotStage {
    let view: AnyView
    /// Whether the hosted tree shows what the scene is about, read after
    /// each layout, for what a view builds from a read it makes itself and
    /// no store publishes, such as a text field holding a loaded value.
    let isDrawn: @MainActor (NSView) -> Bool
    /// Read under observation tracking, so it holds only what a store
    /// publishes, or what a fake announces with `Wait.happened()`.
    let isReady: @MainActor () -> Bool

    init(
        _ view: some View,
        isReady: @escaping @MainActor () -> Bool = { true },
        isDrawn: @escaping @MainActor (NSView) -> Bool = { _ in true }
    ) {
        self.view = AnyView(view)
        self.isReady = isReady
        self.isDrawn = isDrawn
    }
}

/// What a shot takes in: the scene's view alone, or the window it is shown
/// in, title bar and toolbar included, for a scene whose toolbar is part of
/// what it shows. A window is drawn through the window server on the shot
/// display.
enum ShotFrame {
    case content
    /// A design-system component, drawn at its board's size, the preview
    /// row's cell (`ShotBoardSizes`), whatever the scene's `size` says.
    case component
    case window(title: String, titleBar: TitleBar = .shown)

    enum TitleBar {
        /// The native title bar and its system background.
        case shown
        /// The native title bar over a background the app's look draws.
        case themed
        /// The content reaches under the window buttons.
        case hidden
    }
}

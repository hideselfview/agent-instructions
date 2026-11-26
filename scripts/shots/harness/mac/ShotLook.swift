import AppKit
import SwiftUI

/// How the app draws its views in a shot: its appearance, its preferences,
/// its title bar and the windows it puts over a scene's window.
@MainActor
protocol ShotLook {
    /// A dressing for one render of a scene: `view`, the scene's own, which
    /// the shot frames as `frame`.
    func dress(_ view: AnyView, framedAs frame: ShotFrame) throws
        -> any ShotDressing
}

/// The look of one render of one scene, which holds what the render needs
/// until `undress()`.
@MainActor
protocol ShotDressing {
    /// The view the renderer hosts: the scene's view with what the app's
    /// windows give their content. A view the app hosts in a window of its
    /// own, which inherits nothing from this one, takes `shotStill()` too.
    var rootView: AnyView { get }

    /// Gives the shot's window, before the renderer shows it, what the app
    /// gives its windows, its appearance among it.
    func dress(_ window: NSWindow)

    /// The windows the app puts over the scene's window now, each over the
    /// ones before it: alerts and other views it draws as panels of their
    /// own. A sheet attached to the window is the system's, which the
    /// renderer takes in by itself.
    var overlays: [ShotOverlay] { get }

    /// Removes what the dressing made, such as a preferences suite, once the
    /// render has ended, however it ended.
    func undress()
}

/// A panel the app puts over a scene's window. The shot's pixels take it in,
/// and its top one holds the keyboard.
struct ShotOverlay {
    let panel: NSPanel
    let reading: Reading

    /// Where the element list reads the panel's elements.
    enum Reading {
        /// After everything the list holds so far.
        case last
        /// Before the element at the index the closure gives, or after
        /// everything when it gives none. The closure takes the list so far
        /// and the index of the root of the window's content.
        case before(@MainActor (AccessibilityTree, Int?) -> Int?)
        /// Nowhere, such as a window under a layer that covers it, which the
        /// person cannot reach.
        case unlisted
    }
}

extension View {
    /// This view with animations off: a view that opens or moves after the
    /// window shows, such as a grid's detail opening on its place's first
    /// answer, lands at once, so the capture never holds a frame partway
    /// through.
    func shotStill() -> some View {
        transaction {
            $0.animation = nil
            $0.disablesAnimations = true
        }
    }
}

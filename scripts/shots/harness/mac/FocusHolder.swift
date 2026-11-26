import AppKit

/// The app that holds the focus, as macOS names its front app: the renderer
/// names it when a scene's window never becomes key, so whoever reads the
/// failure knows what to close, such as a system alert that took the focus.
enum FocusHolder: Equatable, Sendable {
    /// The shot test itself.
    case thisApp
    /// Another app, with its name when macOS gives one.
    case other(name: String?, pid: pid_t)
    /// macOS names no front app.
    case noApp

    init(_ app: NSRunningApplication?) {
        guard let app else {
            self = .noApp
            return
        }
        self =
            app == NSRunningApplication.current
            ? .thisApp
            : .other(name: app.localizedName, pid: app.processIdentifier)
    }

    /// The front app now.
    @MainActor
    static func now() -> FocusHolder {
        FocusHolder(NSWorkspace.shared.frontmostApplication)
    }
}

import CoreGraphics

/// Why a shot could not be taken.
enum ShotError: Error, CustomStringConvertible {
    /// A variable the checker sets for a capture is unset.
    case unset(String)
    case bitmapAllocationFailed(String)
    case pngEncodingFailed(String)
    case windowNotShared(String)
    /// The window server shows no window for the scene: its frame there, and
    /// what the window server and the displays held then.
    case windowNotDrawn(String, CGRect, ShotWindowState)
    /// The scene's window never became key; `focus` is the app macOS named
    /// as the front app when the wait ended.
    case windowNotKey(String, focus: FocusHolder)
    case noScreen(String)
    case noContentView(String)
    case windowTooLarge(
        String,
        content: CGSize,
        window: CGSize,
        shown: CGSize,
        room: CGSize
    )
    case unknownSelection(String)
    case noBoardSize(String)

    var description: String {
        switch self {
        case .unset(let variable):
            "\(variable) is unset: take the shots through builds shots, which "
                + "sets it and its TEST_RUNNER_ copy"
        case .bitmapAllocationFailed(let id):
            "failed to allocate the capture bitmap for scene \(id)"
        case .pngEncodingFailed(let id):
            "failed to PNG-encode scene \(id)"
        case .windowNotShared(let id):
            "the window server shares no window for scene \(id)"
        case .windowNotDrawn(let id, let frame, let state):
            "the window server shows scene \(id)'s window with frame "
                + "\(frame), off the screen or empty. \(state.description)"
        case .windowNotKey(let id, .other(.some(let name), let pid)):
            "\(name) (pid \(pid)) kept the focus, so scene \(id) would draw "
                + "inactive"
        case .windowNotKey(let id, .other(nil, let pid)):
            "an app with no name (pid \(pid)) kept the focus, so scene \(id) "
                + "would draw inactive"
        case .windowNotKey(let id, .thisApp):
            "the shot test is the front app, but scene \(id)'s window never "
                + "became key, so it would draw inactive"
        case .windowNotKey(let id, .noApp):
            "macOS names no front app, and scene \(id)'s window never became "
                + "key, so it would draw inactive"
        case .noScreen(let id):
            "ScreenCaptureKit shares no shot display for scene \(id)'s window"
        case .noContentView(let id):
            "scene \(id)'s window has no content view"
        case .windowTooLarge(
            let id,
            let content,
            let window,
            let shown,
            let room
        ):
            "scene \(id) asks for \(content) points of content, and its "
                + "window of \(window) points shows \(shown) of it, on a shot "
                + "display with a free area of \(room) points"
        case .noBoardSize(let id):
            "the boards' sizes list none for component scene \(id)"
        case .unknownSelection(let prefix):
            "no scene the run takes starts with \(prefix)"
        }
    }
}

import Testing

@testable import ShotHarness

/// What a failure for a window the window server does not show says when
/// the window server gives no list of windows.
struct ShotWindowStateTests {
    @Test
    @MainActor
    func aMissingWindowListReadsAsUnread() {
        let windows = ShotWindowState.windowList([41], listed: nil)
        #expect(windows == .unread)
        let state = ShotWindowState(
            windows: windows,
            displays: [],
            shotDisplay: 7,
            holder: "owned by this process"
        )
        #expect(
            state.description
                == "the window server gave no list of windows; "
                + "shot display held as: owned by this process"
        )
    }
}

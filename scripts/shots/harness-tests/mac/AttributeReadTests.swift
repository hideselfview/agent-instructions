import ApplicationServices
import Testing

@testable import ShotHarness

/// What the reader makes of an attribute the accessibility API fails to give.
struct AttributeReadTests {
    /// An element of a process that does not exist answers no read, and the
    /// reader says which attribute failed rather than reading it as absent.
    @Test
    func aFailedReadIsAnErrorAndNoAbsentAttribute() {
        let gone = AXUIElementCreateApplication(pid_t(Int32.max))
        let error = #expect(
            throws: AccessibilityTree.ReadError.AttributeFailure.self
        ) {
            try AccessibilityTree.Reader.string(gone, kAXTitleAttribute)
        }
        #expect(error?.attribute == kAXTitleAttribute)
    }
}

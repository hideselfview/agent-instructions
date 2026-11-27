import Foundation
import Testing

@testable import ShotHarness

/// What wakes a wait to read its condition again.
struct WaitTests {
    private static let changed = Notification.Name("WaitTests.changed")

    /// A notice posted while the condition is read is kept for the wait that
    /// follows the reading, and each later one wakes the wait again.
    @Test
    @MainActor
    func eachNoticeReadsTheConditionAgain() async throws {
        let center = NotificationCenter.default
        let subject = NSObject()
        var readings = 0
        try await Wait.until(
            notices: [Wait.Notice(Self.changed, of: subject)]
        ) {
            readings += 1
            if readings < 3 {
                center.post(name: Self.changed, object: subject)
            }
            return readings == 3
        }
        #expect(readings == 3)
    }

    /// A change of what the condition read wakes the wait.
    @Test
    @MainActor
    func aChangeOfWhatTheConditionReadReadsItAgain() async throws {
        let record = Record()
        var readings = 0
        let wait = Task { @MainActor in
            try await Wait.until {
                readings += 1
                return record.isDone
            }
        }
        // The wait's first reading is queued ahead of this change.
        await Task { @MainActor in record.isDone = true }.value
        try await wait.value
        #expect(readings == 2)
    }
}

@MainActor
@Observable
private final class Record {
    var isDone = false
}

import Testing

@testable import ShotHarness

/// Which serial the shot display takes, with a stand-in for the CoreGraphics
/// call that makes the display.
struct ShotDisplaySerialTests {
    /// A stand-in display call that refuses each serial in `held`, as
    /// CoreGraphics refuses a display whose identity a held display has.
    private static func make(held: Set<UInt32>) -> (UInt32) -> UInt32? {
        { serial in held.contains(serial) ? nil : serial }
    }

    @Test
    func theFirstSerialIsOneWhenNothingIsHeld() throws {
        let taken = try ShotDisplaySerial.claim(
            Self.make(held: []),
            heldDisplays: { "" }
        )
        #expect(taken == 1)
    }

    @Test
    func theSerialsAreTheSameOnEveryRun() throws {
        let first = try ShotDisplaySerial.claim(
            Self.make(held: [1]),
            heldDisplays: { "" }
        )
        let second = try ShotDisplaySerial.claim(
            Self.make(held: [1]),
            heldDisplays: { "" }
        )
        #expect(first == 2)
        #expect(second == first)
    }

    @Test
    func aHeldSerialIsSkippedForTheNextOne() throws {
        let taken = try ShotDisplaySerial.claim(
            Self.make(held: [1, 2, 3, 5]),
            heldDisplays: { "" }
        )
        #expect(taken == 4)
    }

    @Test
    func theEighthSerialIsTheLastOneTried() throws {
        let taken = try ShotDisplaySerial.claim(
            Self.make(held: Set(1...7)),
            heldDisplays: { "" }
        )
        #expect(taken == 8)
    }

    @Test
    func eightHeldSerialsFailNamingTheHeldDisplays() {
        do {
            _ = try ShotDisplaySerial.claim(
                Self.make(held: Set(1...8)),
                heldDisplays: { "display 7 (serial 3)" }
            )
            Issue.record("expected no serial to be free")
        }
        catch let error as ShotDisplayError {
            #expect(
                error.reason
                    == "CoreGraphics refused serials 1 to 8, held by: "
                    + "display 7 (serial 3)"
            )
        }
        catch {
            Issue.record("expected ShotDisplayError: \(error)")
        }
    }
}

import CoreGraphics

/// The serial numbers the shot display takes. macOS writes a ColorSync
/// profile for each display identity (vendor, product and serial) it has not
/// seen, and keeps it, so the shot display's identity comes from a fixed set:
/// a Mac holds at most one profile for each serial, however many runs it
/// makes.
///
/// CoreGraphics makes no display from a descriptor whose identity matches a
/// display it still holds, such as one a crashed test process left behind or
/// one another run of the shots holds. So a display takes the first serial
/// that CoreGraphics accepts.
enum ShotDisplaySerial {
    static let serials: ClosedRange<UInt32> = 1...8

    /// The result of `make` for the first serial in `serials` that it makes
    /// something for, where nil means CoreGraphics refused that serial. When
    /// it refuses every serial, throws `ShotDisplayError` with what
    /// `heldDisplays` says.
    static func claim<Display>(
        _ make: (UInt32) throws -> Display?,
        heldDisplays: () -> String
    ) throws -> Display {
        for serial in serials {
            if let display = try make(serial) {
                return display
            }
        }
        throw ShotDisplayError(
            "CoreGraphics refused serials \(serials.lowerBound) to "
                + "\(serials.upperBound), held by: \(heldDisplays())"
        )
    }

    /// The displays the Mac lists, each with its ID, vendor and serial.
    static func describeHeldDisplays() -> String {
        var count: UInt32 = 0
        CGGetOnlineDisplayList(0, nil, &count)
        var ids = [CGDirectDisplayID](repeating: 0, count: Int(count))
        CGGetOnlineDisplayList(count, &ids, &count)
        return ids.prefix(Int(count))
            .map { id in
                "display \(id) (vendor \(String(CGDisplayVendorNumber(id), radix: 16)), "
                    + "product \(String(CGDisplayModelNumber(id), radix: 16)), "
                    + "serial \(CGDisplaySerialNumber(id)))"
            }
            .joined(separator: ", ")
    }
}

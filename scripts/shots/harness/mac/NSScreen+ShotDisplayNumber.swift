import AppKit

extension NSScreen {
    /// The number CoreGraphics gives the screen's display.
    var shotDisplayNumber: CGDirectDisplayID? {
        deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")]
            as? CGDirectDisplayID
    }
}

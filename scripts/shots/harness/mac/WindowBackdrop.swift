import AppKit

/// What a window shows under a captured view. The app draws every window
/// over its window background, and a view captured on its own is transparent
/// where it draws nothing, so text in a label colour over that transparency
/// reads by appearance: the light text of the dark appearance vanishes on a
/// light viewer. Laid over the window's own colour, a capture shows what a
/// person sees.
///
/// Composed in a Core Graphics context addressed in pixels: a fill through
/// a bitmap's AppKit graphics context painted nothing on the hosted runner
/// and left the capture transparent, while the same fill painted on a
/// development machine.
enum WindowBackdrop {
    enum Failure: Error, CustomStringConvertible {
        case unresolvedBackground
        case unreadableCapture
        case contextUnavailable

        var description: String {
            switch self {
            case .unresolvedBackground:
                "the window background resolved to no opaque colour"
            case .unreadableCapture:
                "the capture has no image or colour space to compose"
            case .contextUnavailable:
                "no bitmap context to compose the capture in"
            }
        }
    }

    /// `capture`, of `host` at `size` points, laid over the window background
    /// in `host`'s appearance.
    @MainActor
    static func composite(
        _ capture: NSBitmapImageRep,
        of host: NSView,
        size: NSSize
    ) throws -> NSBitmapImageRep {
        guard let image = capture.cgImage,
            let space = capture.colorSpace.cgColorSpace
        else { throw Failure.unreadableCapture }
        let pixels = CGRect(
            x: 0,
            y: 0,
            width: capture.pixelsWide,
            height: capture.pixelsHigh
        )
        guard
            let context = CGContext(
                data: nil,
                width: capture.pixelsWide,
                height: capture.pixelsHigh,
                bitsPerComponent: 8,
                bytesPerRow: 0,
                space: space,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )
        else { throw Failure.contextUnavailable }
        let backdrop = try windowBackground(for: host, in: capture.colorSpace)
        context.setFillColor(backdrop.cgColor)
        context.fill(pixels)
        context.draw(image, in: pixels)
        guard let composed = context.makeImage() else {
            throw Failure.contextUnavailable
        }
        let rep = NSBitmapImageRep(cgImage: composed)
        rep.size = size
        return rep
    }

    /// The window background colour under `host`'s appearance, as concrete
    /// components in `space`: a dynamic colour resolves against the
    /// appearance current when it is drawn, and a bitmap context has none of
    /// its own.
    @MainActor
    private static func windowBackground(
        for host: NSView,
        in space: NSColorSpace
    ) throws -> NSColor {
        var resolved: NSColor?
        host.effectiveAppearance.performAsCurrentDrawingAppearance {
            resolved = NSColor.windowBackgroundColor.usingColorSpace(space)
        }
        guard let resolved, resolved.alphaComponent == 1 else {
            throw Failure.unresolvedBackground
        }
        return resolved
    }
}

import CoreGraphics

/// Pixels the capture hands back, put on the grid every host's shot shares.
enum ShotImage {
    /// `image` at exactly `width` by `height` pixels. The window server
    /// captures a window at its display's scale, whatever size the capture
    /// asks for: a host whose display is not Retina (an external monitor, say) hands
    /// back a 1x image, which would fill a corner of the 2x file with the
    /// rest empty. Drawing it up to the asked size puts every host's shot
    /// on the same pixel grid.
    nonisolated static func scaled(
        _ image: CGImage,
        width: Int,
        height: Int,
        sceneId: String
    ) throws -> CGImage {
        if image.width == width, image.height == height { return image }
        guard
            let space = CGColorSpace(name: CGColorSpace.sRGB),
            let context = CGContext(
                data: nil,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: 0,
                space: space,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )
        else { throw ShotError.bitmapAllocationFailed(sceneId) }
        context.interpolationQuality = .high
        context.draw(
            image,
            in: CGRect(x: 0, y: 0, width: width, height: height)
        )
        guard let scaled = context.makeImage() else {
            throw ShotError.bitmapAllocationFailed(sceneId)
        }
        return scaled
    }
}

import CoreGraphics
import ScreenCaptureKit

/// A system sheet's shadow, laid on a window shot. The window server draws it
/// on the screen, and ScreenCaptureKit leaves it out of a display capture of
/// chosen windows. A capture that keeps it, such as one of the sheet's window
/// alone with its shadow, also draws the window server's rim, a dark line
/// just outside the sheet and a light one inside it, which no board draws. So
/// the shot keeps the pixels of the display capture and takes from the
/// sheet's own capture only the shadow's opacity outside the sheet.
enum ShotSheetShadow {
    /// How opaque each pixel of a picture is, 0 to 255, row by row from the
    /// top left.
    struct Opacity: Sendable {
        let width: Int
        let height: Int
        let values: [UInt8]

        /// The opacity at `x`, `y`: nothing outside the picture.
        subscript(x: Int, y: Int) -> UInt8 {
            guard x >= 0, y >= 0, x < width, y < height else { return 0 }
            return values[y * width + x]
        }

        /// The smallest box that holds every pixel that is wholly opaque, or
        /// nil when none is.
        var opaqueBox: (x: Int, y: Int, width: Int, height: Int)? {
            var minX = width
            var minY = height
            var maxX = -1
            var maxY = -1
            for y in 0..<height {
                for x in 0..<width where self[x, y] == 255 {
                    minX = min(minX, x)
                    minY = min(minY, y)
                    maxX = max(maxX, x)
                    maxY = max(maxY, y)
                }
            }
            guard maxX >= 0 else { return nil }
            return (minX, minY, maxX - minX + 1, maxY - minY + 1)
        }
    }

    /// A window's opacity, placed with its top left at `x`, `y` pixels of the
    /// shot.
    struct Placed: Sendable {
        let opacity: Opacity
        let x: Int
        let y: Int

        subscript(shotX: Int, shotY: Int) -> UInt8 {
            opacity[shotX - x, shotY - y]
        }
    }

    /// A sheet's window as the window server draws it alone.
    struct Sheet: Sendable {
        /// The sheet without its shadow, placed where the shot shows it: what
        /// the sheet itself covers.
        let covers: Placed
        /// The sheet with its shadow and rim, placed so that the sheet in it
        /// lies on `covers`.
        let shadowed: Placed

        /// `bare` placed at `x`, `y` of the shot, with `shadowed`, the same
        /// window drawn with its shadow, in which the window lies somewhere
        /// inside the shadow's margins. Fails when the wholly opaque parts of
        /// the two differ in size, since the one cannot then be laid on the
        /// other.
        init(
            bare: Opacity,
            shadowed: Opacity,
            x: Int,
            y: Int,
            sceneId: String
        ) throws {
            guard let inner = bare.opaqueBox, let outer = shadowed.opaqueBox,
                inner.width == outer.width, inner.height == outer.height
            else {
                throw ShotError.sheetShadowUnplaced(sceneId)
            }
            covers = Placed(opacity: bare, x: x, y: y)
            self.shadowed = Placed(
                opacity: shadowed,
                x: x + inner.x - outer.x,
                y: y + inner.y - outer.y
            )
        }

        /// How much the shadow darkens what lies under the sheet at the
        /// shot's pixel `x`, `y`, 0 to 1.
        func shade(x: Int, y: Int) -> Double {
            let covered = covers[x, y]
            if covered == 255 { return 0 }
            let uncovered = 1 - Double(covered) / 255
            guard touches(x: x, y: y) else {
                return Double(shadowed[x, y]) / 255 * uncovered
            }
            // The pixels that touch the sheet hold the rim in the sheet's own
            // capture. The shadow there is as dark as the darkest of the
            // nearest pixels the rim does not reach.
            var darkest: UInt8 = 0
            for dy in -Self.reach...Self.reach {
                for dx in -Self.reach...Self.reach
                where !touches(x: x + dx, y: y + dy) {
                    darkest = max(darkest, shadowed[x + dx, y + dy])
                }
            }
            return Double(darkest) / 255 * uncovered
        }

        /// How far from a pixel that touches the sheet to look for one that
        /// does not, in pixels.
        private static let reach = 2

        /// Whether the sheet covers any of the pixel at `x`, `y` or of a pixel
        /// beside it, which is where the rim outside the sheet is drawn.
        private func touches(x: Int, y: Int) -> Bool {
            for dy in -1...1 {
                for dx in -1...1 where covers[x + dx, y + dy] > 0 {
                    return true
                }
            }
            return false
        }
    }

    /// `pixels`, RGBA bytes of a shot `width` pixels wide, with `sheet`'s
    /// shadow laid on every pixel the sheet does not cover, less where a
    /// window `above` the sheet covers it.
    static func lay(
        _ sheet: Sheet,
        under above: [Placed],
        on pixels: inout [UInt8],
        width: Int
    ) {
        let height = pixels.count / (width * 4)
        let left = max(0, sheet.shadowed.x)
        let right = min(
            width,
            sheet.shadowed.x + sheet.shadowed.opacity.width
        )
        let top = max(0, sheet.shadowed.y)
        let bottom = min(
            height,
            sheet.shadowed.y + sheet.shadowed.opacity.height
        )
        guard left < right, top < bottom else { return }
        for y in top..<bottom {
            for x in left..<right {
                var shade = sheet.shade(x: x, y: y)
                for window in above {
                    shade *= 1 - Double(window[x, y]) / 255
                }
                guard shade > 0 else { continue }
                // The shadow is black, so it takes that share of each colour.
                for channel in 0..<3 {
                    let index = (y * width + x) * 4 + channel
                    pixels[index] = UInt8(
                        (Double(pixels[index]) * (1 - shade)).rounded()
                    )
                }
            }
        }
    }
}

extension ShotSheetShadow {
    /// `image`, a shot of the window `parent`, with the shadow of each of
    /// `sheets` laid around it, under the windows `above` the sheets.
    nonisolated static func cast(
        by sheets: [SCWindow],
        under above: [SCWindow],
        on image: CGImage,
        of parent: SCWindow,
        sceneId: String
    ) async throws -> CGImage {
        if sheets.isEmpty { return image }
        // Where `window` lies in the shot, in the shot's pixels.
        func origin(of window: SCWindow) -> (x: Int, y: Int) {
            let scale = CGFloat(ShotDisplay.scale)
            return (
                Int(
                    ((window.frame.minX - parent.frame.minX) * scale).rounded()
                ),
                Int(((window.frame.minY - parent.frame.minY) * scale).rounded())
            )
        }
        var covering: [Placed] = []
        for window in above {
            let at = origin(of: window)
            covering.append(
                Placed(
                    opacity: try await alone(
                        window,
                        withShadow: false,
                        sceneId: sceneId
                    ),
                    x: at.x,
                    y: at.y
                )
            )
        }
        var pixels = try bytes(of: image, sceneId: sceneId)
        for window in sheets {
            let at = origin(of: window)
            let sheet = try Sheet(
                bare: try await alone(
                    window,
                    withShadow: false,
                    sceneId: sceneId
                ),
                shadowed: try await alone(
                    window,
                    withShadow: true,
                    sceneId: sceneId
                ),
                x: at.x,
                y: at.y,
                sceneId: sceneId
            )
            lay(sheet, under: covering, on: &pixels, width: image.width)
        }
        return try picture(of: pixels, like: image, sceneId: sceneId)
    }

    /// Room around a window for its shadow, in points: more than the window
    /// server's shadows reach.
    private static let margin = 200

    /// The opacity of `window` captured alone at the shot's scale, with or
    /// without its shadow. With its shadow the picture has room around the
    /// window, where the window server draws it from the top left.
    private nonisolated static func alone(
        _ window: SCWindow,
        withShadow: Bool,
        sceneId: String
    ) async throws -> Opacity {
        let configuration = SCStreamConfiguration()
        let room = withShadow ? 2 * margin : 0
        configuration.width =
            (Int(window.frame.width) + room) * ShotDisplay.scale
        configuration.height =
            (Int(window.frame.height) + room) * ShotDisplay.scale
        configuration.scalesToFit = false
        configuration.ignoreShadowsSingleWindow = !withShadow
        configuration.includeChildWindows = false
        configuration.backgroundColor = .clear
        configuration.showsCursor = false
        let image = try ShotImage.scaled(
            try await SCScreenshotManager.captureImage(
                contentFilter: SCContentFilter(
                    desktopIndependentWindow: window
                ),
                configuration: configuration
            ),
            width: configuration.width,
            height: configuration.height,
            sceneId: sceneId
        )
        let pixels = try bytes(of: image, sceneId: sceneId)
        return Opacity(
            width: image.width,
            height: image.height,
            values: stride(from: 3, to: pixels.count, by: 4).map { pixels[$0] }
        )
    }

    /// `image`'s pixels as RGBA bytes with premultiplied alpha, in its own
    /// colour space.
    private nonisolated static func bytes(
        of image: CGImage,
        sceneId: String
    ) throws -> [UInt8] {
        var pixels = [UInt8](
            repeating: 0,
            count: image.width * image.height * 4
        )
        try pixels.withUnsafeMutableBytes { buffer in
            let context = try context(
                buffer.baseAddress,
                like: image,
                sceneId: sceneId
            )
            context.draw(
                image,
                in: CGRect(x: 0, y: 0, width: image.width, height: image.height)
            )
        }
        return pixels
    }

    /// The picture that `pixels`, laid out as `bytes(of: image)`, hold.
    private nonisolated static func picture(
        of pixels: [UInt8],
        like image: CGImage,
        sceneId: String
    ) throws -> CGImage {
        var pixels = pixels
        return try pixels.withUnsafeMutableBytes { buffer in
            let context = try context(
                buffer.baseAddress,
                like: image,
                sceneId: sceneId
            )
            guard let made = context.makeImage() else {
                throw ShotError.bitmapAllocationFailed(sceneId)
            }
            return made
        }
    }

    private nonisolated static func context(
        _ pixels: UnsafeMutableRawPointer?,
        like image: CGImage,
        sceneId: String
    ) throws -> CGContext {
        guard let space = image.colorSpace,
            let context = CGContext(
                data: pixels,
                width: image.width,
                height: image.height,
                bitsPerComponent: 8,
                bytesPerRow: image.width * 4,
                space: space,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )
        else { throw ShotError.bitmapAllocationFailed(sceneId) }
        return context
    }
}

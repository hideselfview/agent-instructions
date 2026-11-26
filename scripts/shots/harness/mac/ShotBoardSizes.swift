import CoreGraphics
import Foundation

/// The size of each component scene's board, in points: the preview row's
/// cell, which `builds shots` measures and writes as `board-sizes.json`
/// before the capture starts, naming the file in `BUILDS_SHOTS_BOARD_SIZES`.
struct ShotBoardSizes {
    private struct Entry: Decodable {
        let width: Double
        let height: Double
    }

    private let sizes: [String: CGSize]

    /// The sizes the file at `url` lists.
    init(contentsOf url: URL) throws {
        sizes = try JSONDecoder()
            .decode([String: Entry].self, from: Data(contentsOf: url))
            .mapValues { CGSize(width: $0.width, height: $0.height) }
    }

    /// The board size of component scene `id`; a scene with no entry fails.
    func size(of id: String) throws -> CGSize {
        guard let size = sizes[id] else {
            throw ShotError.noBoardSize(id)
        }
        return size
    }
}

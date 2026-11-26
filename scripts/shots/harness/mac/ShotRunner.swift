import Foundation

/// Takes an app's shots: each selected scene drawn once in the app's look and
/// written as `<board>@macos.png`, with the elements its view lists beside it
/// as `<board>@macos.json`. An app's shots test calls `capture` with its
/// scenes and its look.
@MainActor
enum ShotRunner {
    /// The scenes that failed to render, encode or write, with why.
    struct Failed: Error, CustomStringConvertible {
        let failures: [String]

        var description: String {
            "scene capture failed:\n" + failures.joined(separator: "\n")
        }
    }

    /// Captures the scenes of `scenes` that `environment` asks for, in
    /// `look`. Every selected scene is attempted, and any failure fails the
    /// capture with the full list, so a broken scene is never a silent skip.
    /// Scenes shown in their windows lease a slot on the host's shot display.
    static func capture(
        _ scenes: [ShotScene],
        in look: some ShotLook,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) async throws {
        let request = try ShotRequest(environment)
        try FileManager.default.createDirectory(
            at: request.out,
            withIntermediateDirectories: true
        )
        let selected = try request.selected(of: scenes)
        let boardSizes = try ShotBoardSizes(contentsOf: request.boardSizes)
        var lease: ShotDisplayLease?
        if selected.contains(where: { $0.frame.isWindow }) {
            guard let tool = request.displayTool else {
                throw ShotError.unset("BUILDS_SHOTS_DISPLAY")
            }
            lease = try await ShotDisplayLease.acquire(tool: tool)
        }

        var failures: [String] = []
        for scene in selected {
            do {
                let capture = try await ShotRenderer.render(
                    scene,
                    in: look,
                    boardSizes: boardSizes,
                    on: lease?.display
                )
                guard !capture.png.isEmpty else {
                    failures.append("\(scene.id): rendered no bytes")
                    continue
                }
                let url = request.out
                    .appendingPathComponent("\(scene.id)@macos.png")
                try capture.png.write(to: url)
                try capture.elements.json()
                    .write(
                        to: url.deletingPathExtension()
                            .appendingPathExtension("json")
                    )
            }
            catch {
                failures.append("\(scene.id): \(error)")
            }
        }

        try lease?.close()
        guard failures.isEmpty else { throw Failed(failures: failures) }
    }
}

extension ShotFrame {
    fileprivate var isWindow: Bool {
        if case .window = self {
            true
        }
        else {
            false
        }
    }
}

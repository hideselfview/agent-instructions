import Foundation

/// What the checker asks a capture for, in the `BUILDS_SHOTS_*` variables it
/// runs the app's capture command with (`builds help`, APP BUILDS). Xcode
/// passes the test process each `TEST_RUNNER_` copy under the plain name.
struct ShotRequest {
    /// The folder each shot is written to.
    let out: URL
    /// The board-name prefixes of the scenes to take; nil for every scene.
    let prefixes: [String]?
    /// Whether the run also takes the scenes that take the keyboard focus.
    let takesFocus: Bool
    /// The file of the component scenes' board sizes.
    let boardSizes: URL
    /// The tool that leases a slot of the shot display, which a run with
    /// scenes shown in their windows names.
    let displayTool: String?

    /// The request `environment` holds.
    init(_ environment: [String: String]) throws {
        func value(_ name: String) -> String? {
            environment[name].flatMap { $0.isEmpty ? nil : $0 }
        }
        guard let out = value("BUILDS_SHOTS_OUT") else {
            throw ShotError.unset("BUILDS_SHOTS_OUT")
        }
        guard let boardSizes = value("BUILDS_SHOTS_BOARD_SIZES") else {
            throw ShotError.unset("BUILDS_SHOTS_BOARD_SIZES")
        }
        self.out = URL(fileURLWithPath: out, isDirectory: true)
        self.boardSizes = URL(fileURLWithPath: boardSizes)
        prefixes = value("BUILDS_SHOTS_SELECT")
            .map { $0.split(separator: ",").map(String.init) }
        takesFocus = value("BUILDS_SHOTS_WINDOWS") == "1"
        displayTool = value("BUILDS_SHOTS_DISPLAY")
    }

    /// The scenes of `scenes` the run takes: those a prefix starts, or all
    /// of them, without the ones that take the focus unless the run takes
    /// those. A prefix that starts none fails.
    @MainActor
    func selected(of scenes: [ShotScene]) throws -> [ShotScene] {
        let taken = scenes.filter { takesFocus || !$0.takesFocus }
        guard let prefixes else { return taken }
        for prefix in prefixes
        where !taken.contains(where: { $0.id.hasPrefix(prefix) }) {
            throw ShotError.unknownSelection(prefix)
        }
        return taken.filter { scene in
            prefixes.contains { scene.id.hasPrefix($0) }
        }
    }
}

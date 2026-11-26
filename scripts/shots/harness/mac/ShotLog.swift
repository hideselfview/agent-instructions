import OSLog

/// The harness's log, which writes to the unified log under the subsystem
/// `builds.shots`, with its messages public:
/// `/usr/bin/log show --info --debug --predicate 'subsystem == "builds.shots"'`.
struct ShotLog: Sendable {
    private let logger: Logger

    init(_ category: String) {
        logger = Logger(subsystem: "builds.shots", category: category)
    }

    func debug(_ message: String) {
        logger.debug("\(message, privacy: .public)")
    }

    func info(_ message: String) {
        logger.info("\(message, privacy: .public)")
    }

    func error(_ message: String) {
        logger.error("\(message, privacy: .public)")
    }
}

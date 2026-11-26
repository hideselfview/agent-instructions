import AppKit
import Foundation

/// The writing end of the lease process's input. Closing it releases our slot.
@MainActor
struct ShotDisplayLease {
    private static let logger = ShotLog("ShotDisplayLease")

    let display: ShotDisplay
    private let process: Process
    private let release: FileHandle

    /// A slot of the shared shot display, leased by the lease tool at
    /// `tool`, which the checker names in `BUILDS_SHOTS_DISPLAY`.
    static func acquire(tool: String) async throws -> ShotDisplayLease {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["python3", tool]
        let ready = Pipe()
        let lifetime = Pipe()
        process.standardOutput = ready
        process.standardInput = lifetime
        try process.run()
        let release = lifetime.fileHandleForWriting
        do {
            let reply =
                try await Task.detached {
                    try ready.fileHandleForReading.readToEnd()
                }
                .value
            let words = reply.flatMap { String(data: $0, encoding: .utf8) }?
                .split(whereSeparator: \.isWhitespace)
            guard let words, words.count == 2,
                let id = CGDirectDisplayID(words[0]), id != 0,
                let slot = Int(words[1]), (0..<2).contains(slot)
            else {
                throw ShotDisplayError(
                    "The display lease did not supply a display ID and slot."
                )
            }
            let display = try await ShotDisplay.borrow(
                id: id,
                slot: slot,
                leaseProcess: process.processIdentifier
            )
            logger.info("The capture leased display \(id), slot \(slot).")
            return ShotDisplayLease(
                display: display,
                process: process,
                release: release
            )
        }
        catch {
            try release.close()
            throw error
        }
    }

    func close() throws {
        try release.close()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            throw ShotDisplayError(
                "The display lease process exited with status \(process.terminationStatus)."
            )
        }
        Self.logger.info("The capture released display \(display.id).")
    }
}

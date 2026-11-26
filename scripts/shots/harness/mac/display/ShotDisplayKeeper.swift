import AppKit
import Darwin

/// Owns the virtual display until the last capture drops its shared lock.
@main
struct ShotDisplayKeeper {
    @MainActor
    static func main() {
        let app = NSApplication.shared
        guard app.setActivationPolicy(.accessory) else {
            report(
                "AppKit refused to make the shot display keeper a background process.",
                to: .pipe(FileHandle.standardOutput)
            )
            exit(EXIT_FAILURE)
        }
        Task { @MainActor in
            await holdDisplay()
            exit(EXIT_SUCCESS)
        }
        app.run()
    }

    @MainActor
    private static func holdDisplay() async {
        let args = CommandLine.arguments
        guard args.count == 4, let startup = Int32(args[2]),
            let owner = Int32(args[3])
        else {
            FileHandle.standardOutput.write(
                Data(
                    "The shot display keeper needs a directory and two lock descriptors.\n"
                        .utf8
                )
            )
            exit(EXIT_FAILURE)
        }
        let root = URL(fileURLWithPath: args[1], isDirectory: true)
        let output = FileHandle.standardOutput
        let result = root.appendingPathComponent("result")
        var announced = false
        do {
            let active = try FileHandle(
                forReadingFrom: root.appendingPathComponent("active")
            )
            let id: CGDirectDisplayID
            do {
                let display = try await ShotDisplay.add(
                    size: ShotDisplay.sharedSize
                )
                id = display.id
                let identity = Data("\(id)\n".utf8)
                try identity.write(
                    to: root.appendingPathComponent("display-id"),
                    options: .atomic
                )
                // Publish the identity before letting another capture join.
                close(startup)
                try output.write(contentsOf: identity)
                announced = true
                try output.close()
                try await waitForCaptures(active)
                withExtendedLifetime(display) {}
            }
            // Keep the exclusive activity lock until AppKit sees the display
            // go away, so the next owner does not change the layout mid-shot.
            try await ShotDisplay.waitUntilRemoved(id)
            try Data().write(to: result, options: .atomic)
            close(owner)
            try active.close()
        }
        catch {
            report(
                String(describing: error),
                to: announced ? .file(result) : .pipe(output)
            )
            exit(EXIT_FAILURE)
        }
    }

    private enum FailureOutput {
        case file(URL)
        case pipe(FileHandle)
    }

    private static func report(_ message: String, to output: FailureOutput) {
        let logger = ShotLog("ShotDisplayKeeper")
        logger.error("\(message)")
        let failure = Data("\(message)\n".utf8)
        do {
            switch output {
            case .file(let result):
                try failure.write(to: result, options: .atomic)
            case .pipe(let pipe):
                try pipe.write(contentsOf: failure)
            }
        }
        catch {
            logger.error("The keeper could not report its failure. \(error)")
        }
    }

    private static func waitForCaptures(_ active: FileHandle) async throws {
        let descriptor = active.fileDescriptor
        try await withCheckedThrowingContinuation {
            (waiting: CheckedContinuation<Void, any Error>) in
            DispatchQueue.global()
                .async {
                    while flock(descriptor, LOCK_EX) != 0 {
                        if errno == EINTR { continue }
                        waiting.resume(
                            throwing: NSError(
                                domain: NSPOSIXErrorDomain,
                                code: Int(errno)
                            )
                        )
                        return
                    }
                    waiting.resume()
                }
        }
    }
}

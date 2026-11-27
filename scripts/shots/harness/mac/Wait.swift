import AppKit
import Foundation

/// Waiting in a test for something that happens later: a value landing in a
/// store, a recorder receiving a call, a hosted view finishing its layout.
///
/// A wait ends on the event it waits for, never on a clock. A condition is
/// read under observation tracking, so the wait resumes when anything
/// observable the condition read changes: a store's value, or what a test's
/// recorder or fake records. What AppKit changes without observation is
/// named as notices. A hosted view tree is read each time the main run loop
/// goes idle, about to sleep. The only timer is the hang guard
/// ([`HangGuard`]), which fails the wait loudly when the event never comes.
enum Wait {
    /// What a wait was given did not happen in time.
    struct TimedOut: LocalizedError, CustomStringConvertible {
        let file: StaticString
        let line: UInt

        var description: String {
            "the wait at \(file):\(line) was still waiting after \(HangGuard.duration)"
        }

        var errorDescription: String? { description }
    }

    /// Return once `condition` holds. It is read under observation tracking,
    /// so it holds only what is observable or what `notices` tell of.
    /// Observation starts tracking what a condition read only once the whole
    /// condition has run, so what changes off the main thread tells of each
    /// change on the main thread.
    ///
    /// What AppKit changes without observation, such as a clip view's bounds,
    /// is named in `notices`: each posting of one reads the condition again.
    @MainActor
    static func until(
        notices: [Notice] = [],
        file: StaticString = #filePath,
        line: UInt = #line,
        _ condition: @MainActor () throws -> Bool
    ) async throws {
        let wake = Wake()
        let listeners = notices.map { $0.listen { wake.fire() } }
        defer { for listener in listeners { listener.stop() } }
        let end = ContinuousClock.now.advanced(by: HangGuard.duration)
        while true {
            var holds = false
            var failure: (any Error)?
            withObservationTracking {
                do { holds = try condition() }
                catch { failure = error }
            } onChange: {
                wake.fire()
            }
            if let failure { throw failure }
            if holds { return }
            try await wake.fired(
                before: end,
                file: file,
                line: line
            )
        }
    }

    /// Something AppKit changes without observation, which a wait reads its
    /// condition again on: a notification, or a key-value-observed property.
    struct Notice {
        let listen: (@escaping @Sendable () -> Void) -> Listener

        init(_ name: Notification.Name, of object: AnyObject?) {
            listen = { fire in
                let observer = NotificationCenter.default.addObserver(
                    forName: name,
                    object: object,
                    queue: nil
                ) { _ in fire() }
                return Listener {
                    NotificationCenter.default.removeObserver(observer)
                }
            }
        }
    }

    final class Listener: @unchecked Sendable {
        private let stopping: () -> Void

        init(stopping: @escaping () -> Void) {
            self.stopping = stopping
        }

        func stop() { stopping() }
    }

    /// One wait's wake-up, fired by the observation of what its condition
    /// read and by its notices. A fire while nothing waits is kept for the
    /// next wait for it, which takes it.
    private final class Wake: @unchecked Sendable {
        private let lock = NSLock()
        private var isFired = false
        private var waiter: CheckedContinuation<Void, Never>?

        func fire() {
            let resume: CheckedContinuation<Void, Never>? = lock.withLock {
                guard let waiting = waiter else {
                    isFired = true
                    return nil
                }
                waiter = nil
                return waiting
            }
            resume?.resume()
        }

        private func wait() async {
            await withCheckedContinuation { continuation in
                let resumeNow = lock.withLock {
                    if isFired {
                        isFired = false
                        return true
                    }
                    waiter = continuation
                    return false
                }
                if resumeNow { continuation.resume() }
            }
        }

        /// Return when fired; throw when `end` comes first.
        func fired(
            before end: ContinuousClock.Instant,
            file: StaticString,
            line: UInt
        ) async throws {
            try await withThrowingTaskGroup(of: Void.self) { group in
                group.addTask {
                    await withTaskCancellationHandler {
                        await self.wait()
                    } onCancel: {
                        self.fire()
                    }
                }
                group.addTask {
                    try await Task.sleep(until: end, clock: ContinuousClock())
                    throw TimedOut(file: file, line: line)
                }
                defer { group.cancelAll() }
                try await group.next()
            }
        }
    }
}

// Waits for a hosted view tree, read at each idle of the main run loop.
extension Wait {
    /// Return once `condition` holds of a hosted view tree, laid out before
    /// each reading: at once, or at a later idle of the main run loop.
    /// SwiftUI publishes a tree's changes over several main-actor turns,
    /// each ending when the run loop has nothing left to run, so the tree is
    /// read again at each idle. Whatever a store or a recorder holds is
    /// waited for with [`until`].
    @MainActor
    static func untilLaidOut(
        _ host: NSView,
        file: StaticString = #filePath,
        line: UInt = #line,
        _ condition: @MainActor () throws -> Bool
    ) async throws {
        let end = ContinuousClock.now.advanced(by: HangGuard.duration)
        while true {
            host.layoutSubtreeIfNeeded()
            if try condition() { return }
            try await nextIdle(
                before: end,
                file: file,
                line: line
            )
        }
    }

    /// Return at the main run loop's next idle: once it has run SwiftUI's
    /// and Core Animation's updates and is about to sleep.
    ///
    /// An idle is not the end of all main-actor work: one can come while
    /// main-actor work is still queued, and the run loop wakes again at once
    /// to run it: a view can ask for its list some idles after its window
    /// opens, its tree unchanged across them. Work running
    /// off the main thread is not waited for either: what it brings back
    /// wakes the run loop, and its own event is what a wait for it reads.
    @MainActor
    static func nextIdle(
        before end: ContinuousClock.Instant,
        file: StaticString,
        line: UInt
    ) async throws {
        let wake = Wake()
        // Ordered last, so every other observer of the same pass has run.
        let observer = CFRunLoopObserverCreateWithHandler(
            nil,
            CFRunLoopActivity.beforeWaiting.rawValue,
            false,
            CFIndex.max
        ) { _, _ in wake.fire() }
        CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
        defer { CFRunLoopObserverInvalidate(observer) }
        try await wake.fired(
            before: end,
            file: file,
            line: line
        )
    }

    /// Return once `value` reads the same at two idles of the main run loop
    /// in a row: a hosted tree whose frames stopped moving.
    ///
    /// SwiftUI lays a hosted tree out over several main-actor turns, each
    /// ending at an idle of the run loop: a geometry reader publishes a
    /// width, the views under it measure again, work a view started on the
    /// main actor lands. A tree read the same across an idle has none of
    /// that left, unless what is left is a chain of main-actor steps that
    /// moves no frame, since an idle can come with main-actor work still
    /// queued. Such a chain, and work off the main thread such as a page a
    /// query answers, is waited for by its own event first. So is an
    /// animation: the run loop idles between its frames with the tree where
    /// the last frame left it, so a tree mid-animation reads the same at two
    /// idles in a row. A wait for it reads the value it carries landing on
    /// its end.
    @MainActor
    static func untilSteady<Value: Equatable>(
        file: StaticString = #filePath,
        line: UInt = #line,
        _ value: @MainActor () async throws -> Value
    ) async throws {
        let end = ContinuousClock.now.advanced(by: HangGuard.duration)
        var last = try await value()
        while true {
            try await nextIdle(
                before: end,
                file: file,
                line: line
            )
            let now = try await value()
            if now == last { return }
            last = now
        }
    }
}

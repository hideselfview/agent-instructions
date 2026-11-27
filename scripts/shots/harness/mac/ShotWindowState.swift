import AppKit

/// What the window server and the displays held when a scene's window was
/// not shown: the failure names it, so whoever reads the failure sees why
/// the window was missing.
struct ShotWindowState: Sendable, Equatable {
    /// The window server's entry for one window, which it lists whether or
    /// not the window is on the screen.
    struct Entry: Sendable, Equatable {
        let id: CGWindowID
        let frame: CGRect
        let layer: Int
        let onScreen: Bool
    }

    /// One display AppKit lists.
    struct Display: Sendable, Equatable {
        let id: CGDirectDisplayID
        let frame: CGRect
        let scale: CGFloat
    }

    /// The window server's entry for one of the scene's windows, or nil when
    /// it lists none.
    struct Listing: Sendable, Equatable {
        let id: CGWindowID
        let entry: Entry?
    }

    /// A window of this process the scene did not ask for.
    struct Other: Sendable, Equatable {
        let entry: Entry
        let className: String
    }

    /// What the window server's list of windows says.
    enum WindowList: Sendable, Equatable {
        /// One listing for each of the scene's windows, in order, and every
        /// other window of this process that the window server shows on the
        /// screen, with the class AppKit gives it: a window left over from
        /// an earlier scene can cover one of the scene's.
        case read([Listing], others: [Other])
        /// The window server gave no list.
        case unread
    }

    let windows: WindowList
    let displays: [Display]
    /// The display the scene is drawn on.
    let shotDisplay: CGDirectDisplayID
    /// Who holds that display (`ShotDisplay.holder`).
    let holder: String

    /// The state now, for the windows numbered `windowIDs`.
    @MainActor
    static func read(
        windowIDs: [CGWindowID],
        on display: ShotDisplay
    ) -> Self {
        return Self(
            windows: windowList(
                windowIDs,
                listed: CGWindowListCopyWindowInfo(
                    [.optionAll],
                    kCGNullWindowID
                ) as? [[String: Any]]
            ),
            displays: NSScreen.screens.compactMap { screen in
                screen.shotDisplayNumber.map {
                    Display(
                        id: $0,
                        frame: screen.frame,
                        scale: screen.backingScaleFactor
                    )
                }
            },
            shotDisplay: display.id,
            holder: display.holder
        )
    }

    /// What `listed`, the window server's list, or nil when it gave none,
    /// says of the windows numbered `windowIDs`.
    @MainActor
    static func windowList(
        _ windowIDs: [CGWindowID],
        listed: [[String: Any]]?
    ) -> WindowList {
        guard let listed else { return .unread }
        let entries = listed.compactMap(entry(from:))
        let pid = Int(getpid())
        let ours = Set(
            listed.compactMap { info -> CGWindowID? in
                guard info[kCGWindowOwnerPID as String] as? Int == pid else {
                    return nil
                }
                return (info[kCGWindowNumber as String] as? Int)
                    .map(CGWindowID.init)
            }
        )
        let classes = Dictionary(
            NSApplication.shared.windows.map {
                (CGWindowID($0.windowNumber), $0.className)
            },
            uniquingKeysWith: { first, _ in first }
        )
        return .read(
            windowIDs.map { id in
                Listing(id: id, entry: entries.first { $0.id == id })
            },
            others:
                entries.filter {
                    ours.contains($0.id) && $0.onScreen
                        && !windowIDs.contains($0.id)
                }
                .map {
                    Other(entry: $0, className: classes[$0.id] ?? "unknown")
                }
        )
    }

    private static func entry(from info: [String: Any]) -> Entry? {
        guard
            let number = info[kCGWindowNumber as String] as? Int,
            let bounds = info[kCGWindowBounds as String] as? [String: Any],
            let frame = CGRect(dictionaryRepresentation: bounds as CFDictionary)
        else { return nil }
        return Entry(
            id: CGWindowID(number),
            frame: frame,
            layer: info[kCGWindowLayer as String] as? Int ?? 0,
            // The window server leaves the key out for a window that is not
            // on the screen.
            onScreen: info[kCGWindowIsOnscreen as String] as? Bool ?? false
        )
    }

    /// The state as the failure prints it.
    var description: String {
        let windowLines: [String] =
            switch windows {
            case .unread:
                ["the window server gave no list of windows"]
            case .read(let listings, let others):
                listings.map(\.description) + others.map(\.description)
            }
        let displayLines = displays.map {
            "display \($0.id)\($0.id == shotDisplay ? " (the shot display)" : ""): "
                + "frame \($0.frame), \($0.scale)x"
        }
        return
            (windowLines + displayLines
            + ["shot display held as: \(holder)"])
            .joined(separator: "; ")
    }
}

extension ShotWindowState.Listing: CustomStringConvertible {
    var description: String {
        entry.map {
            "window \(id): window server frame \($0.frame), "
                + "layer \($0.layer), "
                + "\($0.onScreen ? "on" : "not on") the screen"
        } ?? "window \(id): the window server lists no entry"
    }
}

extension ShotWindowState.Other: CustomStringConvertible {
    var description: String {
        "other window \(entry.id) (\(className)) of this process: "
            + "window server frame \(entry.frame), layer \(entry.layer), "
            + "on the screen"
    }
}

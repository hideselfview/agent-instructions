import AppKit
import ObjectiveC

/// A virtual display beside the Mac's own displays. A capture draws its
/// windows in a 1600 by 1200 point area at two pixels per point. The capture
/// borrows one area from the keeper. Renderer tests own a display. A shot taken from a real display
/// would depend on that display: a small display leaves less free area than the
/// library window's scenes need, and a display at 1 pixel a
/// point would back the window at half the shot's scale. On this display
/// every Mac draws a window scene the same way, and the window server still
/// composes the window with the alert panels the app raises over it.
///
/// CoreGraphics adds a virtual display only through its private classes
/// `CGVirtualDisplayDescriptor`, `CGVirtualDisplayMode`,
/// `CGVirtualDisplaySettings` and `CGVirtualDisplay`, which no SDK header
/// declares, so this type reaches them through the Objective-C runtime. It is
/// the only code that does. When macOS lacks them, or makes no display from
/// them, every window scene fails with `ShotDisplayError`.
@MainActor
final class ShotDisplay {
    /// The display's size, in points.
    static let size = CGSize(width: 1_600, height: 1_200)
    /// The display draws at the scale every shot is captured at.
    nonisolated static let scale = 2
    static let sharedSize = CGSize(width: size.width * 2, height: size.height)

    let id: CGDirectDisplayID

    private enum Backing {
        case owned(NSObject)
        case borrowed(slot: Int, leaseProcess: pid_t)
    }

    private let backing: Backing

    private init(id: CGDirectDisplayID, backing: Backing) {
        self.id = id
        self.backing = backing
    }

    func visibleFrame() throws -> CGRect {
        let screen = try screen()
        switch backing {
        case .owned:
            return screen.visibleFrame
        case .borrowed(let slot, _):
            let frame = screen.frame
            let slotFrame = CGRect(
                x: frame.minX + CGFloat(slot) * Self.size.width,
                y: frame.minY,
                width: Self.size.width,
                height: Self.size.height
            )
            return screen.visibleFrame.intersection(slotFrame)
        }
    }

    /// The screen AppKit lists for the display, with its free area.
    func screen() throws -> NSScreen {
        guard let screen = Self.screen(for: id) else {
            throw ShotDisplayError("AppKit no longer lists it")
        }
        return screen
    }

    static func screen(for id: CGDirectDisplayID) -> NSScreen? {
        NSScreen.screens.first { $0.shotDisplayNumber == id }
    }

    private static let screenChanges = Wait.Notice(
        NSApplication.didChangeScreenParametersNotification,
        of: nil
    )

    /// Add the display, once AppKit lists it at its size and scale.
    static func add(size: CGSize) async throws -> ShotDisplay {
        let width = Int(size.width)
        let height = Int(size.height)
        let display = try virtualDisplay(
            pixelsWide: width * scale,
            pixelsHigh: height * scale,
            mode: (width, height)
        )
        guard
            let id = display.value(forKey: "displayID") as? CGDirectDisplayID,
            id != 0
        else {
            throw ShotDisplayError("CoreGraphics gave it no display ID")
        }
        // CoreGraphics lists the display's modes once the display is
        // connected, which AppKit announces by listing its screen.
        try await listed(id, as: "a screen") { _ in true }
        // The display offers a mode at 2 pixels a point alongside 1x modes.
        let options =
            [kCGDisplayShowDuplicateLowResolutionModes: true] as CFDictionary
        guard
            let modes = CGDisplayCopyAllDisplayModes(id, options)
                as? [CGDisplayMode]
        else {
            throw ShotDisplayError("CoreGraphics lists no modes for it")
        }
        guard
            let mode = modes.first(where: {
                $0.width == width && $0.height == height
                    && $0.pixelWidth == width * scale
                    && $0.pixelHeight == height * scale
            })
        else {
            throw ShotDisplayError(
                "it offers no mode of \(width) by \(height) points at "
                    + "\(scale) pixels a point"
            )
        }
        try select(mode, for: id)
        try await listed(id, at: size)
        return ShotDisplay(id: id, backing: .owned(display))
    }

    private static func listed(
        _ id: CGDirectDisplayID,
        at size: CGSize
    ) async throws {
        try await listed(
            id,
            as:
                "\(Int(size.width)) by \(Int(size.height)) points at \(scale) pixels a point"
        ) {
            $0.backingScaleFactor == CGFloat(scale) && $0.frame.size == size
        }
    }

    /// Wait for AppKit to list display `id` with a screen that `holds`, which
    /// `described` names in the failure.
    private static func listed(
        _ id: CGDirectDisplayID,
        as described: String,
        where holds: @MainActor (NSScreen) -> Bool
    ) async throws {
        do {
            try await Wait.until(
                notices: [screenChanges]
            ) {
                screen(for: id).map { holds($0) } == true
            }
        }
        catch is Wait.TimedOut {
            throw ShotDisplayError(
                "AppKit never listed it as \(described)"
            )
        }
    }

    /// A `CGVirtualDisplay` of at most `pixelsWide` by `pixelsHigh`, offering
    /// `mode` in points.
    private static func virtualDisplay(
        pixelsWide: Int,
        pixelsHigh: Int,
        mode: (width: Int, height: Int)
    ) throws -> NSObject {
        let displayClass = try privateClass("CGVirtualDisplay")
        typealias InitWithDescriptor =
            @convention(c) (AnyObject, Selector, AnyObject) -> Unmanaged<
                NSObject
            >?
        let initialize = try call(
            "initWithDescriptor:",
            of: displayClass,
            as: InitWithDescriptor.self
        )
        // CoreGraphics makes no display from a descriptor whose identity a
        // display it holds has.
        let display = try ShotDisplaySerial.claim(
            { serial in
                initialize(
                    try allocate(displayClass),
                    NSSelectorFromString("initWithDescriptor:"),
                    try descriptor(
                        pixelsWide: pixelsWide,
                        pixelsHigh: pixelsHigh,
                        serial: serial
                    )
                )?
                .takeRetainedValue()
            },
            heldDisplays: ShotDisplaySerial.describeHeldDisplays
        )
        typealias ApplySettings =
            @convention(c) (AnyObject, Selector, AnyObject) -> Bool
        let applied = try call(
            "applySettings:",
            of: displayClass,
            as: ApplySettings.self
        )(
            display,
            NSSelectorFromString("applySettings:"),
            try settings(offering: mode)
        )
        guard applied else {
            throw ShotDisplayError("CoreGraphics refused its settings")
        }
        return display
    }

    /// A `CGVirtualDisplayDescriptor` of at most `pixelsWide` by
    /// `pixelsHigh`.
    private static func descriptor(
        pixelsWide: Int,
        pixelsHigh: Int,
        serial: UInt32
    ) throws -> NSObject {
        let descriptor = try privateClass("CGVirtualDisplayDescriptor").init()
        descriptor.setValue(DispatchQueue.main, forKey: "queue")
        descriptor.setValue("builds shots", forKey: "name")
        descriptor.setValue(pixelsWide, forKey: "maxPixelsWide")
        descriptor.setValue(pixelsHigh, forKey: "maxPixelsHigh")
        // The physical size the descriptor asks for.
        descriptor.setValue(
            NSValue(size: CGSize(width: 600, height: 450)),
            forKey: "sizeInMillimeters"
        )
        descriptor.setValue(0xB11D, forKey: "vendorID")
        descriptor.setValue(0xB11D, forKey: "productID")
        // The serial is one of `ShotDisplaySerial.serials`, so macOS keeps at
        // most that many display profiles for the shot display.
        descriptor.setValue(serial, forKey: "serialNum")
        return descriptor
    }

    /// `CGVirtualDisplaySettings` that offer `mode` in points, at 1 and at 2
    /// pixels a point.
    private static func settings(
        offering mode: (width: Int, height: Int)
    ) throws -> NSObject {
        let modeClass = try privateClass("CGVirtualDisplayMode")
        typealias InitWithSize =
            @convention(c) (AnyObject, Selector, UInt, UInt, Double) ->
            Unmanaged<NSObject>
        let pointMode = try call(
            "initWithWidth:height:refreshRate:",
            of: modeClass,
            as: InitWithSize.self
        )(
            try allocate(modeClass),
            NSSelectorFromString("initWithWidth:height:refreshRate:"),
            UInt(mode.width),
            UInt(mode.height),
            60
        )
        .takeRetainedValue()

        let settings = try privateClass("CGVirtualDisplaySettings").init()
        settings.setValue([pointMode], forKey: "modes")
        // Offer each mode at 2 pixels a point too.
        settings.setValue(1, forKey: "hiDPI")
        return settings
    }

    private static func privateClass(_ name: String) throws -> NSObject.Type {
        guard let found = NSClassFromString(name) as? NSObject.Type else {
            throw ShotDisplayError("macOS has no class \(name)")
        }
        return found
    }

    private static func allocate(_ type: NSObject.Type) throws -> AnyObject {
        guard
            let allocated = type.perform(NSSelectorFromString("alloc"))?
                .takeUnretainedValue()
        else {
            throw ShotDisplayError("\(type) allocated nothing")
        }
        return allocated
    }

    /// The instance method `selector` of `type`, as a function of `signature`.
    private static func call<Function>(
        _ selector: String,
        of type: NSObject.Type,
        as signature: Function.Type
    ) throws -> Function {
        let selector = NSSelectorFromString(selector)
        guard
            type.instancesRespond(to: selector),
            let implementation = class_getMethodImplementation(type, selector)
        else {
            throw ShotDisplayError(
                "\(type) has no method \(NSStringFromSelector(selector))"
            )
        }
        return unsafeBitCast(implementation, to: signature)
    }
}

extension ShotDisplay {
    /// The leased slot of a display held by the capture process's keeper.
    /// `leaseProcess` is the process that holds the lease for the capture.
    static func borrow(
        id: CGDirectDisplayID,
        slot: Int,
        leaseProcess: pid_t
    ) async throws -> ShotDisplay {
        try await listed(id, at: sharedSize)
        return ShotDisplay(
            id: id,
            backing: .borrowed(slot: slot, leaseProcess: leaseProcess)
        )
    }

    /// Who holds the display, for a failure that names it: the slot and the
    /// process that holds its lease, or that this process owns the display.
    var holder: String {
        switch backing {
        case .owned:
            "owned by this process (pid \(getpid()))"
        case .borrowed(let slot, let leaseProcess):
            "slot \(slot) of the shared display, leased by pid \(leaseProcess) "
                + "for the capture in pid \(getpid())"
        }
    }

    static func waitUntilRemoved(_ id: CGDirectDisplayID) async throws {
        try await Wait.until(notices: [screenChanges]) {
            screen(for: id) == nil
        }
    }

    /// WindowServer restores the session's display configuration when
    /// another virtual display closes. Save the shot display's 2x mode in
    /// that configuration so it keeps the same size and scale.
    fileprivate static func select(
        _ mode: CGDisplayMode,
        for id: CGDirectDisplayID
    ) throws {
        var configuration: CGDisplayConfigRef?
        let begin = CGBeginDisplayConfiguration(&configuration)
        guard begin == .success, let configuration else {
            throw ShotDisplayError(
                "CoreGraphics could not start the mode change for display \(id) "
                    + "(error \(begin.rawValue))"
            )
        }
        let change = CGConfigureDisplayWithDisplayMode(
            configuration,
            id,
            mode,
            nil
        )
        guard change == .success else {
            CGCancelDisplayConfiguration(configuration)
            throw ShotDisplayError(
                "CoreGraphics refused the mode for display \(id) "
                    + "(error \(change.rawValue))"
            )
        }
        let complete = CGCompleteDisplayConfiguration(
            configuration,
            .forSession
        )
        guard complete == .success else {
            throw ShotDisplayError(
                "CoreGraphics could not save the session mode for display \(id) "
                    + "(error \(complete.rawValue))"
            )
        }
    }
}

struct ShotDisplayError: Error, CustomStringConvertible {
    let reason: String

    init(_ reason: String) {
        self.reason = reason
    }

    var description: String { "The renderer needs a shot display. \(reason)" }
}

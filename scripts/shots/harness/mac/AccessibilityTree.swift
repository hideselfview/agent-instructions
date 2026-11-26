import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

private let treeLogger = ShotLog("AccessibilityTree")

/// The accessibility tree of one of this process's own windows, read the way
/// an assistive client reads it: through the accessibility API.
///
/// SwiftUI builds its elements (texts, buttons, toggles, pop-ups) only for a
/// client of the accessibility API; the hosting view's own
/// `accessibilityChildren()` lists none of them. A process may query its own
/// windows without the accessibility permission. The API answers such a
/// request on the calling thread, so the read runs on the main actor, where
/// SwiftUI evaluates view bodies (a body that is main-actor isolated traps
/// when evaluated anywhere else).
struct AccessibilityTree: Sendable {
    /// The words' look as the tree gives it: the font's name (whose last part
    /// is the weight), its size in points and the colour as sRGB, 0 to 1.
    struct TextStyle: Sendable {
        let fontName: String
        let size: Double
        let red: Double
        let green: Double
        let blue: Double
        let alpha: Double
    }

    enum Value: Sendable {
        case text(String)
        case number(Double)
    }

    /// One element, with its box in points from the window's top-left.
    struct Node: Sendable {
        let role: String
        let subrole: String?
        let title: String?
        let description: String?
        /// The words of the element this one names itself by
        /// (`AXTitleUIElement`), such as the label beside a pop-up.
        /// VoiceOver reads them as the name of a control that has no title or
        /// description of its own.
        var titleElementWords: String?
        let placeholder: String?
        let value: Value?
        let x: Double
        let y: Double
        let width: Double
        let height: Double
        let enabled: Bool
        let selected: Bool?
        let focused: Bool?
        let style: TextStyle?
        /// The index in `nodes` of the element that holds this one.
        let parent: Int?
        /// For a static text, the box of its words alone, as the tree gives
        /// the bounds of the text's whole range. The element's own box is
        /// its container's when it is the container's only child.
        var wordsBox: CGRect?
    }

    /// The window's elements, each before the ones it holds, in the order an
    /// assistive client reads them. The window itself is not one.
    let nodes: [Node]

    /// The tree of `window`.
    @MainActor
    static func read(of window: NSWindow, subject: String) throws -> Self {
        let listed = try listedWindow(window, subject: subject)
        var reader = Reader(origin: listed.origin, subject: subject)
        try reader.add(
            children: Reader.children(of: listed.element, subject: subject),
            parent: nil
        )
        return Self(nodes: reader.nodes)
    }

    struct Reader {
        let origin: CGPoint
        /// What the tree is read for, which an error names.
        let subject: String
        var nodes: [Node] = []

        mutating func add(children: [AXUIElement], parent: Int?) throws {
            for child in children {
                // An element the tree gives an infinite position has no box
                // to list, and JSON has no infinity to write one as.
                // The elements it holds are listed under its parent.
                if let frame = Self.frame(of: child), !frame.isFinite {
                    try add(
                        children: Self.children(of: child, subject: subject),
                        parent: parent
                    )
                    continue
                }
                let index = nodes.count
                guard let node = try node(of: child, parent: parent) else {
                    continue
                }
                nodes.append(node)
                try add(
                    children: Self.children(of: child, subject: subject),
                    parent: index
                )
            }
        }

        private func node(of element: AXUIElement, parent: Int?) throws -> Node?
        {
            guard let frame = Self.frame(of: element),
                let role = Self.string(element, kAXRoleAttribute)
            else { return nil }
            let value = Self.value(element)
            return Node(
                role: role,
                subrole: Self.string(element, kAXSubroleAttribute),
                title: Self.string(element, kAXTitleAttribute),
                description: Self.string(element, kAXDescriptionAttribute),
                titleElementWords: Self.titleElementWords(of: element),
                placeholder: Self.string(element, kAXPlaceholderValueAttribute),
                value: value,
                x: frame.minX - origin.x,
                y: frame.minY - origin.y,
                width: frame.width,
                height: frame.height,
                enabled: Self.bool(element, kAXEnabledAttribute) ?? true,
                selected: Self.bool(element, kAXSelectedAttribute),
                focused: Self.bool(element, kAXFocusedAttribute),
                style: try Self.style(
                    of: element,
                    value: value,
                    role: role,
                    subject: subject
                ),
                parent: parent,
                wordsBox:
                    try Self.wordsBox(
                        of: element,
                        value: value,
                        role: role,
                        subject: subject
                    )
                    .map { $0.offsetBy(dx: -origin.x, dy: -origin.y) }
            )
        }

        static func frame(of element: AXUIElement) -> CGRect? {
            var point = CGPoint.zero
            var size = CGSize.zero
            guard
                let position = attribute(element, kAXPositionAttribute),
                let extent = attribute(element, kAXSizeAttribute),
                CFGetTypeID(position) == AXValueGetTypeID(),
                CFGetTypeID(extent) == AXValueGetTypeID(),
                AXValueGetValue(
                    unsafeDowncast(position, to: AXValue.self),
                    .cgPoint,
                    &point
                ),
                AXValueGetValue(
                    unsafeDowncast(extent, to: AXValue.self),
                    .cgSize,
                    &size
                )
            else { return nil }
            return CGRect(origin: point, size: size)
        }

        private static func attribute(
            _ element: AXUIElement,
            _ name: String
        ) -> CFTypeRef? {
            var result: CFTypeRef?
            guard
                AXUIElementCopyAttributeValue(
                    element,
                    name as CFString,
                    &result
                )
                    == .success
            else { return nil }
            return result
        }

        static func string(
            _ element: AXUIElement,
            _ name: String
        ) -> String? {
            guard let text = attribute(element, name) as? String,
                !text.isEmpty
            else { return nil }
            return text
        }

        private static func bool(
            _ element: AXUIElement,
            _ name: String
        ) -> Bool? {
            attribute(element, name) as? Bool
        }

        private static func value(_ element: AXUIElement) -> Value? {
            switch attribute(element, kAXValueAttribute) {
            case let text as String: .text(text)
            case let number as NSNumber: .number(number.doubleValue)
            default: nil
            }
        }

        /// The look of a text element's words over their whole range.
        private static func style(
            of element: AXUIElement,
            value: Value?,
            role: String,
            subject: String
        ) throws -> TextStyle? {
            guard
                let attributed = try wholeRange(
                    of: element,
                    attribute:
                        kAXAttributedStringForRangeParameterizedAttribute,
                    value: value,
                    role: role,
                    subject: subject
                ),
                let text = attributed as? NSAttributedString,
                text.length > 0
            else { return nil }
            let attributes = text.attributes(at: 0, effectiveRange: nil)
            guard
                let font = attributes[NSAttributedString.Key("AXFont")]
                    as? [String: Any],
                let name = font["AXFontName"] as? String,
                let size = (font["AXFontSize"] as? NSNumber)?.doubleValue,
                let color = attributes[
                    NSAttributedString.Key("AXForegroundColor")
                ],
                CFGetTypeID(color as CFTypeRef) == CGColor.typeID,
                let srgb = CGColorSpace(name: CGColorSpace.sRGB),
                let rgb = unsafeDowncast(color as AnyObject, to: CGColor.self)
                    .converted(to: srgb, intent: .defaultIntent, options: nil),
                let c = rgb.components, c.count == 4
            else { return nil }
            return TextStyle(
                fontName: name,
                size: size,
                red: c[0],
                green: c[1],
                blue: c[2],
                alpha: c[3]
            )
        }
    }
}

extension AccessibilityTree.Reader {
    /// The words of the element `element` names itself by, which are the
    /// label's value, without the colon that ends a label: VoiceOver does
    /// not read it.
    fileprivate static func titleElementWords(of element: AXUIElement)
        -> String?
    {
        guard
            let named = attribute(element, kAXTitleUIElementAttribute),
            CFGetTypeID(named) == AXUIElementGetTypeID(),
            let words = string(
                unsafeDowncast(named, to: AXUIElement.self),
                kAXValueAttribute
            )
        else { return nil }
        return words.hasSuffix(":") ? String(words.dropLast()) : words
    }

    /// What the tree gives under the parameterized `attribute` for the whole
    /// range of a static text's words; nil when the element is no static
    /// text or has nothing to give, and an error when the read fails.
    fileprivate static func wholeRange(
        of element: AXUIElement,
        attribute: String,
        value: AccessibilityTree.Value?,
        role: String,
        subject: String
    ) throws -> CFTypeRef? {
        guard
            let words = AccessibilityTree.Node.staticText(
                role: role,
                value: value
            )
        else { return nil }
        var range = CFRange(location: 0, length: words.utf16.count)
        guard let rangeValue = AXValueCreate(.cfRange, &range) else {
            throw AccessibilityTree.ReadError.attributeFailed(
                subject,
                element: described(element),
                attribute: attribute,
                status: AXError.illegalArgument.rawValue
            )
        }
        var result: CFTypeRef?
        let status = AXUIElementCopyParameterizedAttributeValue(
            element,
            attribute as CFString,
            rangeValue,
            &result
        )
        switch status {
        case .success:
            return result
        // The tree lists some static texts, such as a few of a scene's
        // credits and addresses, that refuse the range of their own words as
        // an illegal argument, or no longer answer: they have nothing to
        // give. Any other failure is an error.
        case .attributeUnsupported, .parameterizedAttributeUnsupported,
            .noValue, .invalidUIElement, .illegalArgument:
            treeLogger.debug(
                "No \(attribute) for \(described(element)): error \(status.rawValue)"
            )
            return nil
        default:
            throw AccessibilityTree.ReadError.attributeFailed(
                subject,
                element: described(element),
                attribute: attribute,
                status: status.rawValue
            )
        }
    }

    /// The bounds the tree gives the whole range of a static text's words,
    /// in the tree's coordinates.
    fileprivate static func wordsBox(
        of element: AXUIElement,
        value: AccessibilityTree.Value?,
        role: String,
        subject: String
    ) throws -> CGRect? {
        guard
            let bounds = try wholeRange(
                of: element,
                attribute: kAXBoundsForRangeParameterizedAttribute,
                value: value,
                role: role,
                subject: subject
            )
        else { return nil }
        var box = CGRect.zero
        guard CFGetTypeID(bounds) == AXValueGetTypeID(),
            AXValueGetValue(
                unsafeDowncast(bounds, to: AXValue.self),
                .cgRect,
                &box
            ), box.isFinite
        else {
            throw AccessibilityTree.ReadError.attributeFailed(
                subject,
                element: described(element),
                attribute: kAXBoundsForRangeParameterizedAttribute,
                status: AXError.failure.rawValue
            )
        }
        guard !box.isEmpty else {
            // A text with no words laid out has an empty box.
            treeLogger.debug(
                "Empty bounds of the words of \(described(element))"
            )
            return nil
        }
        return box
    }
}

extension AccessibilityTree {
    /// Why a tree could not be read; `subject` names what it was read for.
    enum ReadError: Error, CustomStringConvertible {
        case noScreen(String)
        case unavailable(String, Int32)
        case listsNoWindow(String)
        case windowNotListed(String, wanted: CGRect, listed: [CGRect])
        case attributeFailed(
            String,
            element: String,
            attribute: String,
            status: Int32
        )

        var description: String {
            switch self {
            case .noScreen(let subject):
                "no display to place \(subject)'s window against"
            case .unavailable(let subject, let status):
                "the accessibility API listed no windows of this process for "
                    + "\(subject) (error \(status))"
            case .listsNoWindow(let subject):
                "the accessibility API lists no window of this process for "
                    + "\(subject); it lists none while the screen is locked"
            case .windowNotListed(let subject, let wanted, let listed):
                "the accessibility tree lists no window with the identifier "
                    + "of \(subject)'s window, at \(wanted); it lists windows "
                    + "at \(listed)"
            case .attributeFailed(
                let subject,
                let element,
                let attribute,
                let status
            ):
                "the accessibility API failed to give \(attribute) of "
                    + "\(element) in \(subject)'s window (error \(status))"
            }
        }
    }
}

extension AccessibilityTree.Reader {
    /// The elements `element` holds, in the order VoiceOver reads them:
    /// the navigation order where the element gives one, which differs
    /// from the plain list of children for a titled window. A window lists
    /// its content first and its title bar's buttons, toolbar and title
    /// after it, and reads them title bar first. An element that has
    /// neither list holds nothing; any other failure to read one throws.
    static func children(
        of element: AXUIElement,
        subject: String
    ) throws -> [AXUIElement] {
        let navigation = NSAccessibility.Attribute
            .childrenInNavigationOrderAttribute.rawValue
        return try elementList(element, navigation, subject: subject)
            ?? elementList(element, kAXChildrenAttribute, subject: subject)
            ?? []
    }

    /// The elements `element`'s attribute `name` lists, or nil when the
    /// element has no such attribute or it holds no value. Any other
    /// failure, or a value that is not a list of elements, throws.
    private static func elementList(
        _ element: AXUIElement,
        _ name: String,
        subject: String
    ) throws -> [AXUIElement]? {
        var result: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(
            element,
            name as CFString,
            &result
        )
        switch status {
        case .success:
            if let list = result as? [AXUIElement] { return list }
        case .attributeUnsupported, .noValue:
            return nil
        default:
            break
        }
        throw AccessibilityTree.ReadError.attributeFailed(
            subject,
            element: described(element),
            attribute: name,
            status: status.rawValue
        )
    }

    /// The element as an error names it: its role and its box.
    private static func described(_ element: AXUIElement) -> String {
        let role = string(element, kAXRoleAttribute) ?? "an element"
        guard let frame = frame(of: element) else { return role }
        return "\(role) at \(frame)"
    }
}

extension CGRect {
    fileprivate var isFinite: Bool {
        origin.x.isFinite && origin.y.isFinite && size.width.isFinite
            && size.height.isFinite
    }
}

extension AccessibilityTree {
    /// This tree with `other`'s elements after its own, or before the node at
    /// `index` and the nodes it holds, `other` read from a window that sits at
    /// `x`, `y` points from this tree's window's top-left.
    func adding(
        _ other: Self,
        atX x: Double,
        y: Double,
        before index: Int? = nil
    ) -> Self {
        let at = index ?? nodes.count
        let count = other.nodes.count
        return Self(
            nodes: nodes[..<at].map { $0 }
                + other.nodes.map {
                    $0.placed(dx: x, dy: y, parent: $0.parent.map { $0 + at })
                }
                + nodes[at...]
                .map {
                    $0.placed(
                        dx: 0,
                        dy: 0,
                        parent: $0.parent.map { $0 >= at ? $0 + count : $0 }
                    )
                }
        )
    }

    /// This tree without the nodes `isLeftOut` picks and the nodes they hold.
    func leavingOut(_ isLeftOut: (Node) -> Bool) -> Self {
        pruned(from: ()) { node, _ in (!isLeftOut(node), ()) }
    }

    /// This tree without the nodes `visit` rejects and the nodes they hold.
    /// `visit` takes a node and what the node that holds it passed down (`root`
    /// for a node nothing holds), and says whether to keep the node and what
    /// to pass to the nodes it holds.
    private func pruned<Context>(
        from root: Context,
        _ visit: (Node, Context) -> (keep: Bool, passed: Context)
    ) -> Self {
        // The new index of each node kept, and what it passes down, by the
        // node's old index.
        var kept: [Int: Int] = [:]
        var passedDown: [Int: Context] = [:]
        var out: [Node] = []
        for (index, node) in nodes.enumerated() {
            var parent: Int?
            var context = root
            if let old = node.parent {
                // A node whose holder was left out goes with it.
                guard let new = kept[old], let held = passedDown[old] else {
                    continue
                }
                parent = new
                context = held
            }
            let (keep, passed) = visit(node, context)
            guard keep else { continue }
            kept[index] = out.count
            passedDown[index] = passed
            out.append(node.placed(dx: 0, dy: 0, parent: parent))
        }
        return Self(nodes: out)
    }
}

extension AccessibilityTree {
    /// This tree without the nodes a person cannot see: a node whose box lies
    /// wholly outside the window's content rect, or wholly outside the box of
    /// the scroll area that holds it, is a row scrolled out of view or
    /// clipped. The nodes such a node holds go with it.
    func leavingOutWhatIsNotVisible(windowSize: CGSize) -> Self {
        // What a node shows through: the window's rect, cut down by the
        // scroll area that holds it.
        pruned(from: CGRect(origin: .zero, size: windowSize)) { node, area in
            let box = node.frame
            let overlaps =
                box.maxX > area.minX && box.minX < area.maxX
                && box.maxY > area.minY && box.minY < area.maxY
            let passed =
                node.role == kAXScrollAreaRole ? area.intersection(box) : area
            return (overlaps, passed)
        }
    }
}

extension AccessibilityTree.Node {
    var frame: CGRect { CGRect(x: x, y: y, width: width, height: height) }

    /// The name VoiceOver reads for a control: its title, else its
    /// description, else the words of the element it names itself by.
    var controlName: String? { title ?? description ?? titleElementWords }

    var staticText: String? { Self.staticText(role: role, value: value) }

    fileprivate static func staticText(
        role: String,
        value: AccessibilityTree.Value?
    ) -> String? {
        guard role == kAXStaticTextRole, case .text(let words)? = value
        else { return nil }
        return words
    }

    /// This node moved `dx`, `dy` points, held by the node at `parent`.
    fileprivate func placed(dx: Double, dy: Double, parent: Int?) -> Self {
        Self(
            role: role,
            subrole: subrole,
            title: title,
            description: description,
            titleElementWords: titleElementWords,
            placeholder: placeholder,
            value: value,
            x: x + dx,
            y: y + dy,
            width: width,
            height: height,
            enabled: enabled,
            selected: selected,
            focused: focused,
            style: style,
            parent: parent,
            wordsBox: wordsBox?.offsetBy(dx: dx, dy: dy)
        )
    }
}

extension AccessibilityTree {
    /// The element the accessibility API lists for `window`, and the
    /// window's top-left in the API's coordinates: points from the top-left
    /// of the main display.
    @MainActor
    static func listedWindow(
        _ window: NSWindow,
        subject: String
    ) throws -> (element: AXUIElement, origin: CGPoint) {
        guard let main = NSScreen.screens.first else {
            throw ReadError.noScreen(subject)
        }
        let frame = window.frame
        let origin = CGPoint(x: frame.minX, y: main.frame.maxY - frame.maxY)
        let size = frame.size
        let application = AXUIElementCreateApplication(getpid())
        AXUIElementSetMessagingTimeout(application, 10)
        var windows: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(
            application,
            kAXWindowsAttribute as CFString,
            &windows
        )
        guard status == .success, let list = windows as? [AXUIElement] else {
            throw ReadError.unavailable(subject, status.rawValue)
        }
        // A locked screen lists no window of any app.
        guard !list.isEmpty else {
            throw ReadError.listsNoWindow(subject)
        }
        // Windows of one frame, such as a window, the window that shields it
        // under an alert and a layer's panel over it, are told apart by an
        // identifier that this read gives `window` alone.
        let token = UUID().uuidString
        let previous = window.accessibilityIdentifier()
        window.setAccessibilityIdentifier(token)
        defer { window.setAccessibilityIdentifier(previous) }
        let named = list.first {
            var identifier: CFTypeRef?
            AXUIElementCopyAttributeValue(
                $0,
                kAXIdentifierAttribute as CFString,
                &identifier
            )
            return (identifier as? String) == token
        }
        guard let element = named else {
            throw ReadError.windowNotListed(
                subject,
                wanted: CGRect(origin: origin, size: size),
                listed: list.compactMap(Reader.frame(of:))
            )
        }
        return (element, origin)
    }
}

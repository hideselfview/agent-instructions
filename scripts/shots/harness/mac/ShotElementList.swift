import AppKit
import Foundation

/// Every key of an element but `text` and `placeholder` is written, null when the tree has no
/// answer.
private enum ElementKey: String, CodingKey {
    case role, native, label, value, placeholder, bounds, enabled, selected,
        focused
    case text, parent
}

/// What a shot holds, as its window's accessibility tree lists it: each
/// element's role, words, value and box, written beside the shot's PNG as
/// `<board>@macos.json` for the shot check to compare with the board, in the
/// shape the checker reads on every platform: points from the shot's
/// top-left, `scale` PNG pixels per point, elements in the tree's order, no
/// guesses for what the tree lacks.
struct ShotElementList: Encodable {
    let board: String
    let platform = "macos"
    let size: Size
    let scale: Int
    let elements: [Element]

    struct Size: Encodable, Equatable {
        let width: Double
        let height: Double
    }

    struct Bounds: Encodable, Equatable {
        let x: Double
        let y: Double
        let width: Double
        let height: Double
    }

    /// The style a text element draws its words in.
    struct TextStyle: Encodable, Equatable {
        let size: Double
        /// 100 to 900.
        let weight: Int
        /// `#RRGGBBAA`, sRGB.
        let color: String
    }

    struct Element: Encodable {
        let role: String
        /// The platform's own role string or its secure text field subrole.
        let native: String
        let label: String?
        let value: String?
        /// The words a field shows while it is empty; absent when the tree
        /// gives none.
        let placeholder: String?
        let bounds: Bounds
        let enabled: Bool
        let selected: Bool?
        let focused: Bool?
        /// Absent when the tree gives no style for the element's words.
        let text: TextStyle?
        /// The index in the list of the element that holds this one.
        let parent: Int?

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: ElementKey.self)
            try container.encode(role, forKey: .role)
            try container.encode(native, forKey: .native)
            try container.encode(label, forKey: .label)
            try container.encode(value, forKey: .value)
            try container.encodeIfPresent(placeholder, forKey: .placeholder)
            try container.encode(bounds, forKey: .bounds)
            try container.encode(enabled, forKey: .enabled)
            try container.encode(selected, forKey: .selected)
            try container.encode(focused, forKey: .focused)
            try container.encodeIfPresent(text, forKey: .text)
            try container.encode(parent, forKey: .parent)
        }
    }

    /// The elements of `window`'s accessibility tree, with those of each of
    /// `overlays` where the overlay reads them, boxed in points from the
    /// top-left of the window, which is the shot's top-left. The view the
    /// pixels come from is `size` points big.
    @MainActor
    static func read(
        board: String,
        window: NSWindow,
        overlays: [ShotOverlay],
        size: CGSize,
        scale: Int
    ) throws -> Self {
        var tree = try AccessibilityTree.read(of: window, subject: board)
        // An overlay is a separate window over this one, so this list adds
        // its elements where it draws them over this window.
        let windowRoot = tree.nodes.lastIndex { $0.parent == nil }
        for overlay in overlays {
            let before: Int?
            switch overlay.reading {
            case .unlisted: continue
            case .last: before = nil
            case .before(let index): before = index(tree, windowRoot)
            }
            let panel = overlay.panel
            tree = tree.adding(
                try AccessibilityTree.read(of: panel, subject: board),
                atX: panel.frame.minX - window.frame.minX,
                y: window.frame.maxY - panel.frame.maxY,
                before: before
            )
        }
        // A row scrolled out of its pane, or clipped by the window, is not on
        // the screen, so the list leaves it out.
        tree = tree.leavingOutWhatIsNotVisible(windowSize: size)
        // The window's own buttons are the platform's frame, not the view:
        // a board draws them and hides them from its reading order. A scroll
        // view's scroll bar, with the buttons it holds, is the platform's
        // too: it shows on the screen only while the person scrolls, and a
        // board, which crops its window, draws none. A stepper's arrow buttons
        // belong to the platform's control: the stepper is the one element.
        tree = tree.leavingOut { node in
            node.subrole.map(windowButtons.contains) == true
                || node.subrole.map(stepperArrows.contains) == true
                || node.role == NSAccessibility.Role.scrollBar.rawValue
        }
        return Self(
            board: board,
            size: Size(width: size.width, height: size.height),
            scale: scale,
            elements: tree.nodes.map(Element.init(node:))
        )
    }

    /// The subroles of a stepper's two arrow buttons.
    private static let stepperArrows: Set<String> = [
        NSAccessibility.Subrole.incrementArrow.rawValue,
        NSAccessibility.Subrole.decrementArrow.rawValue,
    ]

    /// The subroles of a window's close, minimise, zoom and full screen
    /// buttons.
    private static let windowButtons: Set<String> = [
        NSAccessibility.Subrole.closeButton.rawValue,
        NSAccessibility.Subrole.minimizeButton.rawValue,
        NSAccessibility.Subrole.zoomButton.rawValue,
        NSAccessibility.Subrole.fullScreenButton.rawValue,
    ]

    /// The JSON the list is written as.
    func json() throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return try encoder.encode(self)
    }
}

extension ShotElementList.Element {
    /// The element a tree node is: its role from the platform's own, its
    /// words, value and look as the tree gives them.
    init(node: AccessibilityTree.Node) {
        let role = Self.role(native: node.role, subrole: node.subrole)
        let secureSubrole = NSAccessibility.Subrole.secureTextField.rawValue
        let isSecure = node.subrole == secureSubrole
        let value =
            role == "text" || role == "radio" || role == "stepper"
            ? nil : Self.valueText(node.value, role: role)
        self.init(
            role: role,
            native: isSecure ? secureSubrole : node.role,
            label: role == "text"
                ? Self.words(node.value) : node.controlName,
            value: isSecure && value != nil ? "\u{F79A}" : value,
            placeholder: node.placeholder,
            bounds: Self.bounds(of: node),
            enabled: node.enabled,
            selected: role == "radio"
                ? Self.radioChecked(node.value) : node.selected,
            focused: node.focused,
            text: role == "text" ? node.style.flatMap(Self.textStyle) : nil,
            parent: node.parent
        )
    }

    /// The box of a text is its words' own when they sit inside the element's
    /// box with room on both sides: a text that is the only child of a
    /// padded container, such as a chip or a cell, has the container's box,
    /// where the board's text has its own. A text that starts at the box's
    /// edge keeps the box, which is what the board's text fills.
    private static func bounds(of node: AccessibilityTree.Node)
        -> ShotElementList.Bounds
    {
        let frame = node.frame
        let box =
            node.wordsBox.flatMap {
                frame.insetBy(dx: -0.5, dy: -0.5).contains($0)
                    && $0.minX - frame.minX >= 1 && frame.maxX - $0.maxX >= 1
                    ? $0 : nil
            } ?? frame
        return ShotElementList.Bounds(
            x: box.minX,
            y: box.minY,
            width: box.width,
            height: box.height
        )
    }

    private static func words(_ value: AccessibilityTree.Value?) -> String? {
        guard case .text(let text)? = value, !text.isEmpty else { return nil }
        return text
    }

    /// A radio's value is 1 while it is chosen and 0 otherwise. The list
    /// writes that as `selected` and gives the radio no value, as the board
    /// side does.
    private static func radioChecked(_ value: AccessibilityTree.Value?) -> Bool?
    {
        guard case .number(let number)? = value else { return nil }
        return number != 0
    }

    /// A toggle's value is `"on"` or `"off"`; any other number is written as
    /// the tree gives it.
    private static func valueText(
        _ value: AccessibilityTree.Value?,
        role: String
    ) -> String? {
        switch value {
        case .text(let text)?: text.isEmpty ? nil : text
        case .number(let number)?:
            role == "toggle"
                ? (number == 0 ? "off" : "on")
                : (number == number.rounded()
                    ? String(Int(number)) : String(number))
        case nil: nil
        }
    }

    private static func role(native: String, subrole: String?) -> String {
        if subrole == "AXTabButton" { return "tab" }
        if subrole == "AXSwitch" { return "toggle" }
        return roles[native] ?? "other"
    }

    /// The element roles by the platform's own role.
    private static let roles: [String: String] = [
        NSAccessibility.Role.staticText.rawValue: "text",
        NSAccessibility.Role.button.rawValue: "button",
        NSAccessibility.Role.checkBox.rawValue: "toggle",
        NSAccessibility.Role.radioButton.rawValue: "radio",
        NSAccessibility.Role.textField.rawValue: "field",
        NSAccessibility.Role.textArea.rawValue: "field",
        NSAccessibility.Role.comboBox.rawValue: "field",
        NSAccessibility.Role.popUpButton.rawValue: "popup",
        NSAccessibility.Role.menuButton.rawValue: "popup",
        NSAccessibility.Role.link.rawValue: "link",
        NSAccessibility.Role.image.rawValue: "image",
        NSAccessibility.Role.slider.rawValue: "slider",
        NSAccessibility.Role.incrementor.rawValue: "stepper",
        NSAccessibility.Role.progressIndicator.rawValue: "progress",
        // AppKit's name for a progress indicator of unknown length, which
        // is what a spinner is.
        NSAccessibility.Role.busyIndicator.rawValue: "progress",
    ]

    /// The look the tree reports, nil when its font's weight is not one of
    /// the nine named weights.
    private static func textStyle(
        _ style: AccessibilityTree.TextStyle
    ) -> ShotElementList.TextStyle? {
        let part = style.fontName.split(separator: "-").last.map(String.init)
        let named = (part ?? "").replacingOccurrences(of: "Italic", with: "")
        guard let weight = weights[named.isEmpty ? "Regular" : named] else {
            return nil
        }
        func byte(_ value: Double) -> Int { Int((value * 255).rounded()) }
        return ShotElementList.TextStyle(
            size: style.size,
            weight: weight,
            color: String(
                format: "#%02X%02X%02X%02X",
                byte(style.red),
                byte(style.green),
                byte(style.blue),
                byte(style.alpha)
            )
        )
    }

    /// The weight names a font name ends in, on the 100 to 900 scale.
    private static let weights: [String: Int] = [
        "Ultralight": 100, "Thin": 200, "Light": 300, "Regular": 400,
        "Medium": 500, "Semibold": 600, "Bold": 700, "Heavy": 800,
        "Black": 900,
    ]
}

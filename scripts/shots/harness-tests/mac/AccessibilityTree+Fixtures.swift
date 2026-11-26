@testable import ShotHarness

extension AccessibilityTree.Node {
    /// A node a test lists by hand: its role and box, with nothing else the
    /// tree could give unless the test names it.
    static func fixture(
        role: String,
        title: String? = nil,
        description: String? = nil,
        value: AccessibilityTree.Value? = nil,
        x: Double = 0,
        y: Double = 0,
        width: Double = 100,
        height: Double = 20,
        parent: Int? = nil
    ) -> Self {
        Self(
            role: role,
            subrole: nil,
            title: title,
            description: description,
            placeholder: nil,
            value: value,
            x: x,
            y: y,
            width: width,
            height: height,
            enabled: true,
            selected: nil,
            focused: nil,
            style: nil,
            parent: parent
        )
    }
}

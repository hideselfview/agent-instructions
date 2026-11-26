import CoreGraphics
import Testing

@testable import ShotHarness

/// The shots list only what a person can see: a row scrolled out of its
/// scroll area, or beyond the window's content, is left out.
struct VisibleElementsTests {
    private func labels(_ tree: AccessibilityTree) -> [String?] {
        tree.nodes.map(\.description)
    }

    @Test
    func leavesOutARowBelowTheWindow() {
        let tree = AccessibilityTree(nodes: [
            .fixture(role: "AXButton", description: "inside", y: 100),
            .fixture(role: "AXButton", description: "below", y: 450),
            .fixture(role: "AXButton", description: "straddling", y: 410),
        ])
        let shown = tree.leavingOutWhatIsNotVisible(
            windowSize: CGSize(width: 400, height: 420)
        )
        #expect(labels(shown) == ["inside", "straddling"])
    }

    @Test
    func leavesOutARowScrolledOutOfItsScrollArea() {
        func button(
            _ label: String,
            y: Double,
            parent: Int?
        ) -> AccessibilityTree.Node {
            .fixture(
                role: "AXButton",
                description: label,
                y: y,
                parent: parent
            )
        }
        let tree = AccessibilityTree(nodes: [
            .fixture(
                role: "AXScrollArea",
                description: "pane",
                width: 400,
                height: 200
            ),
            button("in view", y: 50, parent: 0),
            button("scrolled out", y: 250, parent: 0),
            button("its child", y: 255, parent: 2),
            button("outside the pane", y: 300, parent: nil),
        ])
        let shown = tree.leavingOutWhatIsNotVisible(
            windowSize: CGSize(width: 400, height: 600)
        )
        #expect(labels(shown) == ["pane", "in view", "outside the pane"])
        #expect(shown.nodes[1].parent == 0)
    }
}

import AppKit
import Testing

@testable import ShotHarness

/// How a tree node reads as an element of a shot's list.
struct ShotElementTests {
    /// macOS gives a radio button its checked state as a value of 1 or 0; the
    /// list writes it as `selected` and leaves the value out, as the board
    /// side does.
    @Test
    func listsARadiosCheckedStateAsSelected() {
        func radio(_ value: Double) -> ShotElementList.Element {
            ShotElementList.Element(
                node: .fixture(
                    role: NSAccessibility.Role.radioButton.rawValue,
                    title: "Albums",
                    value: .number(value)
                )
            )
        }
        #expect(radio(1).role == "radio")
        #expect(radio(1).selected == true)
        #expect(radio(1).value == nil)
        #expect(radio(0).selected == false)
        #expect(radio(0).value == nil)
    }

    /// A text that is the only child of a padded container reports the
    /// container's box through the tree; the list boxes it by its words, as
    /// the board's text is. A text that starts at its box's edge keeps the
    /// box, which is what the board's text fills.
    @Test
    func boxesATextByItsWordsWhenTheyAreCentredInTheElement() {
        func text(words: CGRect) -> ShotElementList.Element {
            var node = AccessibilityTree.Node.fixture(
                role: NSAccessibility.Role.staticText.rawValue,
                value: .text("Barcode"),
                width: 70,
                height: 17
            )
            node.wordsBox = words
            return ShotElementList.Element(node: node)
        }
        let centred = text(words: CGRect(x: 6, y: 2, width: 44, height: 12))
        #expect(centred.bounds.x == 6)
        #expect(centred.bounds.y == 2)
        #expect(centred.bounds.width == 44)
        #expect(centred.bounds.height == 12)
        let flush = text(words: CGRect(x: 0, y: 2, width: 44, height: 12))
        #expect(flush.bounds.x == 0)
        #expect(flush.bounds.width == 70)
        #expect(flush.bounds.height == 17)
    }
}

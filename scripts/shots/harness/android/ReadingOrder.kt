package builds.shots

import android.content.Context
import android.view.View
import android.view.accessibility.AccessibilityManager
import android.view.accessibility.AccessibilityNodeInfo
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.semantics.SemanticsNode
import androidx.test.core.app.ApplicationProvider
import org.robolectric.Shadows

/**
 * The order TalkBack reads a Compose screen's nodes in, from Compose's own accessibility delegate. The node provider
 * gives TalkBack no sorted child list: the delegate sorts all nodes by place on screen and traversal index into one
 * chain, and tells each node the node that follows it. [ElementList] lists each node's children by their place in
 * that chain; a node that is no stop of the chain goes where the chain first reaches something under it.
 */
internal fun interface ReadingOrder {
    /** The semantics node ids under [parent], in reading order, or null when the provider has no such node. */
    fun childrenOf(parent: Int): List<Int>?

    companion object {
        private const val ID_SHIFT = 32
        private const val NONE = -1
        private const val BEFORE_KEY = "android.view.accessibility.extra.EXTRA_DATA_TEST_TRAVERSALBEFORE_VAL"
        private const val AFTER_KEY = "android.view.accessibility.extra.EXTRA_DATA_TEST_TRAVERSALAFTER_VAL"

        /** The id of the provider's host node, whose children are the children of the semantics root. */
        const val HOST = -1

        /**
         * Turns on Compose's accessibility sorting for the screen that [root] belongs to, as an enabled TalkBack does.
         * Compose sorts only while an accessibility service runs.
         */
        fun enableTalkBack(root: SemanticsNode) {
            // Compose sends accessibility events while it is forced on, which the platform's manager refuses when off.
            Shadows
                .shadowOf(
                    ApplicationProvider.getApplicationContext<Context>().getSystemService(
                        Context.ACCESSIBILITY_SERVICE,
                    ) as AccessibilityManager,
                ).setEnabled(true)
            (root.root as ViewRootForTest).forceAccessibilityForTesting(true)
        }

        /** The reading order of the Compose view below [window], after [enableTalkBack]. */
        fun of(window: View): ReadingOrder {
            val view = window.composeView()
            val provider = view.accessibilityNodeProvider
            val childId = AccessibilityNodeInfo::class.java.getMethod("getChildId", Int::class.java)

            fun virtual(id: Long) = (id ushr ID_SHIFT).toInt()
            val children = mutableMapOf<Int, List<Int>>()
            val next = mutableMapOf<Int, Int>()

            fun read(id: Int) {
                if (id in children) return
                val node = provider.createAccessibilityNodeInfo(id) ?: return
                // A child that is an Android view, not a node, has the host's id.
                children[id] = (0 until node.childCount).map { virtual(childId.invoke(node, it) as Long) }.filter { it != HOST }
                children.getValue(id).forEach(::read)
            }
            read(HOST)
            val delegate =
                view.javaClass
                    .getDeclaredField("composeAccessibilityDelegate")
                    .apply { isAccessible = true }
                    .get(view)!!
            // The node ids of Compose's chain: the delegate's map from each node to the node read after it.
            val map =
                delegate.javaClass
                    .getDeclaredField("idToBeforeMap")
                    .apply { isAccessible = true }
                    .get(delegate)!!
            val lookup = map.javaClass.getMethod("getOrDefault", Int::class.java, Int::class.java)
            for (id in children.keys) (lookup.invoke(map, id, NONE) as Int).takeIf { it != NONE }?.let { next[id] = it }
            val rank = mutableMapOf<Int, Int>()
            for (head in children.keys.filter { it !in next.values && it in next.keys }) {
                var at: Int? = head
                while (at != null && at !in rank) {
                    rank[at] = rank.size
                    at = next[at]
                }
            }
            // A node TalkBack never stops on (a layout with no semantics of its own) is read where its first
            // descendant is, so it takes the earliest rank at or under it.
            val reached = mutableMapOf<Int, Int>()

            fun earliest(id: Int): Int =
                reached.getOrPut(id) {
                    (children[id].orEmpty().map(::earliest) + listOfNotNull(rank[id])).minOrNull() ?: Int.MAX_VALUE
                }
            val full = ReadingOrder { parent -> children[parent]?.sortedBy(::earliest) }
            // A node of the screen keeps the order it has with no window up: a window is read apart from the screen.
            val screen = ShotWindows.pageOrder ?: return full
            return ReadingOrder { parent -> screen.childrenOf(parent) ?: full.childrenOf(parent) }
        }
    }
}

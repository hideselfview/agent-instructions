package builds.shots

import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.LayoutInfo
import androidx.compose.ui.node.ModifierNodeElement
import androidx.compose.ui.semantics.SemanticsNode

/**
 * Marks a layout of a shot's scene as a window that the app opens over its screen: an open menu, a sheet or a dialog.
 * A shot's picture holds none of those windows, so a scene draws their contents inside the screen. TalkBack moves its
 * focus into a window that opens, so [ElementList] lists the elements under a marked layout before the screen's own.
 * The mark adds no semantics, so it leaves Compose's reading chain as the app builds it.
 */
internal fun Modifier.windowOverScreen(): Modifier = this.then(WindowMark)

/** Whether the node is in a layout the scene marks as a window the app opens over its screen ([windowOverScreen]). */
internal fun SemanticsNode.inWindow(): Boolean =
    generateSequence<LayoutInfo>(layoutInfo) { it.parentInfo }.any { layout ->
        layout.getModifierInfo().any { it.modifier === WindowMark }
    }

private data object WindowMark : ModifierNodeElement<WindowMarkNode>() {
    override fun create() = WindowMarkNode()

    override fun update(node: WindowMarkNode) = Unit
}

private class WindowMarkNode : Modifier.Node()

/**
 * The windows that one shot's scene opens over its screen, which [settle] gives the scene. A window in the app has a
 * reading order of its own, apart from the screen's, but a scene draws it inside the screen, where Compose sorts its
 * nodes together with the screen's by place and moves the screen's nodes. So [settle] draws the screen alone first and
 * returns its reading order, then draws the windows.
 */
internal class ShotWindows {
    /** Whether the scene draws its windows; [settle] turns them on after it reads the screen's own reading order. */
    var shown by mutableStateOf(false)

    /** Whether the scene holds a window drawn through [WindowOverScreen]. */
    var declared = false

    /** Whether Compose has applied a composition that draws the windows. */
    var composed = false
}

internal val LocalShotWindows =
    staticCompositionLocalOf<ShotWindows> { error("WindowOverScreen draws only in a scene that settle draws") }

/** [content] as a window over the screen: drawn once [settle] has read the screen's reading order. */
@Composable
internal fun WindowOverScreen(content: @Composable () -> Unit) {
    val windows = LocalShotWindows.current
    SideEffect { windows.declared = true }
    if (windows.shown) {
        content()
        SideEffect { windows.composed = true }
    }
}

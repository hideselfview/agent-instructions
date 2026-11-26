package builds.shots

import android.view.View
import androidx.compose.animation.core.Transition
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.currentComposer
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.runtime.tooling.CompositionData
import androidx.compose.runtime.tooling.LocalInspectionTables
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.isFocused
import androidx.compose.ui.test.junit4.ComposeContentTestRule
import androidx.compose.ui.test.onRoot

private const val SETTLE_MILLIS = 100L

/**
 * Draws [content] in [look] and stops at the same frame every run: the clock advances only by [SETTLE_MILLIS], by the
 * frames that compose the scene's windows, and, while a field has focus, by the frames that its transitions take, so a
 * spinner, a looping animation or a progress bar is caught at one moment, and an animation that never ends can't keep
 * the test waiting for idle. The content stages its own theme.
 *
 * Returns the reading order of the screen drawn without the windows the scene opens over it, which
 * [ReadingOrder.of] takes, or null for a scene that opens none.
 */
internal fun ComposeContentTestRule.settle(
    look: ShotLook,
    content: @Composable () -> Unit,
): ReadingOrder? {
    mainClock.autoAdvance = false
    setTestAnimatorScale(0f)
    val windows = ShotWindows()
    // The data of the scene's composition, which the scene adds, and of each subcomposition, which Compose adds.
    val compositions = mutableSetOf<CompositionData>()
    setContent {
        compositions.add(currentComposer.compositionData)
        CompositionLocalProvider(LocalShotWindows provides windows, LocalInspectionTables provides compositions) {
            look.Dress(content)
        }
    }
    ReadingOrder.enableTalkBack(onRoot().fetchSemanticsNode())
    mainClock.advanceTimeBy(SETTLE_MILLIS)
    // The windows a scene opens over its screen come after the screen's reading order, read with no window up.
    val screenOrder =
        if (windows.declared) ReadingOrder.of(onRoot().fetchSemanticsNode().root as View, screen = null) else null
    windows.shown = true
    // The write is outside a composition, so the compose runtime hears of it only once it is told.
    Snapshot.sendApplyNotifications()
    if (windows.declared) {
        mainClock.advanceTimeUntil { windows.composed }
        waitForIdle()
    }
    // A field that takes focus raises its label in a transition, which the steps above can stop partway through.
    if (onAllNodes(hasSetTextAction() and isFocused()).fetchSemanticsNodes().isNotEmpty()) {
        // A field that has only now taken focus starts its transition in the next frame's composition.
        mainClock.advanceTimeByFrame()
        mainClock.advanceTimeUntil { compositions.all { data -> data.transitions().all { it.hasEnded() } } }
        waitForIdle()
    }
    return screenOrder
}

/** The transitions that the composition holds. */
private fun CompositionData.transitions(): Sequence<Transition<*>> =
    compositionGroups.asSequence().flatMap { group ->
        group.data.asSequence().filterIsInstance<Transition<*>>() + group.transitions()
    }

/** Whether the transition is at its target with no animation left to run. */
private fun Transition<*>.hasEnded(): Boolean = currentState == targetState && !isRunning

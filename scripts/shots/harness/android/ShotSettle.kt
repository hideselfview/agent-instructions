package builds.shots

import android.view.View
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.isFocused
import androidx.compose.ui.test.junit4.ComposeContentTestRule
import androidx.compose.ui.test.onRoot

private const val SETTLE_MILLIS = 100L
private const val FOCUS_SETTLE_MILLIS = 200L

/**
 * Draws [content] in [look] and stops at the same frame every run: the clock advances only by [SETTLE_MILLIS] (and by
 * [FOCUS_SETTLE_MILLIS] more while a field has focus), so a spinner, a looping animation or a progress bar is caught
 * at one moment, and an animation that never ends can't keep the test waiting for idle. The content stages its own
 * theme.
 */
internal fun ComposeContentTestRule.settle(
    look: ShotLook,
    content: @Composable () -> Unit,
) {
    mainClock.autoAdvance = false
    setTestAnimatorScale(0f)
    val windows = ShotWindows()
    setContent { CompositionLocalProvider(LocalShotWindows provides windows) { look.Dress(content) } }
    ReadingOrder.enableTalkBack(onRoot().fetchSemanticsNode())
    mainClock.advanceTimeBy(SETTLE_MILLIS)
    // The windows a scene opens over its screen come after the screen's reading order, read with no window up.
    if (windows.declared) {
        val screen = onRoot().fetchSemanticsNode().root as View
        screen.tag = ScreenOrder(ReadingOrder.of(screen))
    }
    windows.shown = true
    // The write is outside a composition, so the compose runtime hears of it only once it is told.
    Snapshot.sendApplyNotifications()
    if (windows.declared) mainClock.advanceTimeBy(SETTLE_MILLIS)
    // A field that took focus raises its label over 150 ms, which the first step stops partway through.
    if (onAllNodes(hasSetTextAction() and isFocused()).fetchSemanticsNodes().isNotEmpty()) {
        mainClock.advanceTimeBy(FOCUS_SETTLE_MILLIS)
    }
}

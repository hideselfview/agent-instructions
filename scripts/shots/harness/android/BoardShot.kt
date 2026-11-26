package builds.shots

import android.app.Application
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import com.github.takahirom.roborazzi.ExperimentalRoborazziApi
import com.github.takahirom.roborazzi.RoborazziOptions
import com.github.takahirom.roborazzi.RoborazziTaskType
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Assume.assumeNotNull
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.rules.TestName
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File
import kotlin.math.ceil

/**
 * A board's shot: the real composable drawn in the app's [look] at a fixed phone size, to `<board>@android.png` and
 * its element list `<board>@android.json` in the run's folder ([ShotRequest]). In a run that takes no shots the test
 * is skipped, and so is a board that the run's selection leaves out.
 *
 * A shot is a test function named by its board in PascalCase (`AndroidWelcomeLibraries` is
 * `android.welcome.libraries`), written as camelCase; [shot] refuses a board its function doesn't spell.
 *
 * An app's shots extend a subclass of its own, which gives its look and names with `@RunWith` the Robolectric test
 * runner that runs them.
 */
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], application = Application::class, qualifiers = PHONE_QUALIFIERS)
abstract class BoardShot(
    private val look: ShotLook,
) {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    @get:Rule
    val test = TestName()

    /**
     * Draws a design-system component in one of its states, as [id] (`android.component.<thing>.<state>`) names
     * it, on the app's component page, at the size of its board's preview cell ([ShotBoardSizes]). The component is
     * at the top left with no margin, and the view is proposed the cell's size. A selected state with no board size
     * fails its test, by its board's name. A component that enters with an animation (a snackbar's fade and scale)
     * passes its length as [enterMillis], so the shot draws it entered.
     */
    protected fun component(
        id: String,
        enterMillis: Long = 0L,
        content: @Composable () -> Unit,
    ) {
        assumeRequested(id)
        val size = ShotBoardSizes(ShotRequest.boardSizes).sizeOf(id)
        sizeWindowTo(size)
        shot(id, enterMillis = enterMillis) {
            look.ComponentPage { Box(modifier = Modifier.size(size)) { content() } }
        }
    }

    /**
     * Draws [content] as [board]. A board whose frame is [height] dp tall instead of the phone's gets a window
     * as tall as the frame, which holds the page the board draws below the phone's fold.
     */
    protected fun shot(
        board: String,
        height: Dp? = null,
        enterMillis: Long = 0L,
        content: @Composable () -> Unit,
    ) {
        val spelled = board.split('.', '-').joinToString("") { it.replaceFirstChar(Char::uppercase) }
        require(spelled.replaceFirstChar(Char::lowercase) == test.methodName) {
            "shot '${test.methodName}' is not the camelCase of board '$board' ($spelled)"
        }
        val directory = assumeRequested(board)
        height?.let { sizeWindowTo(DpSize(PHONE_WIDTH, it)) }
        compose.settle(look, content)
        compose.mainClock.advanceTimeBy(enterMillis)
        write(board, directory)
    }

    /** The folder the run writes [board] to. Skips the test when the run takes no shots or leaves [board] out. */
    private fun assumeRequested(board: String): File {
        val directory = ShotRequest.directory
        assumeNotNull(directory)
        assumeTrue("board '$board' is not selected", ShotRequest.selects(board))
        return checkNotNull(directory)
    }

    /**
     * Gives the activity a window as wide and as tall as [size], the board's preview cell, so a view that fills
     * the cell (a dialog centred in it, a sheet at its foot) sits where its board draws it, even when the cell is
     * wider than the phone.
     */
    private fun sizeWindowTo(size: DpSize) {
        val width = ceil(size.width.value).toInt()
        val height = ceil(size.height.value).toInt()
        // Robolectric swaps the two when the orientation it infers disagrees with them, so the orientation is named.
        val orientation = if (width > height) "land" else "port"
        RuntimeEnvironment.setQualifiers("+w${width}dp-h${height}dp-$orientation")
        compose.activityRule.scenario.recreate()
    }

    /** Writes the settled screen's picture and element list as [board]'s into [directory]. */
    @OptIn(ExperimentalRoborazziApi::class)
    private fun write(
        board: String,
        directory: File,
    ) {
        val root = compose.onRoot()
        // The run asks for shots, so Roborazzi records, whatever the build's own Roborazzi properties say.
        root.captureRoboImage(
            File(directory, "$board@android.png").path,
            roborazziOptions = RoborazziOptions(taskType = RoborazziTaskType.Record),
        )
        val window = compose.activity.window.decorView
        val embedded = EmbeddedViews.of(window, compose.activity.resources.displayMetrics.density, look::drawnBox)
        val unmerged = compose.onRoot(useUnmergedTree = true).fetchSemanticsNode()
        File(directory, "$board@android.json")
            .writeText(
                ElementList
                    .of(board, root.fetchSemanticsNode(), embedded, unmerged, ReadingOrder.of(window))
                    .toString(JSON_INDENT) + "\n",
            )
    }
}

/**
 * 412 by 916 dp at 320 dpi (2 pixels to the dp), dark. A density with a half pixel in a dp or an sp (2.625)
 * rounds each box up to a whole pixel, which the board, drawn in CSS pixels, does not, so a shot's rows would not
 * match its board's.
 */
private val PHONE_WIDTH = 412.dp

private const val PHONE_QUALIFIERS = "w412dp-h916dp-night-320dpi"
private const val JSON_INDENT = 2

package builds.shots

import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import org.json.JSONObject
import java.io.File

/**
 * The size of each component board in dp, the preview row's cell, which `builds shots` measures and writes before the
 * capture starts: `{"<board>": {"width", "height"}}`.
 */
internal class ShotBoardSizes(
    file: File,
) {
    private val sizes = JSONObject(file.readText())

    /** The board size of the component [board]; a board with no entry fails. */
    fun sizeOf(board: String): DpSize {
        val entry = checkNotNull(sizes.optJSONObject(board)) { "the board sizes list none for '$board'" }
        return DpSize(entry.getDouble("width").dp, entry.getDouble("height").dp)
    }
}

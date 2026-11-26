package builds.shots

import java.io.File

/**
 * What the checker asks a capture for. It runs the app's capture command with the `BUILDS_SHOTS_*` variables
 * (`builds help`, APP BUILDS), and the app's build passes each on to the test JVM as a system property:
 * `builds.shotsDir`, `builds.shotsSelect` and `builds.shotsBoardSizes`.
 */
internal object ShotRequest {
    /** The folder each shot is written to; null in a test run that takes no shots. */
    val directory: File?
        get() = property("builds.shotsDir")?.let(::File)

    /** Whether [board] is in the run's selection, the comma-separated board-name prefixes of `builds.shotsSelect`. */
    fun selects(board: String): Boolean = isSelected(board, property("builds.shotsSelect"))

    /** The file of the component boards' sizes. */
    val boardSizes: File
        get() = File(checkNotNull(property("builds.shotsBoardSizes")) { "builds.shotsBoardSizes names no board sizes file" })

    private fun property(name: String): String? = System.getProperty(name)?.takeIf { it.isNotEmpty() }
}

/** Whether [board] is in the run: every board when [select] is absent, else those a listed prefix starts. */
internal fun isSelected(
    board: String,
    select: String?,
): Boolean = select.isNullOrEmpty() || select.split(',').any { board.startsWith(it) }

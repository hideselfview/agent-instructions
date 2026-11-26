package builds.shots

import android.graphics.Rect
import android.view.View
import androidx.compose.runtime.Composable

/** How the app draws its views in a shot. */
interface ShotLook {
    /** [content], a shot's whole tree, with what the app's root gives its views, such as composition locals. */
    @Composable
    fun Dress(content: @Composable () -> Unit)

    /** [content], a design-system component's view, on the page colour in the theme the boards draw. */
    @Composable
    fun ComponentPage(content: @Composable () -> Unit)

    /**
     * The box that the app's boards draw for an embedded Android view, in pixels from [view]'s top-left: the whole
     * view, unless the app gives another, such as the line a seek bar draws inside its touch height.
     */
    fun drawnBox(view: View): Rect = Rect(0, 0, view.width, view.height)
}

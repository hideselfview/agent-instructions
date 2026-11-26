package builds.shots

import android.view.View
import android.view.ViewGroup

private const val COMPOSE_VIEW = "androidx.compose.ui.platform.AndroidComposeView"

/** The Compose view at or below this view; a shot's window always holds one. */
internal fun View.composeView(): View = checkNotNull(findComposeView(this)) { "no Compose view below the window" }

private fun findComposeView(view: View): View? =
    if (view.javaClass.name == COMPOSE_VIEW) {
        view
    } else {
        (view as? ViewGroup)?.let { group -> (0 until group.childCount).firstNotNullOfOrNull { findComposeView(group.getChildAt(it)) } }
    }

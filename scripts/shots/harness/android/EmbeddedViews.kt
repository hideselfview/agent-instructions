package builds.shots

import android.graphics.Rect
import android.view.View
import android.view.ViewGroup
import android.widget.ProgressBar
import android.widget.SeekBar
import android.widget.TextView
import org.json.JSONObject
import kotlin.math.roundToInt

/**
 * The elements of the native views a screen embeds with `AndroidView` (such as a seek bar or a progress bar),
 * in the shape [ElementList] writes. Compose's semantics tree stops at an embedded view, which TalkBack
 * reads on through, so a list of the semantics alone would leave a seek bar's clocks and slider out.
 */
internal object EmbeddedViews {
    private const val PERCENT = 100.0
    private const val HOLDER_SUFFIX = "ViewFactoryHolder"

    /**
     * The elements under every embedded view below [window], in view order, with boxes in dp from the Compose view's
     * top-left. [drawnBox] gives the box of a view that its element has, in pixels from the view's top-left.
     */
    fun of(
        window: View,
        density: Float,
        drawnBox: (View) -> Rect,
    ): List<JSONObject> {
        val compose = window.composeView()
        val origin = IntArray(2).also { compose.getLocationInWindow(it) }
        val out = mutableListOf<JSONObject>()
        holders(compose).forEach { holder -> leaves(holder, origin, density, drawnBox, out) }
        return out
    }

    /** The embedded views' holders below [view], outermost first, in the order Compose places them. */
    private fun holders(view: View): List<ViewGroup> {
        if (view !is ViewGroup) return emptyList()
        if (view.javaClass.name.endsWith(HOLDER_SUFFIX)) return listOf(view)
        return (0 until view.childCount).flatMap { holders(view.getChildAt(it)) }
    }

    private fun leaves(
        view: View,
        origin: IntArray,
        density: Float,
        drawnBox: (View) -> Rect,
        out: MutableList<JSONObject>,
    ) {
        if (view.visibility != View.VISIBLE) return
        element(view, origin, density, drawnBox)?.let(out::add)
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) leaves(view.getChildAt(index), origin, density, drawnBox, out)
        }
    }

    private fun element(
        view: View,
        origin: IntArray,
        density: Float,
        drawnBox: (View) -> Rect,
    ): JSONObject? {
        val label = view.contentDescription?.toString()
        // The role is what the view tells TalkBack it is, so a view that reports a progress bar is listed as one.
        val reported =
            checkNotNull(view.createAccessibilityNodeInfo().className) {
                "${view.javaClass.name} reports no accessibility class name"
            }.toString()
        val reportedClass = Class.forName(reported)
        val native = reported.substringAfterLast('.')
        val (kind, words) =
            when {
                SeekBar::class.java.isAssignableFrom(reportedClass) -> "slider" to label
                ProgressBar::class.java.isAssignableFrom(reportedClass) -> "progress" to label
                view is TextView && view.text.isNotEmpty() -> "text" to view.text.toString()
                else -> return null
            }
        val at = IntArray(2).also { view.getLocationInWindow(it) }
        val box = drawnBox(view)
        val out =
            JSONObject()
                .put("role", kind)
                .put("native", native)
                .put("label", words ?: JSONObject.NULL)
                .put("value", percent(view) ?: JSONObject.NULL)
                .put(
                    "bounds",
                    JSONObject()
                        .put("x", dp((at[0] - origin[0] + box.left).toFloat(), density))
                        .put("y", dp((at[1] - origin[1] + box.top).toFloat(), density))
                        .put("width", dp(box.width().toFloat(), density))
                        .put("height", dp(box.height().toFloat(), density)),
                ).put("enabled", view.isEnabled)
                .put("selected", JSONObject.NULL)
                .put("parent", JSONObject.NULL)
        if (view is TextView) out.put("text", style(view))
        return out
    }

    /** How far a bar is, as a whole percent, the way the boards give a bar's value; null for a text or a moving bar. */
    private fun percent(view: View): String? =
        if (view is ProgressBar) {
            view
                .takeIf { !it.isIndeterminate && it.max > 0 }
                ?.let { (it.progress * PERCENT / it.max).roundToInt().toString() }
        } else {
            // A view of its own tells a screen reader the range it is at; none while a bar moves.
            view
                .createAccessibilityNodeInfo()
                .rangeInfo
                ?.current
                ?.roundToInt()
                ?.toString()
        }

    /** The words' size in sp and colour, as the view draws them. */
    private fun style(view: TextView): JSONObject {
        val sp = view.textSize / view.resources.displayMetrics.scaledDensity
        return JSONObject()
            .put("size", tenths(sp))
            .put("color", hex(view.currentTextColor))
    }
}

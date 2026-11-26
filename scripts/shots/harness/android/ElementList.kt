package builds.shots

import android.graphics.Paint
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.isSpecified
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.layout.LayoutInfo
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsConfiguration
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.toSize
import org.json.JSONObject
import kotlin.math.roundToInt

/**
 * The element list a shot writes beside its PNG as `<board>@android.json`: what Compose's merged
 * semantics tree holds, in the order TalkBack reads it (see [ReadingOrder]), in dp from the shot's top-left. This is the only
 * writer; every shot goes through [of]. A merged node of several texts (a clickable row of a title, a credit and a
 * duration) is listed as the one node TalkBack reads and taps, with no label of its own, and holds one text element
 * for each visible text the unmerged tree holds, since the merged node keeps only the first text's layout. A merged
 * node of one text (a clickable menu row) is that text, drawn where the unmerged tree's text node draws it.
 */
internal object ElementList {
    private const val PLATFORM = "android"

    /** A line-break mark that a screen puts in its words, which the board has no word for. */
    private const val ZERO_WIDTH_SPACE = "\u200B"

    private const val TENTH = 10.0
    private const val SLACK = 0.6
    private const val HEX_RADIX = 16
    private const val RGB_DIGITS = 8
    private const val FIELD_LABEL_RISE_DP = 8f
    private const val OUTLINE_HEIGHT_DP = 56f

    /**
     * The list for the tree under [root], with each node's children in [reading] order (the root has no semantics of its own and is not listed), with the
     * [embedded] views' elements, which the tree does not hold, each placed at the bare node the tree keeps for
     * its view, or else where the reading order reaches its box: before the first element that starts lower, or
     * in its row, further right.
     */
    fun of(
        board: String,
        root: SemanticsNode,
        embedded: List<JSONObject>,
        unmerged: SemanticsNode,
        reading: ReadingOrder,
    ): JSONObject {
        val density = root.layoutInfo.density.density
        val listed = mutableListOf<Listed>()
        val frame = Rect(Offset.Zero, root.size.toSize())

        fun inReadingOrder(
            parent: SemanticsNode,
            id: Int = parent.id,
        ) = reading.childrenOf(id).let { order ->
            if (order == null) {
                parent.children
            } else {
                val (held, others) = parent.children.partition { child -> child.id !in order && embedded.any { it.sitsIn(child, density) } }
                // The bare node of an embedded view is no stop of Compose's chain and has no descendant in it, so
                // TalkBack reads the view by its box among the siblings the chain ranks, not after all of them.
                held.fold(others.sortedBy { order.indexOf(it.id).takeIf { at -> at >= 0 } ?: order.size }) { read, view ->
                    val box = view.wholeBox()
                    val at = read.indexOfFirst { it.id in order && startsAfter(it.wholeBox(), box) }
                    read.toMutableList().apply { add(if (at < 0) size else at, view) }
                }
            }
        }

        fun visit(
            node: SemanticsNode,
            parent: Listed?,
            inWindows: Boolean = false,
        ) {
            // A window drawn over the screen is listed first, on its own (below), and not again in its place.
            if (node.inWindow() != inWindows) return
            // A node with no visible part (below the screen's edge, past a scrolling area's edge or under the
            // drawn keyboard) is not listed, as the board reader leaves out such a node of a board. Its children
            // are visited: they hang from the nearest listed node above.
            val label = fieldLabel(node, unmerged)
            val here = if (hidden(node, frame)) parent else Listed(element(node, density, unmerged, label != null), parent)
            if (here != null && here !== parent) {
                // A board draws a field's label as a text of its own before the field; Compose's field holds it inside.
                label
                    ?.takeUnless { hidden(it, frame) }
                    ?.let { listed.add(Listed(element(it, density, unmerged), parent)) }
                listed.add(here)
                // A child that asks to be read first (a negative traversal index, such as a row's leading picture) comes
                // before the merged node's own texts, as the board lists its image before the row's words.
                inReadingOrder(node).filter { it.readsFirst() }.forEach { visit(it, here, inWindows) }
                mergedTexts(
                    node,
                    unmerged,
                ).filterNot { hidden(it, frame) }.forEach { listed.add(Listed(element(it, density, unmerged), here)) }
            }
            // The list visits a node's children in reading order, the order TalkBack reads them in, not the order
            // Compose draws them in.
            inReadingOrder(node).filterNot { here != null && here !== parent && it.readsFirst() }.forEach { visit(it, here, inWindows) }
        }

        // TalkBack moves its focus into a window that opens over a screen (a menu, a sheet, a dialog), so each such
        // window's elements come before the screen's own, in the chain's order inside the window.
        fun windows(node: SemanticsNode): List<SemanticsNode> =
            if (node.inWindow()) listOf(node) else inReadingOrder(node).flatMap(::windows)
        inReadingOrder(root, ReadingOrder.HOST).flatMap(::windows).forEach { visit(it, null, inWindows = true) }
        inReadingOrder(root, ReadingOrder.HOST).forEach { visit(it, null) }
        // The tree keeps a bare, childless node where an embedded view sits. The element list puts the view's
        // elements at that node, in the view's order. When the tree has no such node, the list puts the view where
        // the reading order reaches its box.
        var from = 0
        val placed = mutableSetOf<Listed>()
        for (view in embedded) {
            val holder =
                listed
                    .filter { held -> held !in placed && holds(held.json, view) && listed.none { it.parent === held } }
                    .minByOrNull { area(it.json) }
            val here = Listed(view, null)
            val at =
                if (holder != null) {
                    var next = listed.indexOf(holder) + 1
                    while (next < listed.size && listed[next] in placed) next++
                    next
                } else {
                    (from until listed.size).firstOrNull { startsAfter(listed[it].json, view) } ?: listed.size
                }
            listed.add(at, here)
            placed.add(here)
            from = at + 1
        }
        val elements = org.json.JSONArray()
        listed.forEach { it.json.put("parent", it.parent?.let(listed::indexOf) ?: JSONObject.NULL) }
        listed.forEach { elements.put(it.json) }
        return JSONObject()
            .put("board", board)
            .put("platform", PLATFORM)
            .put(
                "size",
                JSONObject()
                    .put("width", dp(root.size.width, density))
                    .put("height", dp(root.size.height, density)),
            ).put("scale", density.toDouble())
            .put("elements", elements)
    }

    /**
     * Whether a node that takes up room has no part of it inside the shot's [frame] and its ancestors' clips.
     * [SemanticsNode.boundsInRoot] is the box with those clips applied, so a node cut away entirely has an empty one.
     */
    private fun hidden(
        node: SemanticsNode,
        frame: Rect,
    ): Boolean {
        val box = node.wholeBox()
        return !box.isEmpty && (node.boundsInRoot.isEmpty || !box.overlaps(frame))
    }

    /** Whether the node is in a layout the scene marks as a window the app opens over its screen ([windowOverScreen]). */
    private fun SemanticsNode.inWindow(): Boolean = generateSequence<LayoutInfo>(layoutInfo) { it.parentInfo }.any { it.isWindow() }

    /** Whether the node asks to be read before the texts of the merged node that holds it. */
    private fun SemanticsNode.readsFirst(): Boolean = (config.getOrNull(SemanticsProperties.TraversalIndex) ?: 0f) < 0f

    /** An element of the tree and the one that holds it. */
    private class Listed(
        val json: JSONObject,
        val parent: Listed?,
    )

    /** Whether this embedded view's box lies inside the bare node [node], which has no words, no role and no child. */
    private fun JSONObject.sitsIn(
        node: SemanticsNode,
        density: Float,
    ): Boolean {
        if (node.children.isNotEmpty() || node.config.getOrNull(SemanticsProperties.Text) != null) return false
        val box = node.wholeBox()
        val mine = getJSONObject("bounds")
        val slack = SLACK * density
        return box.left - mine.getDouble("x") * density <= slack &&
            box.top - mine.getDouble("y") * density <= slack &&
            (mine.getDouble("x") + mine.getDouble("width")) * density - box.right <= slack &&
            (mine.getDouble("y") + mine.getDouble("height")) * density - box.bottom <= slack
    }

    /** Whether the box [other] starts lower than [view], or in its row and further right. */
    private fun startsAfter(
        other: Rect,
        view: Rect,
    ): Boolean {
        val lower = other.top - view.top
        return lower > view.height / 2 || (Math.abs(lower) <= view.height / 2 && other.left > view.left)
    }

    /** Whether [element] is a bare node (no words, no role) whose box holds [view]'s. */
    private fun holds(
        element: JSONObject,
        view: JSONObject,
    ): Boolean {
        if (element.getString("role") != "other" || !element.isNull("label")) return false
        val box = element.getJSONObject("bounds")
        val mine = view.getJSONObject("bounds")
        return box.getDouble("x") - mine.getDouble("x") <= SLACK &&
            box.getDouble("y") - mine.getDouble("y") <= SLACK &&
            mine.getDouble("x") + mine.getDouble("width") - (box.getDouble("x") + box.getDouble("width")) <= SLACK &&
            mine.getDouble("y") + mine.getDouble("height") - (box.getDouble("y") + box.getDouble("height")) <= SLACK
    }

    private fun area(element: JSONObject): Double {
        val box = element.getJSONObject("bounds")
        return box.getDouble("width") * box.getDouble("height")
    }

    /** Whether [element] starts lower than [view]'s box, or in its row and further right. */
    private fun startsAfter(
        element: JSONObject,
        view: JSONObject,
    ): Boolean {
        val box = element.getJSONObject("bounds")
        val mine = view.getJSONObject("bounds")
        val lower = box.getDouble("y") - mine.getDouble("y")
        return lower > mine.getDouble("height") / 2 ||
            (Math.abs(lower) <= mine.getDouble("height") / 2 && box.getDouble("x") > mine.getDouble("x"))
    }

    private fun element(
        node: SemanticsNode,
        density: Float,
        unmerged: SemanticsNode,
        hasOutlineLabel: Boolean = false,
    ): JSONObject {
        val config = node.config
        val role = config.getOrNull(SemanticsProperties.Role)
        val editable = config.getOrNull(SemanticsProperties.EditableText)
        val texts = config.getOrNull(SemanticsProperties.Text)
        val description = config.getOrNull(SemanticsProperties.ContentDescription)
        val pane = config.getOrNull(SemanticsProperties.PaneTitle)
        val merged = mergedTexts(node, unmerged)
        val ownText = texts != null && merged.isEmpty()
        val kind = kind(config, role, editable != null, ownText, description != null && node.children.isEmpty())
        val named = merged.isEmpty() || kind != "other"
        val drawn = if (kind == "text") drawnText(node, unmerged) else node
        // The box the node draws, not the 48 dp touch target Compose grows around a smaller one: a board draws the
        // box. It is the whole box, also where it reaches past the shot's frame, as a board's box does.
        val bounds =
            when {
                kind == "text" -> {
                    textBox(drawn)
                }

                // Material's outlined field includes the 8dp in which its visible label floats.
                // A field shorter than its outline and that rise has no floating label, such as a search field, whose
                // box is the whole pill.
                hasOutlineLabel && node.wholeBox().height >= (OUTLINE_HEIGHT_DP + FIELD_LABEL_RISE_DP) * density -> {
                    node.wholeBox().let { Rect(it.left, it.top + FIELD_LABEL_RISE_DP * density, it.right, it.bottom) }
                }

                else -> {
                    node.wholeBox()
                }
            }
        val out =
            JSONObject()
                .put("role", kind)
                .put("native", role?.toString() ?: native(editable != null, ownText, config))
                // TalkBack names a node by its content description when it has one, then by its pane title, over the text it holds. A bare
                // node of several texts has the words of its children, which the board lists one by one with no
                // element around them.
                .put(
                    "label",
                    orNull(
                        description?.joinToString(" ") ?: pane
                            ?: texts?.takeIf { named }?.joinToString(" ") { it.text.replace(ZERO_WIDTH_SPACE, "") },
                    ),
                ).put("value", orNull(value(config, editable?.text)))
                .put(
                    "bounds",
                    JSONObject()
                        .put("x", dp(bounds.left, density))
                        .put("y", dp(bounds.top, density))
                        .put("width", dp(bounds.width, density))
                        .put("height", dp(bounds.height, density)),
                ).put("enabled", !config.contains(SemanticsProperties.Disabled))
                .put("selected", orNull(config.getOrNull(SemanticsProperties.Selected)))
        config.getOrNull(SemanticsProperties.Focused)?.let { out.put("focused", it) }
        if (kind == "text") textStyle(drawn.config)?.let { out.put("text", it) }
        return out
    }

    /**
     * The visible text that names an editable field, from inside its node in the unmerged tree.
     * A PIN field sets a description with no visible label.
     */
    private fun fieldLabel(
        node: SemanticsNode,
        unmerged: SemanticsNode,
    ): SemanticsNode? {
        val config = node.config
        if (!config.contains(SemanticsProperties.EditableText)) return null
        val words = config.getOrNull(SemanticsProperties.Text)?.joinToString(" ") { it.text } ?: return null
        val inside = find(unmerged, node.id) ?: return null
        return inside.children
            .flatMap(::textsUnder)
            .firstOrNull { it.first == words }
            ?.second
    }

    /**
     * The visible texts a merged node of several texts holds, each as the unmerged tree's node of one text, in the
     * tree's order. Empty for a node of one text or none, and for a node whose unmerged texts do not read as its own.
     */
    private fun mergedTexts(
        node: SemanticsNode,
        unmerged: SemanticsNode,
    ): List<SemanticsNode> {
        val words = node.config.getOrNull(SemanticsProperties.Text)?.joinToString(" ") { it.text } ?: return emptyList()
        val parts = textsUnder(find(unmerged, node.id) ?: node)
        return if (parts.size > 1 && parts.joinToString(" ") { it.first } == words) parts.map { it.second } else emptyList()
    }

    /**
     * The node that draws the text [node] reads: [node] itself, or for a merged node of one text (a clickable row
     * of a label) the unmerged tree's text node, whose box and layout are the text's, not the row's.
     */
    private fun drawnText(
        node: SemanticsNode,
        unmerged: SemanticsNode,
    ): SemanticsNode {
        val words = node.config.getOrNull(SemanticsProperties.Text)?.joinToString(" ") { it.text } ?: return node
        val parts = textsUnder(find(unmerged, node.id) ?: node)
        return if (parts.size == 1 && parts.single().first == words) parts.single().second else node
    }

    /**
     * A text node's box as a browser reports a text's: vertically from the first line's ascent to the last line's
     * descent, inside the line boxes whose height only spaces the lines. Compose's node box is the line boxes.
     */
    private fun textBox(node: SemanticsNode): Rect {
        val box = node.wholeBox()
        val glyphs = glyphBox(node.config, box) ?: return box
        return Rect(box.left, glyphs.first, box.right, glyphs.second)
    }

    /**
     * The node's box from its position and size in the root. [SemanticsNode.boundsInRoot] clips the box to the root,
     * which cuts an element that reaches past the shot's frame.
     */
    private fun SemanticsNode.wholeBox(): Rect = if (layoutInfo.isPlaced) Rect(positionInRoot, size.toSize()) else Rect.Zero

    /** The node of [id] at or under [from]. */
    private fun find(
        from: SemanticsNode,
        id: Int,
    ): SemanticsNode? = if (from.id == id) from else from.children.firstNotNullOfOrNull { find(it, id) }

    /** The texts at or under [from] in the tree's order, each with the node that holds it. */
    private fun textsUnder(from: SemanticsNode): List<Pair<String, SemanticsNode>> {
        val own = from.config.getOrNull(SemanticsProperties.Text)?.joinToString(" ") { it.text }
        val held = if (own == null) emptyList() else listOf(own to from)
        return held + from.children.flatMap(::textsUnder)
    }

    /** From the first line's ascent to the last line's descent of the text [config] holds, from the box [bounds]. */
    private fun glyphBox(
        config: SemanticsConfiguration,
        bounds: Rect,
    ): Pair<Float, Float>? {
        val top = bounds.top
        val results = mutableListOf<TextLayoutResult>()
        val read = config.getOrNull(SemanticsActions.GetTextLayoutResult)?.action
        if (read == null || !read(results) || results.isEmpty()) return null
        val layout = results.first()
        val fontSize = layout.layoutInput.style.fontSize
        if (!fontSize.isSp) return null
        val size = with(layout.layoutInput.density) { fontSize.toPx() }
        val metrics = Paint().apply { textSize = size }.fontMetrics
        val first = top + layout.getLineBaseline(0) + metrics.ascent
        val last = top + layout.getLineBaseline(layout.lineCount - 1) + metrics.descent
        return first to last
    }

    /** JSON null for an absent value: [JSONObject.put] would drop the key. */
    private fun orNull(value: Any?): Any = value ?: JSONObject.NULL

    private fun kind(
        config: SemanticsConfiguration,
        role: Role?,
        editable: Boolean,
        text: Boolean,
        describedLeaf: Boolean,
    ): String =
        when {
            role == Role.Button -> {
                "button"
            }

            role == Role.Checkbox || role == Role.Switch -> {
                "toggle"
            }

            role == Role.RadioButton -> {
                "radio"
            }

            role == Role.Tab -> {
                "tab"
            }

            role == Role.Image -> {
                "image"
            }

            role == Role.DropdownList -> {
                "popup"
            }

            editable -> {
                "field"
            }

            config.contains(SemanticsProperties.ProgressBarRangeInfo) -> {
                if (config.contains(SemanticsActions.SetProgress)) "slider" else "progress"
            }

            text -> {
                "text"
            }

            // Compose gives an Image no role: a leaf that only has a content description, no words and no
            // click, is how a picture (a logo, a cover) reaches the tree.
            describedLeaf && !config.contains(SemanticsActions.OnClick) -> {
                "image"
            }

            else -> {
                "other"
            }
        }

    private fun native(
        editable: Boolean,
        text: Boolean,
        config: SemanticsConfiguration,
    ): String =
        when {
            editable -> "EditableText"
            config.contains(SemanticsProperties.ProgressBarRangeInfo) -> "ProgressBarRangeInfo"
            text -> "Text"
            else -> "Node"
        }

    private fun value(
        config: SemanticsConfiguration,
        field: CharSequence?,
    ): Any? =
        field?.toString()
            ?: when (config.getOrNull(SemanticsProperties.ToggleableState)) {
                ToggleableState.On -> "on"
                ToggleableState.Off -> "off"
                else -> null
            }

    /** The text's size in sp, weight on the 100 to 900 scale, and colour, from the layout it was drawn with. */
    private fun textStyle(config: SemanticsConfiguration): JSONObject? {
        val results = mutableListOf<TextLayoutResult>()
        val read = config.getOrNull(SemanticsActions.GetTextLayoutResult)?.action
        if (read == null || !read(results) || results.isEmpty()) return null
        val style = results.first().layoutInput.style
        val out = JSONObject()
        if (style.fontSize.isSp) out.put("size", style.fontSize.value.toDouble())
        style.fontWeight?.let { out.put("weight", it.weight) }
        if (style.color.isSpecified) out.put("color", hex(style.color.toArgb()))
        return out.takeIf { it.length() > 0 }
    }

    /** sRGB as `#RRGGBBAA` from an ARGB int. */
    private fun hex(argb: Int): String {
        val rrggbb = (argb and 0xFFFFFF).toString(HEX_RADIX).padStart(RGB_DIGITS - 2, '0')
        val alpha = ((argb ushr 24) and 0xFF).toString(HEX_RADIX).padStart(2, '0')
        return "#" + (rrggbb + alpha).uppercase()
    }

    private fun dp(
        pixels: Int,
        density: Float,
    ): Double = dp(pixels.toFloat(), density)

    private fun dp(
        pixels: Float,
        density: Float,
    ): Double = (pixels / density * TENTH).roundToInt() / TENTH
}

package builds.shots

import kotlin.math.roundToInt

private const val TENTH = 10.0
private const val HEX_RADIX = 16
private const val RGB_DIGITS = 6
private const val RGB_MASK = 0xFFFFFF
private const val ALPHA_SHIFT = 24

/** A number as an element list gives it, to the tenth. */
internal fun tenths(value: Float): Double = (value * TENTH).roundToInt() / TENTH

/** [pixels] in dp, as an element list gives a length. */
internal fun dp(
    pixels: Float,
    density: Float,
): Double = tenths(pixels / density)

/** sRGB as `#RRGGBBAA` from an ARGB int, as an element list gives a colour. */
internal fun hex(argb: Int): String {
    val rrggbb = (argb and RGB_MASK).toString(HEX_RADIX).padStart(RGB_DIGITS, '0')
    val alpha = (argb ushr ALPHA_SHIFT).toString(HEX_RADIX).padStart(2, '0')
    return "#" + (rrggbb + alpha).uppercase()
}

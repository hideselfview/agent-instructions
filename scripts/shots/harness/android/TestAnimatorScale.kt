package builds.shots

import android.animation.ValueAnimator
import org.robolectric.util.reflector.Direct
import org.robolectric.util.reflector.ForType
import org.robolectric.util.reflector.Reflector.reflector
import org.robolectric.util.reflector.Static

/** Changes the scale and notifies its listeners. Robolectric's shadow only sets the field. */
internal fun setTestAnimatorScale(scale: Float) = reflector(PlatformAnimatorScale::class.java).setDurationScale(scale)

@ForType(ValueAnimator::class)
internal interface PlatformAnimatorScale {
    @Direct
    @Static
    fun setDurationScale(scale: Float)
}

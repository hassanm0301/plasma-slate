package io.github.hassanm0301.plasmaslate.input

import android.view.MotionEvent
import io.github.hassanm0301.plasmaslate.proto.TouchEvent
import io.github.hassanm0301.plasmaslate.proto.TouchPhase
import io.github.hassanm0301.plasmaslate.proto.TouchPoint

/**
 * Maps view coordinates to normalized picture coordinates for a picture of
 * [videoWidth]×[videoHeight] shown "fit centre" in a [viewWidth]×[viewHeight] view.
 */
class TouchMapper(
    private val viewWidth: Int,
    private val viewHeight: Int,
    private val videoWidth: Int,
    private val videoHeight: Int,
) {
    private val scale: Float = minOf(viewWidth.toFloat() / videoWidth, viewHeight.toFloat() / videoHeight)
    private val drawW = videoWidth * scale
    private val drawH = videoHeight * scale
    private val offX = (viewWidth - drawW) / 2f
    private val offY = (viewHeight - drawH) / 2f

    /** Normalized (0..1, clamped) position of a view point. */
    fun normalize(x: Float, y: Float): Pair<Float, Float> {
        val nx = ((x - offX) / drawW).coerceIn(0f, 1f)
        val ny = ((y - offY) / drawH).coerceIn(0f, 1f)
        return nx to ny
    }

    /** One protocol event per MotionEvent; MOVE reports every active pointer. */
    fun convert(ev: MotionEvent, timeUs: Long): TouchEvent? {
        val b = TouchEvent.newBuilder().setTimeUs(timeUs)
        fun point(index: Int, phase: TouchPhase) {
            val (nx, ny) = normalize(ev.getX(index), ev.getY(index))
            b.addPoints(TouchPoint.newBuilder().setId(ev.getPointerId(index)).setPhase(phase).setX(nx).setY(ny))
        }
        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> point(ev.actionIndex, TouchPhase.TOUCH_PHASE_DOWN)
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> point(ev.actionIndex, TouchPhase.TOUCH_PHASE_UP)
            MotionEvent.ACTION_MOVE -> for (i in 0 until ev.pointerCount) point(i, TouchPhase.TOUCH_PHASE_MOVE)
            MotionEvent.ACTION_CANCEL -> for (i in 0 until ev.pointerCount) point(i, TouchPhase.TOUCH_PHASE_CANCEL)
            else -> return null
        }
        return b.build()
    }
}

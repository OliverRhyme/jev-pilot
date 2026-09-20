package dev.jevpilot.helper

import android.accessibilityservice.AccessibilityService
import android.content.Context
import android.os.Build
import android.util.DisplayMetrics
import android.view.Display
import android.view.Surface
import android.view.WindowManager

/**
 * Screen size and rotation, read through whichever API the runtime offers.
 *
 * Every accessor here is either deprecated on new releases or absent on old
 * ones, and each can throw on a particular vendor build, so the value is
 * narrowed in stages: resource metrics first, then the real window bounds,
 * and each stage keeps the previous answer if it fails. Reporting the wrong
 * size silently mis-scales every gesture, so a plausible fallback beats an
 * exception.
 */
object DisplayUtils {

    /** Rotation as UIAutomator reports it: 0, 1, 2 or 3 quarter turns. */
    data class DisplayInfo(val rotation: Int, val width: Int, val height: Int)

    @Suppress("DEPRECATION")
    fun getDisplayInfo(service: AccessibilityService): DisplayInfo {
        var rotation = 0
        var width = 1080
        var height = 2400

        // Baseline: resource metrics cover tablets, TVs and emulators alike.
        runCatching {
            val metrics = service.resources.displayMetrics
            if (metrics.widthPixels > 0 && metrics.heightPixels > 0) {
                width = metrics.widthPixels
                height = metrics.heightPixels
            }
        }

        // API 30+: the full physical bounds, including the area under the bars.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            runCatching {
                val manager = service.getSystemService(Context.WINDOW_SERVICE) as? WindowManager
                val bounds = manager?.maximumWindowMetrics?.bounds
                if (bounds != null && bounds.width() > 0 && bounds.height() > 0) {
                    width = bounds.width()
                    height = bounds.height()
                }
            }
        }

        val display: Display? = runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) service.display else null
        }.getOrNull() ?: runCatching {
            (service.getSystemService(Context.WINDOW_SERVICE) as? WindowManager)?.defaultDisplay
        }.getOrNull()

        if (display != null) {
            rotation = when (display.rotation) {
                Surface.ROTATION_90 -> 1
                Surface.ROTATION_180 -> 2
                Surface.ROTATION_270 -> 3
                else -> 0
            }
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
                runCatching {
                    val metrics = DisplayMetrics()
                    display.getRealMetrics(metrics)
                    width = metrics.widthPixels
                    height = metrics.heightPixels
                }
            }
        }

        return DisplayInfo(rotation, width, height)
    }
}

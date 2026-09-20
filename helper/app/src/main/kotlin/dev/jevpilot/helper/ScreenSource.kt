package dev.jevpilot.helper

import android.accessibilityservice.AccessibilityService
import android.app.UiAutomation
import android.content.Context
import android.graphics.Bitmap
import android.os.Build
import android.util.Log
import android.view.Display
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

/**
 * Where a reading of the screen comes from.
 *
 * Two things can answer, and which one does changes both what can be read and
 * what it costs:
 *
 * * **An accessibility service** ([ServiceSource]) is always there, costs
 *   nothing to keep running, and is told when windows change. It is also
 *   refused the content of some windows — Settings' Wi-Fi panel among them —
 *   which the platform still lists as present and focused.
 * * **A `UiAutomation`** ([AutomationSource]) is privileged and refused
 *   nothing, but exists only while an instrumentation runs, and by default
 *   unbinds every accessibility service for as long as it lives.
 *   `FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES` is what stops that, and is why
 *   both can serve one device at once.
 */
interface ScreenSource {
    /** Every window, or an empty list when none can be enumerated. */
    fun windows(): List<AccessibilityWindowInfo>

    /** The root of the active window, if there is one. */
    fun activeRoot(): AccessibilityNodeInfo?

    /** The node holding the given kind of focus, if any. */
    fun findFocus(focusType: Int): AccessibilityNodeInfo?

    /** A screenshot, or null when this source cannot take one. */
    fun screenshot(): Bitmap?

    /** A context, for reading the display's size and rotation. */
    val context: Context

    /** The package on screen, as far as this source knows. */
    val currentPackage: String

    /** The activity on screen, as far as this source knows. */
    val currentActivity: String

    /** What this source is, reported by `/ping`. */
    val label: String
}

/** Reading through the accessibility service. */
class ServiceSource(private val service: PilotAccessibilityService) : ScreenSource {

    override fun windows(): List<AccessibilityWindowInfo> =
        runCatching { service.windows.orEmpty() }.getOrDefault(emptyList())

    override fun activeRoot(): AccessibilityNodeInfo? =
        runCatching { service.rootInActiveWindow }.getOrNull()

    override fun findFocus(focusType: Int): AccessibilityNodeInfo? =
        runCatching { service.findFocus(focusType) }.getOrNull()

    override val context: Context get() = service
    override val currentPackage: String get() = service.currentPackageName
    override val currentActivity: String get() = service.currentActivityName
    override val label: String get() = "PilotAccessibilityService"

    /**
     * A screenshot through the accessibility API, which answers on a callback
     * and is rate-limited to roughly one per 333ms.
     */
    override fun screenshot(): Bitmap? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return null
        val bitmap = AtomicReference<Bitmap?>(null)
        val error = AtomicInteger(0)
        val latch = CountDownLatch(1)
        request(bitmap, error, latch, allowRetry = true)
        runCatching { latch.await(2_500L, TimeUnit.MILLISECONDS) }
        return bitmap.get()
    }

    private fun request(
        bitmapRef: AtomicReference<Bitmap?>,
        errorRef: AtomicInteger,
        latch: CountDownLatch,
        allowRetry: Boolean,
    ) {
        try {
            service.takeScreenshot(
                Display.DEFAULT_DISPLAY,
                executor,
                object : AccessibilityService.TakeScreenshotCallback {
                    override fun onSuccess(result: AccessibilityService.ScreenshotResult) {
                        try {
                            val buffer = result.hardwareBuffer
                            val hardware = Bitmap.wrapHardwareBuffer(buffer, result.colorSpace)
                            if (hardware != null) {
                                // A hardware bitmap cannot be compressed
                                // directly, and its buffer must be released.
                                bitmapRef.set(hardware.copy(Bitmap.Config.ARGB_8888, false))
                                hardware.recycle()
                            }
                            buffer.close()
                        } catch (t: Throwable) {
                            Log.w(TAG, "Error copying screenshot buffer", t)
                        } finally {
                            latch.countDown()
                        }
                    }

                    override fun onFailure(errorCode: Int) {
                        errorRef.set(errorCode)
                        if (allowRetry && errorCode == RATE_LIMITED) {
                            executor.execute {
                                runCatching { Thread.sleep(RETRY_MS) }
                                request(bitmapRef, errorRef, latch, allowRetry = false)
                            }
                            return
                        }
                        Log.w(TAG, "takeScreenshot failed, errorCode: $errorCode")
                        latch.countDown()
                    }
                },
            )
        } catch (t: Throwable) {
            Log.w(TAG, "takeScreenshot invocation error", t)
            latch.countDown()
        }
    }

    private companion object {
        const val TAG = "JevPilotServiceSource"

        /** `ERROR_TAKE_SCREENSHOT_INTERVAL_TIME_SHORT`. */
        const val RATE_LIMITED = 3
        const val RETRY_MS = 350L
        val executor = Executors.newSingleThreadExecutor()
    }
}

/**
 * Reading through a `UiAutomation` held by a running instrumentation.
 *
 * Refused nothing, and pays no JVM start per reading: `uiautomator dump`
 * spends about 1.3s of its 2.5s getting a runtime up, measured on a Pixel,
 * because it starts one for every invocation.
 */
class AutomationSource(
    private val automation: UiAutomation,
    override val context: Context,
) : ScreenSource {

    override fun windows(): List<AccessibilityWindowInfo> =
        runCatching { automation.windows.orEmpty() }.getOrDefault(emptyList())

    override fun activeRoot(): AccessibilityNodeInfo? =
        runCatching { automation.rootInActiveWindow }.getOrNull()

    override fun findFocus(focusType: Int): AccessibilityNodeInfo? =
        runCatching { automation.findFocus(focusType) }.getOrNull()

    override fun screenshot(): Bitmap? = runCatching { automation.takeScreenshot() }.getOrNull()

    override val currentPackage: String
        get() = runCatching { activeRoot()?.packageName?.toString().orEmpty() }.getOrDefault("")

    override val currentActivity: String get() = ""

    override val label: String get() = "PilotInstrumentation"

    /**
     * Let the screen go quiet before reading it.
     *
     * `uiautomator dump` waits a full second for that and offers no way to
     * relax it. Here the window is ours: short enough not to catch a frame
     * mid-draw, and short enough not to wait out an animation that may never
     * stop.
     */
    fun waitForIdle(idleMs: Long, totalMs: Long) {
        runCatching { automation.waitForIdle(idleMs, totalMs) }
    }
}

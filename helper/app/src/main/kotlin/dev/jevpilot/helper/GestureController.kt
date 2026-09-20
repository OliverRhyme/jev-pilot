package dev.jevpilot.helper

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Path
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.accessibility.AccessibilityNodeInfo
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Gestures and actions through the accessibility APIs, without UiAutomation or
 * root.
 *
 * Every entry point blocks until the system reports the gesture finished, so a
 * caller that returns has a screen that has already moved. `dispatchGesture`
 * must be called from the main looper and answers on a callback, which is what
 * the latch here bridges.
 */
object GestureController {

    private const val TAG = "JevPilotGestureCtrl"
    private val mainHandler = Handler(Looper.getMainLooper())

    /** A tap is a stroke that goes nowhere, held briefly. */
    fun tap(service: AccessibilityService, x: Float, y: Float, timeoutMs: Long): Boolean =
        dispatchSynchronous(service, stroke(Path().apply { moveTo(x, y) }, 60L), timeoutMs)

    fun doubleTap(service: AccessibilityService, x: Float, y: Float, timeoutMs: Long): Boolean {
        if (!tap(service, x, y, timeoutMs)) return false
        runCatching { Thread.sleep(100L) }
        return tap(service, x, y, timeoutMs)
    }

    fun longPress(
        service: AccessibilityService,
        x: Float,
        y: Float,
        durationMs: Long,
        timeoutMs: Long,
    ): Boolean {
        // Below the system's long-press threshold the gesture lands as a tap.
        val held = durationMs.coerceIn(500L, 5_000L)
        return dispatchSynchronous(
            service,
            stroke(Path().apply { moveTo(x, y) }, held),
            timeoutMs + held,
        )
    }

    fun swipe(
        service: AccessibilityService,
        x1: Float,
        y1: Float,
        x2: Float,
        y2: Float,
        durationMs: Long,
        timeoutMs: Long,
    ): Boolean {
        val travel = durationMs.coerceIn(50L, 5_000L)
        val path = Path().apply {
            moveTo(x1, y1)
            lineTo(x2, y2)
        }
        return dispatchSynchronous(service, stroke(path, travel), timeoutMs + travel)
    }

    /**
     * Sets, or with [append] extends, the text of the focused or first editable
     * field.
     *
     * `ACTION_SET_TEXT` replaces the whole content, where typing through an IME
     * adds to it, so appending is what reproduces the caller's expectation.
     * A field showing hint text holds no text, and treating the hint as existing
     * content would prepend the placeholder to what was typed.
     */
    @JvmOverloads
    fun setText(service: AccessibilityService, text: String?, append: Boolean = false): Boolean {
        val inputNode = HierarchyDumper.findInputNode(service)
        if (inputNode == null) {
            Log.w(TAG, "No editable/focused input node found for setText")
            return false
        }
        return try {
            var value = text.orEmpty()
            if (append) {
                val showingHint = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                    runCatching { inputNode.isShowingHintText }.getOrDefault(false)
                val existing = inputNode.text
                if (!showingHint && !existing.isNullOrEmpty()) {
                    value = existing.toString() + value
                }
            }
            val args = Bundle().apply {
                putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, value)
            }
            inputNode.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
        } catch (t: Throwable) {
            Log.w(TAG, "performAction ACTION_SET_TEXT failed", t)
            false
        } finally {
            HierarchyDumper.safeRecycle(inputNode)
        }
    }

    fun clearText(service: AccessibilityService): Boolean = setText(service, "")

    /** Maps a name to one of the system-wide actions, or refuses it. */
    fun performGlobalAction(service: AccessibilityService, actionName: String?): Boolean {
        val actionId = when (actionName?.lowercase()) {
            "back" -> AccessibilityService.GLOBAL_ACTION_BACK
            "home" -> AccessibilityService.GLOBAL_ACTION_HOME
            "recents" -> AccessibilityService.GLOBAL_ACTION_RECENTS
            "notifications" -> AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS
            "quick_settings" -> AccessibilityService.GLOBAL_ACTION_QUICK_SETTINGS
            "power_dialog" -> AccessibilityService.GLOBAL_ACTION_POWER_DIALOG
            "toggle_split_screen" -> AccessibilityService.GLOBAL_ACTION_TOGGLE_SPLIT_SCREEN
            "lock_screen" ->
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                    AccessibilityService.GLOBAL_ACTION_LOCK_SCREEN
                } else {
                    return false
                }
            "take_screenshot" ->
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                    AccessibilityService.GLOBAL_ACTION_TAKE_SCREENSHOT
                } else {
                    return false
                }
            else -> return false
        }
        return service.performGlobalAction(actionId)
    }

    private fun stroke(path: Path, durationMs: Long): GestureDescription =
        GestureDescription.Builder()
            .addStroke(GestureDescription.StrokeDescription(path, 0L, durationMs))
            .build()

    private fun dispatchSynchronous(
        service: AccessibilityService,
        gesture: GestureDescription,
        timeoutMs: Long,
    ): Boolean {
        val latch = CountDownLatch(1)
        val result = AtomicBoolean(false)

        mainHandler.post {
            try {
                service.dispatchGesture(
                    gesture,
                    object : AccessibilityService.GestureResultCallback() {
                        override fun onCompleted(gestureDescription: GestureDescription?) {
                            result.set(true)
                            latch.countDown()
                        }

                        override fun onCancelled(gestureDescription: GestureDescription?) {
                            result.set(false)
                            latch.countDown()
                        }
                    },
                    null,
                )
            } catch (t: Throwable) {
                Log.w(TAG, "dispatchGesture threw exception", t)
                result.set(false)
                latch.countDown()
            }
        }

        return try {
            latch.await(timeoutMs, TimeUnit.MILLISECONDS) && result.get()
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            false
        }
    }

    /**
     * Writes to the system clipboard.
     *
     * Clipboard *writes* are allowed from any app on every API level — only
     * reads were restricted in Android 10 — which makes this the IME-free route
     * for multiline and non-ASCII input: set the clip here, then `KEYCODE_PASTE`
     * over adb.
     */
    fun setClipboard(service: AccessibilityService, text: String?): Boolean {
        val latch = CountDownLatch(1)
        val ok = AtomicBoolean(false)
        mainHandler.post {
            try {
                val clipboard =
                    service.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                if (clipboard != null) {
                    clipboard.setPrimaryClip(ClipData.newPlainText("jev-pilot", text.orEmpty()))
                    ok.set(true)
                }
            } catch (t: Throwable) {
                Log.w(TAG, "setClipboard failed: ${t.message}")
            } finally {
                latch.countDown()
            }
        }
        try {
            latch.await(2_000L, TimeUnit.MILLISECONDS)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        }
        return ok.get()
    }
}

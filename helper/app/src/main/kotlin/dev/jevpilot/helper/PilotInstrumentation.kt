package dev.jevpilot.helper

import android.app.Instrumentation
import android.app.UiAutomation
import android.os.Bundle
import android.util.Log

/**
 * A long-running instrumentation that serves the screen through `UiAutomation`.
 *
 * Started with:
 *
 * ```
 * adb shell am instrument -w dev.jevpilot.helper/.PilotInstrumentation
 * ```
 *
 * # Why this exists alongside the accessibility service
 *
 * The service cannot read every screen. Some windows are simply not served to
 * an accessibility service — Settings' Wi-Fi panel among them — while the
 * platform lists them as present and focused. `UiAutomation` is privileged and
 * is refused none of them.
 *
 * The obvious way to reach one, `uiautomator dump`, costs about 2.5s, and
 * about 1.3s of that is a JVM starting: measured on a Pixel, `uiautomator`
 * with no arguments at all costs the same 1.3s. Holding the runtime open pays
 * that once for a session rather than once per reading.
 *
 * # Why it does not silence the service
 *
 * `UiAutomation` unbinds every accessibility service for as long as it lives,
 * which is why a `uiautomator dump` makes the helper stop answering for about
 * 1.5s. [`FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES`][flag] is the platform's
 * own opt-out, and taking it is what lets both serve one device at once — the
 * service for events and gestures, this for the screens it cannot see.
 *
 * It reads and does not act. Gestures stay with the service, which is bound
 * the whole time and needs no process started to reach it.
 *
 * [flag]: android.app.UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES
 */
class PilotInstrumentation : Instrumentation() {

    private var server: CommandServer? = null

    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        // `start()` gives this a thread of its own; `onStart` then runs there.
        start()
    }

    override fun onStart() {
        super.onStart()
        val automation: UiAutomation = try {
            getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        } catch (t: Throwable) {
            Log.e(TAG, "Could not obtain UiAutomation", t)
            finish(1, Bundle())
            return
        }

        val source = AutomationSource(automation, context)
        server = CommandServer(source, DEFAULT_PORT).apply {
            isDaemon = false
            start()
        }
        Log.i(TAG, "Reading server started on 127.0.0.1:$DEFAULT_PORT")

        // `am instrument` ends when this returns, and the server dies with the
        // process, so this stays parked until the process is stopped.
        while (!Thread.currentThread().isInterrupted) {
            try {
                Thread.sleep(PARK_MS)
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
            }
        }

        server?.shutdown()
        finish(0, Bundle())
    }

    private companion object {
        const val TAG = "JevPilotInstrumentation"

        /**
         * A port of its own, so this and the accessibility service can both be
         * running and a caller can say which it wants.
         */
        const val DEFAULT_PORT = 18878

        const val PARK_MS = 60_000L
    }
}

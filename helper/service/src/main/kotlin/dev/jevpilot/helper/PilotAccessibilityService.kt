package dev.jevpilot.helper

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.AccessibilityServiceInfo
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.util.Log
import android.view.accessibility.AccessibilityEvent

/**
 * A non-exclusive accessibility service that serves the screen over loopback.
 *
 * Being an accessibility service rather than a `UiAutomation` client is the
 * whole point: Android unbinds every accessibility service while a
 * `UiAutomation` connection is alive, so `uiautomator dump`, Appium and
 * Espresso each silence this one for as long as they run — but this one never
 * silences them, and it costs no such connection of its own.
 *
 * Only window-state changes are subscribed to. That is the single event used,
 * to remember the foreground package and activity, and a wider subscription
 * costs CPU and battery on a device sitting idle between runs.
 */
class PilotAccessibilityService : AccessibilityService() {

    /**
     * When any accessibility event last arrived, on the uptime clock.
     *
     * Started at the moment the service connects rather than at -1: "nothing
     * has changed since I began watching" is a true and useful answer, and
     * withholding it would leave the first steps of every run with no
     * account of whether the screen had settled.
     */
    @Volatile
    var lastEventAtMs: Long = -1L
        private set

    @Volatile
    var currentPackageName: String = ""
        private set

    @Volatile
    var currentActivityName: String = ""
        private set

    private var server: CommandServer? = null

    /** Installed versionCode, or -1 when the package manager cannot answer. */
    @Suppress("DEPRECATION")
    val versionCode: Long
        get() = runCatching {
            val info = packageManager.getPackageInfo(packageName, 0)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                info.longVersionCode
            } else {
                info.versionCode.toLong()
            }
        }.getOrDefault(-1L)

    val versionName: String
        get() = runCatching {
            packageManager.getPackageInfo(packageName, 0).versionName.orEmpty()
        }.getOrDefault("")

    override fun onServiceConnected() {
        super.onServiceConnected()
        instance = this
        lastEventAtMs = android.os.SystemClock.uptimeMillis()
        Log.i(TAG, "PilotAccessibilityService connected")

        // A foreground notification is what keeps the service alive on ROMs
        // that kill background work aggressively.
        startForegroundNotification()

        // The manifest's config is a request; some OEM builds do not honour all
        // of it, so the flags that matter are re-asserted here.
        runCatching {
            val info = serviceInfo ?: AccessibilityServiceInfo()
            // Content-changed matters as much as state-changed, and not for
            // the events themselves: the framework's node cache is invalidated
            // by them. Subscribing to state changes alone left a screen that
            // navigated *within* one window — which is every screen of a
            // Flutter app — serving the tree of a screen already left, while
            // `uiautomator` showed the real one. Taps then looked like they
            // did nothing, and a loop watching for the screen to change saw it
            // never change.
            info.eventTypes = AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED or
                AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED or
                AccessibilityEvent.TYPE_WINDOWS_CHANGED or
                AccessibilityEvent.TYPE_VIEW_SCROLLED
            info.feedbackType = AccessibilityServiceInfo.FEEDBACK_GENERIC
            info.notificationTimeout = 100
            info.flags = info.flags or
                AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS or
                AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS
            // Not-important views are what UIAutomator's compressed dump leaves
            // out; keeping them out here too is what makes the two backends
            // describe the same tree.
            info.flags = info.flags and
                AccessibilityServiceInfo.FLAG_INCLUDE_NOT_IMPORTANT_VIEWS.inv()
            serviceInfo = info
            Log.i(TAG, "AccessibilityServiceInfo flags enforced")
        }.onFailure { Log.w(TAG, "Failed to configure AccessibilityServiceInfo", it) }

        server?.shutdown()
        val source = ServiceSource(
            this,
            foreground = { currentPackageName to currentActivityName },
            lastEventAt = { lastEventAtMs },
        )
        server = CommandServer(source, DEFAULT_PORT, gestures = this).apply {
            isDaemon = true
            start()
        }
        Log.i(TAG, "CommandServer started on port $DEFAULT_PORT")
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    @Suppress("DEPRECATION")
    private fun startForegroundNotification() {
        try {
            val manager =
                getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager ?: return

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                // A channel's importance is frozen once it exists, so a quieter
                // channel needs a new id and the old one is removed.
                runCatching { manager.deleteNotificationChannel(LEGACY_CHANNEL_ID) }
                val channel = NotificationChannel(
                    CHANNEL_ID,
                    "Jev Pilot helper",
                    NotificationManager.IMPORTANCE_MIN,
                ).apply {
                    description = "Shown while the Jev Pilot helper is installed on this device"
                    setShowBadge(false)
                }
                manager.createNotificationChannel(channel)
            }

            val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Notification.Builder(this, CHANNEL_ID)
            } else {
                Notification.Builder(this)
            }
            builder.setContentTitle("Jev Pilot helper is running")
                .setContentText("Lets Jev Pilot read this screen during automated tests.")
                .setStyle(
                    Notification.BigTextStyle().bigText(
                        "Installed by Jev Pilot to read the screen layout during automated " +
                            "tests. It answers only on this device (127.0.0.1:$DEFAULT_PORT), " +
                            "only to the computer connected over USB debugging that holds the " +
                            "session token, and it sends nothing anywhere. Remove it any time " +
                            "from Settings > Apps."
                    )
                )
                .setSmallIcon(android.R.drawable.stat_notify_sync)
                .setOngoing(true)
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                builder.setPriority(Notification.PRIORITY_MIN)
            }

            when {
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE ->
                    startForeground(
                        NOTIFICATION_ID,
                        builder.build(),
                        ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
                    )
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q ->
                    startForeground(NOTIFICATION_ID, builder.build(), 0)
                else -> startForeground(NOTIFICATION_ID, builder.build())
            }
            Log.i(TAG, "Foreground keep-alive notification active")
        } catch (t: Throwable) {
            Log.w(TAG, "Foreground notification start skipped or deferred: ${t.message}")
        }
    }

    @Suppress("DEPRECATION")
    private fun stopForegroundNotification() {
        runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                stopForeground(STOP_FOREGROUND_REMOVE)
            } else {
                stopForeground(true)
            }
        }
    }

    /**
     * Only window-state changes say what is in front; the rest are subscribed
     * to for their effect on the framework's node cache rather than for
     * anything they carry.
     */
    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event == null) return
        // Every event, whatever kind. A screen still being drawn emits them
        // and a settled screen does not, so the time since the last one is
        // the device's own answer to "has it finished?" — which a caller
        // polling the tree from outside cannot work out.
        lastEventAtMs = android.os.SystemClock.uptimeMillis()
        if (event.eventType != AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) return
        event.packageName?.let { currentPackageName = it.toString() }
        event.className?.let { currentActivityName = it.toString() }
    }

    override fun onInterrupt() {
        Log.w(TAG, "PilotAccessibilityService interrupted")
    }

    override fun onDestroy() {
        super.onDestroy()
        Log.i(TAG, "PilotAccessibilityService destroyed")
        stopForegroundNotification()
        server?.shutdown()
        server = null
        if (instance === this) instance = null
    }

    companion object {
        private const val TAG = "JevPilotA11yService"

        /** Loopback port on the device. Distinct from other helpers' ports. */
        const val DEFAULT_PORT = 18877

        /**
         * Bumped whenever the HTTP contract changes in a way an older host
         * cannot use. 2: token authentication, byte-accurate bodies,
         * visible-only dumps, `fields=`.
         */
        const val PROTOCOL_VERSION = 2

        private const val LEGACY_CHANNEL_ID = "artemis_helper_channel"
        private const val CHANNEL_ID = "jev_pilot_helper_quiet"
        private const val NOTIFICATION_ID = 18877

        @Volatile
        @JvmStatic
        var instance: PilotAccessibilityService? = null
            private set
    }
}

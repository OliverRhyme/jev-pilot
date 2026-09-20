package dev.jevpilot.helper

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Receives the host's session token.
 *
 * The receiver is guarded in the manifest by `WRITE_SECURE_SETTINGS`, which
 * ordinary apps cannot hold and the adb shell user does, so only something with
 * adb access to the device can set or clear the token:
 *
 * ```
 * adb shell am broadcast -n <package>/dev.jevpilot.helper.TokenReceiver \
 *     -a <package>.SET_TOKEN --es token <hex>
 * ```
 */
class TokenReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context?, intent: Intent?) {
        // The action is the receiving package's own, so the service and the
        // reader are handed tokens separately and neither can be given the
        // other's by a broadcast meant for it.
        if (context == null || intent?.action != "${context.packageName}.SET_TOKEN") return
        TokenStore.set(intent.getStringExtra(EXTRA_TOKEN))
        Log.i(TAG, if (TokenStore.isSet) "Session token set" else "Session token cleared")
    }

    companion object {
        private const val TAG = "JevPilotTokenReceiver"
        const val EXTRA_TOKEN = "token"
    }
}

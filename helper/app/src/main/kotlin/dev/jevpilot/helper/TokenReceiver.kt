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
 * adb shell am broadcast -n dev.jevpilot.helper/.TokenReceiver \
 *     -a dev.jevpilot.helper.SET_TOKEN --es token <hex>
 * ```
 */
class TokenReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context?, intent: Intent?) {
        if (intent?.action != ACTION_SET_TOKEN) return
        TokenStore.set(intent.getStringExtra(EXTRA_TOKEN))
        Log.i(TAG, if (TokenStore.isSet) "Session token set" else "Session token cleared")
    }

    companion object {
        private const val TAG = "JevPilotTokenReceiver"
        const val ACTION_SET_TOKEN = "dev.jevpilot.helper.SET_TOKEN"
        const val EXTRA_TOKEN = "token"
    }
}

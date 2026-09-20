package dev.jevpilot.helper

import java.security.MessageDigest

/**
 * The session token the host pushed, for this process lifetime.
 *
 * The loopback server is reachable by every app on the device, so requests are
 * served only when they carry this token. It lives in memory alone: a killed or
 * re-bound service starts without one and answers 401 until the host pushes it
 * again, which the host does on every attach and on every 401.
 */
object TokenStore {

    @Volatile
    private var token: String? = null

    fun set(value: String?) {
        token = value?.takeUnless(String::isEmpty)
    }

    val isSet: Boolean
        get() = token != null

    /** Constant time, so a local app cannot guess the token byte by byte. */
    fun matches(candidate: String?): Boolean {
        val current = token ?: return false
        if (candidate == null) return false
        return MessageDigest.isEqual(current.toByteArray(), candidate.toByteArray())
    }
}

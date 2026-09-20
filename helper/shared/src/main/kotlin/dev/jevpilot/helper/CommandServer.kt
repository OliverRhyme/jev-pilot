package dev.jevpilot.helper

import android.accessibilityservice.AccessibilityService
import android.os.Build
import android.util.Log
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.URLDecoder
import java.util.Locale
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/**
 * The loopback command server, bound to `127.0.0.1` alone.
 *
 * Two protocols share the port: HTTP (`/ping`, `/dump`, `/dump_xml`,
 * `/hierarchy.xml`, `/snapshot`, `/action`) and line-delimited JSON-RPC over a
 * raw socket.
 *
 * ## What this server is
 *
 * A full-screen reader and a gesture injector, reachable by every app on the
 * device, because loopback has no access control of its own. Everything below
 * follows from that:
 *
 * * **Everything except `/ping` requires the session token** the host pushed
 *   through [TokenReceiver], which only a sender holding `WRITE_SECURE_SETTINGS`
 *   can do. No token, no answer — including before the host has pushed one.
 *   The comparison is constant time ([TokenStore.matches]).
 * * **The peer must be loopback.** Checked on the accepted socket rather than
 *   trusted from the bind, so a mis-bind cannot silently expose the device.
 * * **A request carrying `Origin` is refused.** That is a page in some app's
 *   WebView talking to us, never the host, and refusing it closes the
 *   DNS-rebinding and CSRF routes onto this port.
 * * **Nothing from the request is echoed back.** An unknown path or command
 *   gets a constant message, and an internal failure is logged rather than
 *   described, so the port cannot be used to probe or to reflect a payload.
 * * **Every resource is bounded**: header line length, header count, body
 *   size, connection count and read timeout. A local app that opens sockets in
 *   a loop gets rejections, not an out-of-memory service.
 *
 * Request bodies are read as bytes, since `Content-Length` counts bytes, so
 * UTF-8 payloads in any script survive exactly.
 */
class CommandServer(
    private val source: ScreenSource,
    private val port: Int,
    /**
     * Gestures, when this server can perform them. An instrumentation reads
     * the screen and does not act on it: acting stays with the accessibility
     * service, which is bound the whole time and needs no process started.
     */
    private val gestures: AccessibilityService? = null,
) : Thread("JevPilotCommandServer") {

    @Volatile
    private var isRunning = true
    private var serverSocket: ServerSocket? = null

    /**
     * Bounded on purpose: a thread per connection with no ceiling is a local
     * denial of service that needs nothing but a loop around `connect()`.
     */
    private val clientExecutor = ThreadPoolExecutor(
        2,
        MAX_CONCURRENT_CLIENTS,
        30L,
        TimeUnit.SECONDS,
        ArrayBlockingQueue(MAX_QUEUED_CLIENTS),
        ThreadPoolExecutor.AbortPolicy(),
    )

    override fun run() {
        try {
            serverSocket = ServerSocket().apply {
                reuseAddress = true
                bind(InetSocketAddress(InetAddress.getByName("127.0.0.1"), port), BACKLOG)
            }
            Log.i(TAG, "CommandServer listening on 127.0.0.1:$port")

            while (isRunning) {
                val socket = try {
                    serverSocket?.accept() ?: break
                } catch (_: Exception) {
                    if (!isRunning) break
                    continue
                }
                // The bind is to loopback, so this should be impossible. It is
                // checked anyway: if it ever is not, every other control here
                // is guarding a door that is already open.
                if (socket.inetAddress?.isLoopbackAddress != true) {
                    Log.w(TAG, "Refused non-loopback peer")
                    closeQuietly(socket)
                    continue
                }
                try {
                    clientExecutor.execute { handleClient(socket) }
                } catch (_: RejectedExecutionException) {
                    closeQuietly(socket)
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "Server error", e)
        } finally {
            closeQuietly(serverSocket)
        }
    }

    // ---------------------------------------------------------------- //
    // Byte-accurate request reading
    // ---------------------------------------------------------------- //

    private fun handleClient(socket: Socket) {
        try {
            socket.soTimeout = READ_TIMEOUT_MS
            socket.tcpNoDelay = true
            val input = BufferedInputStream(socket.getInputStream())
            val output = socket.getOutputStream()

            val firstLine = readLineBytes(input) ?: return
            val ascii = String(firstLine, Charsets.ISO_8859_1).trim()

            when {
                ascii.startsWith("GET ") || ascii.startsWith("POST ") ->
                    handleHttp(ascii, input, output)
                ascii.startsWith("{") ->
                    handleJsonRpc(String(firstLine, Charsets.UTF_8).trim(), output)
                else ->
                    sendJson(output, 400, errorJson("Unsupported protocol").toString())
            }
        } catch (e: Exception) {
            Log.w(TAG, "Client handling error: ${e.javaClass.simpleName}")
        } finally {
            closeQuietly(socket)
        }
    }

    // ---------------------------------------------------------------- //
    // HTTP
    // ---------------------------------------------------------------- //

    private fun handleHttp(requestLine: String, input: InputStream, output: OutputStream) {
        try {
            val target = requestLine.split(" ").getOrNull(1) ?: "/"
            val separator = target.indexOf('?')
            val path = if (separator >= 0) target.substring(0, separator) else target
            val query =
                if (separator >= 0) parseQuery(target.substring(separator + 1)) else emptyMap()

            var contentLength = 0
            var headerToken: String? = null
            var sawOrigin = false
            var headers = 0

            while (true) {
                val lineBytes = readLineBytes(input) ?: break
                if (lineBytes.isEmpty()) break
                if (++headers > MAX_HEADERS) {
                    sendJson(output, 431, errorJson("Too many headers").toString())
                    return
                }
                val line = String(lineBytes, Charsets.ISO_8859_1)
                val colon = line.indexOf(':')
                if (colon < 0) continue
                val name = line.substring(0, colon).trim().lowercase(Locale.ROOT)
                val value = line.substring(colon + 1).trim()
                when (name) {
                    "content-length" -> contentLength = value.toIntOrNull() ?: 0
                    TOKEN_HEADER -> headerToken = value
                    "origin" -> sawOrigin = true
                }
            }

            // Only a browser sends Origin. The host never does, so this is a
            // page in some app's WebView reaching for the port.
            if (sawOrigin) {
                sendJson(output, 403, errorJson("Cross-origin requests are refused").toString())
                return
            }
            if (contentLength < 0 || contentLength > MAX_BODY) {
                sendJson(output, 413, errorJson("Body too large").toString())
                return
            }

            val body =
                if (contentLength > 0) String(readExactly(input, contentLength), Charsets.UTF_8)
                else ""

            val authed = TokenStore.matches(headerToken ?: query["token"])

            if (path == "/ping" || path == "/") {
                sendJson(output, 200, buildPing(authed).toString())
                return
            }
            if (!authed) {
                // A token is 32 random bytes, so this is not a real brute-force
                // defence; it is here so a loop of attempts costs the caller
                // something and shows up as latency rather than as free tries.
                sleepQuietly(AUTH_FAILURE_DELAY_MS)
                sendJson(output, 401, unauthorizedJson().toString())
                return
            }

            when {
                path == "/snapshot" -> {
                    val options = HierarchyDumper.DumpOptions.forSnapshot().apply(query)
                    sendJson(output, 200, HierarchyDumper.dumpAtomicSnapshot(source, options).toString())
                }
                path == "/dump_xml" || path == "/hierarchy.xml" ||
                    (path == "/dump" && query["format"] == "xml") -> {
                    val options = HierarchyDumper.DumpOptions.forSnapshot().apply(query)
                    sendXml(output, HierarchyDumper.dumpXml(source, options))
                }
                path == "/dump" || path == "/hierarchy" -> {
                    val options = HierarchyDumper.DumpOptions.forDump().apply(query)
                    sendJson(output, 200, HierarchyDumper.dump(source, options).toString())
                }
                path == "/action" || path == "/rpc" -> {
                    val json = if (body.isEmpty()) JSONObject() else JSONObject(body)
                    sendJson(output, 200, executeCommand(json.optString("cmd", ""), json).toString())
                }
                // The path is not echoed: this port must not reflect anything
                // it was sent.
                else -> sendJson(output, 404, errorJson("Unknown endpoint").toString())
            }
        } catch (e: Exception) {
            // Logged, not returned: an exception message can carry internal
            // state, and the caller has no use for it.
            Log.e(TAG, "HTTP error", e)
            runCatching { sendJson(output, 500, errorJson("Internal error").toString()) }
        }
    }

    // ---------------------------------------------------------------- //
    // Raw JSON-RPC
    // ---------------------------------------------------------------- //

    private fun handleJsonRpc(line: String, output: OutputStream) {
        try {
            val json = JSONObject(line)
            val cmd = json.optString("cmd", "")
            val authed = TokenStore.matches(json.optString("token").takeIf(String::isNotEmpty))
            val response = when {
                cmd.equals("ping", ignoreCase = true) -> buildPing(authed)
                !authed -> {
                    sleepQuietly(AUTH_FAILURE_DELAY_MS)
                    unauthorizedJson()
                }
                else -> executeCommand(cmd, json)
            }
            writeLine(output, response.toString())
        } catch (_: Exception) {
            runCatching { writeLine(output, errorJson("Malformed request").toString()) }
        }
    }

    private fun writeLine(output: OutputStream, payload: String) {
        output.write(payload.toByteArray())
        output.write('\n'.code)
        output.flush()
    }

    // ---------------------------------------------------------------- //
    // Commands
    // ---------------------------------------------------------------- //

    private fun executeCommand(cmd: String, params: JSONObject): JSONObject {
        val resp = JSONObject()
        try {
            when (cmd.lowercase(Locale.ROOT)) {
                "ping" -> return buildPing(true)
                "dump", "dump_ui" ->
                    return HierarchyDumper.dump(source, HierarchyDumper.DumpOptions.forDump())
                "snapshot" ->
                    return HierarchyDumper.dumpAtomicSnapshot(
                        source,
                        HierarchyDumper.DumpOptions.forSnapshot(),
                    )
                "dump_xml" -> {
                    resp.put("success", true)
                    resp.put("xml", HierarchyDumper.dumpXml(source))
                }
                "tap" -> withGestures(resp) { service ->
                    withPoint(params, resp) { x, y ->
                        GestureController.tap(service, x, y, params.optLong("timeout", 1_500L))
                    }
                }
                "double_tap" -> withGestures(resp) { service ->
                    withPoint(params, resp) { x, y ->
                        GestureController.doubleTap(service, x, y, params.optLong("timeout", 2_000L))
                    }
                }
                "long_press" -> withGestures(resp) { service ->
                    withPoint(params, resp) { x, y ->
                        GestureController.longPress(
                            service,
                            x,
                            y,
                            params.optLong("duration", 1_000L),
                            2_500L,
                        )
                    }
                }
                "swipe" -> withGestures(resp) { service ->
                    resp.put("success", GestureController.swipe(
                        service,
                        params.optDouble("x1", -1.0).toFloat(),
                        params.optDouble("y1", -1.0).toFloat(),
                        params.optDouble("x2", -1.0).toFloat(),
                        params.optDouble("y2", -1.0).toFloat(),
                        params.optLong("duration", 300L),
                        3_000L,
                    ))
                }
                "type" -> withGestures(resp) { service ->
                    resp.put("success", GestureController.setText(
                        service,
                        source,
                        params.optString("text", ""),
                        params.optBoolean("append", false),
                    ))
                }
                "clear" -> withGestures(resp) { service ->
                    resp.put("success", GestureController.clearText(service, source))
                }
                "clipboard" -> withGestures(resp) { service ->
                    resp.put("success", GestureController.setClipboard(service, params.optString("text", "")))
                }
                "global" -> withGestures(resp) { service ->
                    resp.put("success", GestureController.performGlobalAction(service, params.optString("action", "")))
                }
                else -> {
                    resp.put("success", false)
                    resp.put("error", "Unknown command")
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Command failed: ${e.javaClass.simpleName}")
            runCatching {
                resp.put("success", false)
                resp.put("error", "Command failed")
            }
        }
        return resp
    }

    /**
     * Runs [act] when this server can perform gestures, and says so when it
     * cannot.
     *
     * The instrumentation reads the screen; it does not act on it. Acting
     * belongs to the accessibility service, which is bound the whole time and
     * needs no process started to reach it.
     */
    private inline fun withGestures(resp: JSONObject, act: (AccessibilityService) -> Unit) {
        val service = gestures
        if (service == null) {
            resp.put("success", false)
            resp.put("error", "this endpoint reads the screen; gestures go to the helper service")
        } else {
            act(service)
        }
    }

    /** Runs [gesture] only when the request carried a point on the screen. */
    private inline fun withPoint(
        params: JSONObject,
        resp: JSONObject,
        gesture: (Float, Float) -> Boolean,
    ) {
        val x = params.optDouble("x", -1.0).toFloat()
        val y = params.optDouble("y", -1.0).toFloat()
        if (x < 0f || y < 0f) {
            resp.put("success", false)
            resp.put("error", "Invalid coordinates")
        } else {
            resp.put("success", gesture(x, y))
        }
    }

    /**
     * The health payload behind `GET /ping` and the `ping` command.
     *
     * The host compares `version_code` and `protocol_version` against the
     * bundled APK to decide whether to upgrade, and reads `token_set` to decide
     * whether to push its token. What is on screen — the foreground package and
     * activity — is told only to a caller that proved it holds the token.
     */
    /** The installed build, read from the package manager. */
    private val versionCode: Long
        get() = runCatching {
            val info = source.context.packageManager
                .getPackageInfo(source.context.packageName, 0)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                info.longVersionCode
            } else {
                @Suppress("DEPRECATION")
                info.versionCode.toLong()
            }
        }.getOrDefault(-1L)

    private val versionName: String
        get() = runCatching {
            source.context.packageManager
                .getPackageInfo(source.context.packageName, 0).versionName.orEmpty()
        }.getOrDefault("")

    private fun buildPing(authed: Boolean): JSONObject = JSONObject().apply {
        put("success", true)
        put("service", source.label)
        put("version_code", versionCode)
        put("version_name", versionName)
        put("protocol_version", PROTOCOL_VERSION)
        put("port", port)
        put("auth_required", true)
        put("token_set", TokenStore.isSet)
        put("authenticated", authed)
        if (authed) {
            put("package", source.currentPackage)
            put("activity", source.currentActivity)
        }
    }

    fun shutdown() {
        isRunning = false
        closeQuietly(serverSocket)
        clientExecutor.shutdownNow()
    }

    private companion object {
        const val TAG = "JevPilotCommandServer"

        /**
         * Bumped whenever the wire contract changes in a way an older host
         * cannot use. 2: token authentication, byte-accurate bodies,
         * visible-only dumps, `fields=`.
         */
        const val PROTOCOL_VERSION = 2

        const val MAX_HEADER_LINE = 16 * 1024
        const val MAX_HEADERS = 64
        const val MAX_BODY = 4 * 1024 * 1024
        const val MAX_CONCURRENT_CLIENTS = 8
        const val MAX_QUEUED_CLIENTS = 16
        const val BACKLOG = 16
        const val READ_TIMEOUT_MS = 10_000
        const val AUTH_FAILURE_DELAY_MS = 250L

        const val TOKEN_HEADER = "x-jev-token"

        fun errorJson(message: String): JSONObject =
            JSONObject().put("success", false).put("error", message)

        fun unauthorizedJson(): JSONObject = errorJson(
            if (TokenStore.isSet) {
                "unauthorized: wrong or missing X-Jev-Token"
            } else {
                "unauthorized: no session token has been pushed to the helper yet"
            }
        ).put("token_set", TokenStore.isSet)

        fun parseQuery(query: String): Map<String, String> {
            if (query.isEmpty()) return emptyMap()
            val params = HashMap<String, String>()
            for (pair in query.split("&")) {
                if (pair.isEmpty()) continue
                val eq = pair.indexOf('=')
                runCatching {
                    val key = URLDecoder.decode(if (eq < 0) pair else pair.substring(0, eq), "UTF-8")
                    val value = if (eq < 0) "" else URLDecoder.decode(pair.substring(eq + 1), "UTF-8")
                    params[key] = value
                }
            }
            return params
        }

        /** One line without CR/LF as raw bytes, or null at end of stream. */
        fun readLineBytes(input: InputStream): ByteArray? {
            val line = ByteArrayOutputStream(256)
            var b: Int
            while (true) {
                b = input.read()
                if (b < 0 || b == '\n'.code) break
                if (b != '\r'.code) line.write(b)
                check(line.size() <= MAX_HEADER_LINE) { "Header line too long" }
            }
            return if (b < 0 && line.size() == 0) null else line.toByteArray()
        }

        fun readExactly(input: InputStream, length: Int): ByteArray {
            val buf = ByteArray(length)
            var total = 0
            while (total < length) {
                val read = input.read(buf, total, length - total)
                if (read < 0) break
                total += read
            }
            return if (total < length) buf.copyOf(total) else buf
        }

        fun reason(status: Int): String = when (status) {
            200 -> "OK"
            400 -> "Bad Request"
            401 -> "Unauthorized"
            403 -> "Forbidden"
            404 -> "Not Found"
            413 -> "Payload Too Large"
            431 -> "Request Header Fields Too Large"
            else -> "Internal Server Error"
        }

        fun send(output: OutputStream, status: Int, contentType: String, bytes: ByteArray) {
            runCatching {
                val header = buildString {
                    append("HTTP/1.1 ").append(status).append(' ').append(reason(status))
                    append("\r\nContent-Type: ").append(contentType).append("; charset=utf-8")
                    append("\r\nContent-Length: ").append(bytes.size)
                    // The screen's contents must not be cached, and must not be
                    // re-typed by a sniffing client.
                    append("\r\nCache-Control: no-store")
                    append("\r\nX-Content-Type-Options: nosniff")
                    append("\r\nConnection: close\r\n\r\n")
                }
                output.write(header.toByteArray())
                output.write(bytes)
                output.flush()
            }
        }

        fun sendJson(output: OutputStream, status: Int, json: String) =
            send(output, status, "application/json", json.toByteArray())

        fun sendXml(output: OutputStream, xml: String) =
            send(output, 200, "application/xml", xml.toByteArray())

        fun sleepQuietly(millis: Long) {
            try {
                Thread.sleep(millis)
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
            }
        }

        fun closeQuietly(closeable: Closeable?) {
            runCatching { closeable?.close() }
        }
    }
}

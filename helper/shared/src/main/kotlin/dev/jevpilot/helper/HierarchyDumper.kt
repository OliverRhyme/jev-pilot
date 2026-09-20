package dev.jevpilot.helper

import android.graphics.Bitmap
import android.graphics.Rect
import android.os.Build
import android.os.SystemClock
import android.util.Base64
import android.util.Log
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream

/**
 * Reads the screen through the accessibility APIs and serialises it.
 *
 * What this buys over `uiautomator dump`:
 *
 * * **No `waitForIdle`.** UIAutomator blocks until the app stops drawing, which
 *   an animation or a video can defer indefinitely; here the tree is read as it
 *   stands.
 * * **Every window, not just the top one** — dialogs, popups, the IME, system
 *   UI and split-screen halves, ordered by Z layer.
 * * **UIAutomator's visibility rules**, kept deliberately: invisible nodes are
 *   dropped and bounds are clipped to the display, the window and every
 *   scrollable ancestor, so a row half under the toolbar reports only the part
 *   that can actually be touched. `include_invisible=1` keeps everything, for
 *   looking at a problem rather than acting on it.
 * * **Batched IPC on API 33+**: descendants are prefetched per `getChild`
 *   instead of one Binder round trip per node.
 *
 * The text of a password field is never serialised, whatever the framework
 * hands back.
 */
object HierarchyDumper {

    private const val TAG = "JevPilotHierarchyDumper"
    private const val MAX_DEPTH = 75
    private const val MAX_NODES = 8000

    /** `AccessibilityNodeInfo.FLAG_PREFETCH_DESCENDANTS_HYBRID`, API 33+. */
    private const val PREFETCH_DESCENDANTS_HYBRID = 1 shl 3

    /** What a dump should contain. Every field defaults to the cheapest answer. */
    class DumpOptions {
        var includeInvisible: Boolean = false
        var wantXml: Boolean = true
        var wantElements: Boolean = false
        var wantTree: Boolean = false

        /** Applies `fields=xml,elements,tree` and `include_invisible=1`. */
        fun apply(query: Map<String, String>?): DumpOptions {
            if (query == null) return this
            query["fields"]?.takeIf { it.isNotBlank() }?.let { fields ->
                wantXml = false
                wantElements = false
                wantTree = false
                for (field in fields.split(",")) {
                    when (field.trim().lowercase()) {
                        "xml" -> wantXml = true
                        "elements" -> wantElements = true
                        "tree" -> wantTree = true
                    }
                }
            }
            query["include_invisible"]?.let {
                includeInvisible = it == "1" || it.equals("true", ignoreCase = true)
            }
            return this
        }

        companion object {
            fun forDump(): DumpOptions = DumpOptions().apply {
                wantElements = true
                wantTree = true
            }

            fun forSnapshot(): DumpOptions = DumpOptions()
        }
    }

    /** Per-dump counters, reported back to the host. */
    private class DumpStats {
        var nodes: Int = 0
        var skippedInvisible: Int = 0
        var truncated: Boolean = false
    }

    @JvmOverloads
    fun dump(source: ScreenSource, options: DumpOptions = DumpOptions.forDump()): JSONObject {
        val startTime = System.currentTimeMillis()
        val result = JSONObject()

        try {
            val displayInfo = DisplayUtils.getDisplayInfo(source.context)
            val stats = DumpStats()
            val rootSnapshots = captureRootSnapshots(source, displayInfo, options, stats)

            result.put("rotation", displayInfo.rotation)
            result.put("width", displayInfo.width)
            result.put("height", displayInfo.height)

            if (rootSnapshots.isEmpty()) {
                result.put("success", false)
                result.put("error", "No active window or root node found")
                result.put("xml", "")
                result.put("elements", JSONArray())
                return result
            }

            if (options.wantXml) {
                result.put("xml", buildXml(rootSnapshots, displayInfo.rotation))
            }
            if (options.wantElements) {
                val flat = ArrayList<JSONObject>()
                rootSnapshots.forEach { it.collectFlatElements(flat) }
                result.put("elements", JSONArray().apply { flat.forEach { put(it) } })
            }
            if (options.wantTree) {
                val trees = JSONArray().apply { rootSnapshots.forEach { put(it.toTreeJson()) } }
                result.put("tree", if (trees.length() == 1) trees.getJSONObject(0) else trees)
            }

            result.put("success", true)
            result.put("node_count", stats.nodes)
            result.put("skipped_invisible", stats.skippedInvisible)
            result.put("truncated", stats.truncated)
            result.put("window_count", rootSnapshots.size)
            result.put("elapsed_ms", System.currentTimeMillis() - startTime)

            result.put("package", source.currentPackage)
            result.put("activity", source.currentActivity)
        } catch (t: Throwable) {
            Log.e(TAG, "Dump failed with exception", t)
            runCatching {
                result.put("success", false)
                result.put("error", "Dump failed")
                result.put("xml", "")
                result.put("elements", JSONArray())
            }
        }

        return result
    }

    /**
     * A screenshot and the hierarchy captured on the same tick.
     *
     * The two are started together so the picture and the tree describe the
     * same screen; reading them in sequence lets the UI move in between, and
     * the mismatch is invisible in the result. When the screenshot arrives,
     * `width` and `height` become the bitmap's own, so a caller normalises
     * coordinates against the very image it is looking at.
     */
    @JvmOverloads
    fun dumpAtomicSnapshot(
        source: ScreenSource,
        options: DumpOptions = DumpOptions.forSnapshot(),
    ): JSONObject {
        val startTime = System.currentTimeMillis()

        // Taken on its own thread so the picture and the tree describe the
        // same screen: reading them in sequence lets the UI move in between,
        // and the mismatch is invisible in the result.
        val shot = java.util.concurrent.atomic.AtomicReference<Bitmap?>(null)
        val taking = Thread { shot.set(source.screenshot()) }.apply { start() }

        val dumpData = dump(source, options)
        runCatching { taking.join(3_000L) }

        val bitmap = shot.get()
        if (bitmap != null) {
            try {
                val stream = ByteArrayOutputStream(bitmap.width * bitmap.height / 4)
                bitmap.compress(Bitmap.CompressFormat.JPEG, 80, stream)
                dumpData.put(
                    "screenshot_base64",
                    Base64.encodeToString(stream.toByteArray(), Base64.NO_WRAP),
                )
                dumpData.put("has_screenshot", true)
                // The bitmap's own size, so a caller normalises coordinates
                // against the very image it is looking at.
                dumpData.put("width", bitmap.width)
                dumpData.put("height", bitmap.height)
            } catch (t: Throwable) {
                Log.w(TAG, "Failed to compress screenshot to JPEG Base64", t)
                runCatching { dumpData.put("has_screenshot", false) }
            } finally {
                bitmap.recycle()
            }
        } else {
            runCatching { dumpData.put("has_screenshot", false) }
        }

        runCatching { dumpData.put("atomic_elapsed_ms", System.currentTimeMillis() - startTime) }
        return dumpData
    }

    /** The hierarchy as a UIAutomator-shaped XML document. */
    @JvmOverloads
    fun dumpXml(
        source: ScreenSource,
        options: DumpOptions = DumpOptions.forSnapshot(),
    ): String {
        val displayInfo = DisplayUtils.getDisplayInfo(source.context)
        val roots = captureRootSnapshots(source, displayInfo, options, DumpStats())
        if (roots.isEmpty()) {
            return "<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>\n" +
                "<hierarchy rotation=\"${displayInfo.rotation}\" />\n"
        }
        return buildXml(roots, displayInfo.rotation)
    }

    private fun buildXml(roots: List<A11yNode>, rotation: Int): String {
        val sb = StringBuilder(roots.size * 1024 + 256)
        sb.append("<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>\n")
        sb.append("<hierarchy rotation=\"").append(rotation).append("\">")
        roots.forEachIndexed { i, root ->
            root.index = i
            root.writeXml(sb)
        }
        sb.append("</hierarchy>\n")
        return sb.toString()
    }

    /** One window's live root, with the window metadata that describes it. */
    private class RawRootEntry(
        val root: AccessibilityNodeInfo,
        val windowId: Int,
        val windowType: String,
        val windowLayer: Int,
        val windowActive: Boolean,
        val windowFocused: Boolean,
        /** Screen rectangle of the window, or null when unknown. */
        val windowBounds: Rect?,
    )

    /**
     * Snapshots every active window, retrying briefly while none is available.
     *
     * During an activity transition or a cold start the window list is
     * momentarily empty. Backing off in short steps rides that out, where
     * returning immediately would report a blank screen that never existed.
     */
    private fun captureRootSnapshots(
        source: ScreenSource,
        displayInfo: DisplayUtils.DisplayInfo,
        options: DumpOptions,
        stats: DumpStats,
    ): List<A11yNode> {
        val retryBackoff = longArrayOf(40L, 80L, 120L, 160L, 220L, 300L)
        var rawRoots: List<RawRootEntry> = emptyList()

        for (attempt in 0..retryBackoff.size) {
            rawRoots = getActiveRawRoots(source)
            if (rawRoots.isNotEmpty()) break
            if (attempt < retryBackoff.size) SystemClock.sleep(retryBackoff[attempt])
        }

        val displayRect = Rect(0, 0, displayInfo.width, displayInfo.height)
        val snapshots = ArrayList<A11yNode>(rawRoots.size)

        rawRoots.forEachIndexed { i, entry ->
            try {
                var clip = Rect(displayRect)
                if (entry.windowBounds != null && !clip.intersect(entry.windowBounds)) {
                    // Entirely off the display, as a second screen would be:
                    // keep the display rect so the root still serialises sanely.
                    clip = Rect(displayRect)
                }
                snapshotNode(entry.root, 0, i, clip, options, stats)?.let { snapshot ->
                    snapshot.windowId = entry.windowId
                    snapshot.windowType = entry.windowType
                    snapshot.windowLayer = entry.windowLayer
                    snapshot.windowActive = entry.windowActive
                    snapshot.windowFocused = entry.windowFocused
                    snapshots.add(snapshot)
                }
            } catch (t: Throwable) {
                Log.w(TAG, "Failed to snapshot root window ${entry.windowId}", t)
            } finally {
                safeRecycle(entry.root)
            }
        }

        return snapshots
    }

    private fun windowRoot(window: AccessibilityWindowInfo): AccessibilityNodeInfo? {
        if (Build.VERSION.SDK_INT >= 33) {
            runCatching { return window.getRoot(PREFETCH_DESCENDANTS_HYBRID) }
        }
        return window.root
    }

    private fun childOf(node: AccessibilityNodeInfo, index: Int): AccessibilityNodeInfo? {
        if (Build.VERSION.SDK_INT >= 33) {
            runCatching { return node.getChild(index, PREFETCH_DESCENDANTS_HYBRID) }
        }
        return node.getChild(index)
    }

    /**
     * Finds the window roots to read, in three tiers.
     *
     * 1. Every window, sorted by Z layer, which covers dialogs, popups, the IME
     *    and split screen.
     * 2. The active window alone, when the list came back empty or without an
     *    application window.
     * 3. The focused node, walked up to its root — the last resort that still
     *    recovers a tree mid-transition.
     */
    private fun getActiveRawRoots(source: ScreenSource): List<RawRootEntry> {
        val roots = ArrayList<RawRootEntry>()
        val seenHashes = HashSet<Int>()
        var hasAppWindow = false

        runCatching {
            val windows = source.windows()
            if (windows.isNotEmpty()) {
                for (window in windows.sortedByDescending { it.layer }) {
                    runCatching {
                        val root = windowRoot(window)
                        if (root != null) {
                            if (seenHashes.add(root.hashCode())) {
                                if (window.type == AccessibilityWindowInfo.TYPE_APPLICATION) {
                                    hasAppWindow = true
                                }
                                val bounds = Rect().also { window.getBoundsInScreen(it) }
                                roots.add(
                                    RawRootEntry(
                                        root,
                                        window.id,
                                        resolveWindowType(window.type),
                                        window.layer,
                                        window.isActive,
                                        window.isFocused,
                                        bounds.takeUnless { it.isEmpty },
                                    )
                                )
                            } else {
                                safeRecycle(root)
                            }
                        }
                    }
                }
            }
        }

        if (!hasAppWindow) {
            runCatching {
                source.activeRoot()?.let { activeRoot ->
                    if (seenHashes.add(activeRoot.hashCode())) {
                        roots.add(
                            0,
                            RawRootEntry(
                                activeRoot,
                                activeRoot.windowId,
                                "application",
                                0,
                                windowActive = true,
                                windowFocused = true,
                                windowBounds = null,
                            )
                        )
                    } else {
                        safeRecycle(activeRoot)
                    }
                }
            }
        }

        if (roots.isEmpty()) {
            val focused =
                source.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
                    ?: source.findFocus(AccessibilityNodeInfo.FOCUS_ACCESSIBILITY)

            if (focused != null) {
                try {
                    var current = focused
                    var parent = current.parent
                    while (parent != null) {
                        if (current !== focused) safeRecycle(current)
                        current = parent
                        parent = current.parent
                    }
                    if (seenHashes.add(current.hashCode())) {
                        roots.add(
                            RawRootEntry(
                                current,
                                current.windowId,
                                "application",
                                0,
                                windowActive = true,
                                windowFocused = true,
                                windowBounds = null,
                            )
                        )
                    } else {
                        safeRecycle(current)
                    }
                } catch (_: Throwable) {
                } finally {
                    safeRecycle(focused)
                }
            }
        }

        return roots
    }

    private fun resolveWindowType(type: Int): String = when (type) {
        AccessibilityWindowInfo.TYPE_APPLICATION -> "application"
        AccessibilityWindowInfo.TYPE_INPUT_METHOD -> "input_method"
        AccessibilityWindowInfo.TYPE_SYSTEM -> "system"
        AccessibilityWindowInfo.TYPE_ACCESSIBILITY_OVERLAY -> "accessibility_overlay"
        AccessibilityWindowInfo.TYPE_SPLIT_SCREEN_DIVIDER -> "split_screen_divider"
        else -> "unknown"
    }

    /**
     * Copies one live node, and its children, into detached [A11yNode]s.
     *
     * [clip] is where the node may be visible: the display intersected with the
     * window and every scrollable ancestor. Children not visible to the user
     * are dropped unless [DumpOptions.includeInvisible]; the window root itself
     * is always kept, as UIAutomator does.
     *
     * Depth and node count are capped because a malformed or hostile hierarchy
     * can be cyclic, and an unbounded walk would hang the service rather than
     * return a partial answer.
     */
    @Suppress("DEPRECATION")
    private fun snapshotNode(
        node: AccessibilityNodeInfo?,
        depth: Int,
        childIndex: Int,
        clip: Rect,
        options: DumpOptions,
        stats: DumpStats,
    ): A11yNode? {
        if (node == null) return null
        if (depth > MAX_DEPTH || stats.nodes >= MAX_NODES) {
            stats.truncated = true
            return null
        }

        val visible = runCatching { node.isVisibleToUser }.getOrDefault(true)
        if (!visible && depth > 0 && !options.includeInvisible) {
            stats.skippedInvisible++
            return null
        }

        stats.nodes++
        val snapshot = A11yNode()
        snapshot.index = childIndex
        snapshot.visibleToUser = visible
        var childClip = clip

        try {
            val bounds = Rect().also { node.getBoundsInScreen(it) }
            if (!bounds.intersect(clip)) bounds.setEmpty()
            snapshot.left = bounds.left
            snapshot.top = bounds.top
            snapshot.right = bounds.right
            snapshot.bottom = bounds.bottom

            snapshot.contentDesc = node.contentDescription?.toString().orEmpty()
            snapshot.packageName = node.packageName?.toString().orEmpty()
            snapshot.className = node.className?.toString().orEmpty()
            snapshot.resourceId = node.viewIdResourceName.orEmpty()

            snapshot.clickable = node.isClickable
            snapshot.checkable = node.isCheckable
            snapshot.checked = node.isChecked
            snapshot.enabled = node.isEnabled
            snapshot.focusable = node.isFocusable
            snapshot.focused = node.isFocused
            snapshot.scrollable = node.isScrollable
            snapshot.longClickable = node.isLongClickable
            snapshot.password = node.isPassword
            snapshot.selected = node.isSelected

            // A password field's contents never leave the device through here,
            // whatever the framework chose to put in getText().
            snapshot.text = if (snapshot.password) "" else node.text?.toString().orEmpty()

            // A scrollable container clips its children, as UIAutomator's
            // trimScrollableParent does: a row half under the toolbar keeps only
            // its visible part, so its centre is a point that can be touched.
            if (snapshot.scrollable && !bounds.isEmpty) {
                childClip = Rect(bounds)
            }

            snapshot.editable = node.isEditable
            node.error?.takeIf { it.isNotEmpty() }?.let { snapshot.errorText = it.toString() }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                runCatching { snapshot.drawingOrder = node.drawingOrder }
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                runCatching {
                    node.hintText?.let { snapshot.hint = it.toString() }
                    // An empty field reports its hint as the text; keeping them
                    // apart is what leaves "this input is empty" detectable.
                    if (node.isShowingHintText) snapshot.text = ""
                }
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                runCatching { snapshot.isHeading = node.isHeading }
                runCatching { snapshot.screenReaderFocusable = node.isScreenReaderFocusable }
                runCatching { node.paneTitle?.let { snapshot.paneTitle = it.toString() } }
                runCatching { node.tooltipText?.let { snapshot.tooltip = it.toString() } }
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                runCatching {
                    node.stateDescription?.let { snapshot.stateDescription = it.toString() }
                }
            }

            for (i in 0 until node.childCount) {
                var childNode: AccessibilityNodeInfo? = null
                try {
                    childNode = childOf(node, i)
                    if (childNode != null) {
                        snapshotNode(childNode, depth + 1, i, childClip, options, stats)
                            ?.let { snapshot.children.add(it) }
                    }
                } catch (_: Throwable) {
                } finally {
                    childNode?.let { safeRecycle(it) }
                }
            }
        } catch (t: Throwable) {
            Log.w(TAG, "Error reading node properties", t)
        }

        return snapshot
    }

    /**
     * Releases a node on API < 30, where the Binder pool is exhaustible.
     *
     * `recycle` is a no-op and deprecated from API 30, so calling it there
     * only earns a warning.
     */
    @Suppress("DEPRECATION")
    @JvmStatic
    fun safeRecycle(node: AccessibilityNodeInfo?) {
        if (node == null) return
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            runCatching { node.recycle() }
        }
    }

    /**
     * The focused, or failing that the first editable, input node.
     *
     * Window roots are enumerated once and recycled; only the node returned
     * stays alive, and the caller owns it.
     */
    @JvmStatic
    fun findInputNode(source: ScreenSource): AccessibilityNodeInfo? {
        val roots = getActiveRawRoots(source)
        var found: AccessibilityNodeInfo? = null
        try {
            for (entry in roots) {
                val focused = runCatching {
                    entry.root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
                }.getOrNull()
                if (focused != null) {
                    found = focused
                    return found
                }
            }
            for (entry in roots) {
                val editable = findFirstEditable(entry.root, 0)
                if (editable != null) {
                    found = editable
                    return found
                }
            }
            return null
        } finally {
            roots.forEach { if (it.root !== found) safeRecycle(it.root) }
        }
    }

    private fun findFirstEditable(node: AccessibilityNodeInfo?, depth: Int): AccessibilityNodeInfo? {
        if (node == null || depth > MAX_DEPTH) return null
        runCatching {
            if (node.isEditable && node.isFocusable && node.isEnabled && node.isVisibleToUser) {
                return node
            }
            for (i in 0 until node.childCount) {
                val child = childOf(node, i)
                if (child != null) {
                    val result = findFirstEditable(child, depth + 1)
                    if (result != null) return result
                    safeRecycle(child)
                }
            }
        }
        return null
    }
}

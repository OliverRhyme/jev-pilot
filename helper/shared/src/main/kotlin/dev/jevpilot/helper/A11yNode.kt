package dev.jevpilot.helper

import org.json.JSONArray
import org.json.JSONObject

/**
 * A detached copy of one accessibility node.
 *
 * Live `AccessibilityNodeInfo` objects are Binder proxies: holding them past
 * the traversal leaks them, and reading one after its window has changed
 * throws. Everything needed is copied out during the walk so serialisation is
 * thread-safe and cannot fail on a stale node.
 *
 * [left], [top], [right] and [bottom] are the node's *visible* bounds — the
 * raw `getBoundsInScreen` rectangle intersected with the display, the node's
 * window and every scrollable ancestor, as UIAutomator's
 * `getVisibleBoundsInScreen` does. They are never negative and never extend
 * past the screen.
 */
class A11yNode {

    var index: Int = 0
    var text: String = ""
    var resourceId: String = ""
    var className: String = ""
    var packageName: String = ""
    var contentDesc: String = ""

    var checkable: Boolean = false
    var checked: Boolean = false
    var clickable: Boolean = false
    var enabled: Boolean = true
    var focusable: Boolean = false
    var focused: Boolean = false
    var scrollable: Boolean = false
    var longClickable: Boolean = false
    var password: Boolean = false
    var selected: Boolean = false
    var visibleToUser: Boolean = true

    /** Window metadata, emitted on window root nodes only. */
    var windowId: Int = -1
    var windowType: String = ""
    var windowLayer: Int = 0
    var windowActive: Boolean = false
    var windowFocused: Boolean = false

    var editable: Boolean = false
    var isHeading: Boolean = false
    var screenReaderFocusable: Boolean = false
    var stateDescription: String = ""
    var errorText: String = ""
    var paneTitle: String = ""
    var tooltip: String = ""

    var left: Int = 0
    var top: Int = 0
    var right: Int = 0
    var bottom: Int = 0

    var drawingOrder: Int = 0
    var hint: String = ""

    val children: MutableList<A11yNode> = ArrayList(4)

    val width: Int get() = right - left
    val height: Int get() = bottom - top

    val boundsString: String get() = "[$left,$top][$right,$bottom]"

    /** Writes this node and its descendants as UIAutomator-shaped XML. */
    fun writeXml(sb: StringBuilder) {
        sb.append("<node")
        XmlUtils.appendIntAttribute(sb, "index", index)
        XmlUtils.appendAttribute(sb, "text", text)
        XmlUtils.appendAttribute(sb, "resource-id", resourceId)
        XmlUtils.appendAttribute(sb, "class", className)
        XmlUtils.appendAttribute(sb, "package", packageName)
        XmlUtils.appendAttribute(sb, "content-desc", contentDesc)
        XmlUtils.appendBooleanAttribute(sb, "checkable", checkable)
        XmlUtils.appendBooleanAttribute(sb, "checked", checked)
        XmlUtils.appendBooleanAttribute(sb, "clickable", clickable)
        XmlUtils.appendBooleanAttribute(sb, "enabled", enabled)
        XmlUtils.appendBooleanAttribute(sb, "focusable", focusable)
        XmlUtils.appendBooleanAttribute(sb, "focused", focused)
        XmlUtils.appendBooleanAttribute(sb, "scrollable", scrollable)
        XmlUtils.appendBooleanAttribute(sb, "long-clickable", longClickable)
        XmlUtils.appendBooleanAttribute(sb, "password", password)
        XmlUtils.appendBooleanAttribute(sb, "selected", selected)
        XmlUtils.appendBooleanAttribute(sb, "visible-to-user", visibleToUser)
        XmlUtils.appendAttribute(sb, "bounds", boundsString)
        XmlUtils.appendIntAttribute(sb, "drawing-order", drawingOrder)

        fun optional(name: String, value: String) {
            if (value.isNotEmpty()) XmlUtils.appendAttribute(sb, name, value)
        }
        fun flag(name: String, value: Boolean) {
            if (value) XmlUtils.appendBooleanAttribute(sb, name, true)
        }

        optional("hint", hint)
        if (windowId >= 0) {
            XmlUtils.appendIntAttribute(sb, "window-id", windowId)
            optional("window-type", windowType)
            XmlUtils.appendIntAttribute(sb, "window-layer", windowLayer)
            flag("window-active", windowActive)
            flag("window-focused", windowFocused)
        }
        flag("editable", editable)
        flag("heading", isHeading)
        flag("screen-reader-focusable", screenReaderFocusable)
        optional("state-description", stateDescription)
        optional("error", errorText)
        optional("pane-title", paneTitle)
        optional("tooltip", tooltip)

        if (children.isEmpty()) {
            sb.append(" />")
        } else {
            sb.append('>')
            children.forEach { it.writeXml(sb) }
            sb.append("</node>")
        }
    }

    /** This node and its descendants as a nested JSON tree. */
    fun toTreeJson(): JSONObject {
        val obj = toBaseJson()
        runCatching {
            if (children.isNotEmpty()) {
                obj.put("children", JSONArray().apply { children.forEach { put(it.toTreeJson()) } })
            }
        }
        return obj
    }

    /** This node alone, for the flat element list. */
    fun toFlatElementJson(): JSONObject = toBaseJson()

    private fun toBaseJson(): JSONObject {
        val obj = JSONObject()
        runCatching {
            obj.put("class", className)
            obj.put("package", packageName)
            obj.put("resource-id", resourceId)
            obj.put("text", text)
            obj.put("content-desc", contentDesc)
            obj.put("bounds", boundsString)
            obj.put(
                "parsed_bounds",
                JSONObject()
                    .put("left", left)
                    .put("top", top)
                    .put("right", right)
                    .put("bottom", bottom)
            )
            obj.put("clickable", clickable)
            obj.put("scrollable", scrollable)
            obj.put("checkable", checkable)
            obj.put("checked", checked)
            obj.put("enabled", enabled)
            obj.put("focusable", focusable)
            obj.put("focused", focused)
            obj.put("selected", selected)
            obj.put("password", password)
            obj.put("long-clickable", longClickable)
            obj.put("visible-to-user", visibleToUser)
            obj.put("drawing-order", drawingOrder)
            if (hint.isNotEmpty()) obj.put("hint", hint)

            if (windowId >= 0) {
                obj.put("window_id", windowId)
                obj.put("window_type", windowType)
                obj.put("window_layer", windowLayer)
                if (windowActive) obj.put("window_active", true)
                if (windowFocused) obj.put("window_focused", true)
            }
            if (editable) obj.put("editable", true)
            if (isHeading) obj.put("is_heading", true)
            if (screenReaderFocusable) obj.put("screen_reader_focusable", true)
            if (stateDescription.isNotEmpty()) obj.put("state_description", stateDescription)
            if (errorText.isNotEmpty()) obj.put("error", errorText)
            if (paneTitle.isNotEmpty()) obj.put("pane_title", paneTitle)
            if (tooltip.isNotEmpty()) obj.put("tooltip", tooltip)
        }
        return obj
    }

    /** Whether this node says or offers anything, and so belongs in the flat list. */
    fun isInformativeOrInteractive(): Boolean {
        val hasContent = text.isNotEmpty() || contentDesc.isNotEmpty() || resourceId.isNotEmpty() ||
            stateDescription.isNotEmpty() || errorText.isNotEmpty() || paneTitle.isNotEmpty()
        val isInteractive =
            clickable || scrollable || checkable || focusable || longClickable || editable
        return hasContent || isInteractive || isHeading
    }

    /** Appends every informative or interactive node beneath here, depth first. */
    fun collectFlatElements(flatList: MutableList<JSONObject>) {
        if (isInformativeOrInteractive()) flatList.add(toFlatElementJson())
        children.forEach { it.collectFlatElements(flatList) }
    }
}

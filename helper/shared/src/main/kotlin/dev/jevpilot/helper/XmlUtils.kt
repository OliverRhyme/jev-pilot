package dev.jevpilot.helper

/**
 * XML serialisation that cannot produce a document the reader will reject.
 *
 * A screen's text is arbitrary application data: a NUL byte, an unpaired
 * surrogate or a raw `&` in a label makes the whole hierarchy unparseable, and
 * the failure surfaces far away as a malformed-document error. Characters
 * outside XML 1.0's legal range are dropped rather than escaped, because no
 * escape for them exists.
 */
object XmlUtils {

    /** Appends `name="value"`, escaped; an absent or empty value gives `name=""`. */
    fun appendAttribute(sb: StringBuilder, name: String, value: CharSequence?) {
        sb.append(' ').append(name).append("=\"")
        if (!value.isNullOrEmpty()) escapeXmlAttr(sb, value)
        sb.append('"')
    }

    /** Appends `name="true"` or `name="false"`. */
    fun appendBooleanAttribute(sb: StringBuilder, name: String, value: Boolean) {
        sb.append(' ').append(name).append(if (value) "=\"true\"" else "=\"false\"")
    }

    /** Appends `name="123"`. */
    fun appendIntAttribute(sb: StringBuilder, name: String, value: Int) {
        sb.append(' ').append(name).append("=\"").append(value).append('"')
    }

    /**
     * Escapes the five XML entities and the three legal control characters, and
     * drops everything XML 1.0 forbids:
     * `#x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]`.
     */
    fun escapeXmlAttr(sb: StringBuilder, text: CharSequence) {
        var i = 0
        val len = text.length
        while (i < len) {
            when (val c = text[i]) {
                '&' -> sb.append("&amp;")
                '<' -> sb.append("&lt;")
                '>' -> sb.append("&gt;")
                '"' -> sb.append("&quot;")
                '\'' -> sb.append("&apos;")
                '\t' -> sb.append("&#9;")
                '\n' -> sb.append("&#10;")
                '\r' -> sb.append("&#13;")
                else -> when {
                    c.code in 0x20..0xD7FF || c.code in 0xE000..0xFFFD -> sb.append(c)
                    // A surrogate is legal only as a complete pair; a lone half
                    // is not a character at all, so it goes no further.
                    c.isHighSurrogate() && i + 1 < len && text[i + 1].isLowSurrogate() -> {
                        sb.append(c).append(text[i + 1])
                        i++
                    }
                }
            }
            i++
        }
    }

    /** The escaped form of [text], or `""` when there is nothing to escape. */
    fun escapeXmlAttr(text: CharSequence?): String {
        if (text.isNullOrEmpty()) return ""
        return StringBuilder(text.length + 16).also { escapeXmlAttr(it, text) }.toString()
    }
}

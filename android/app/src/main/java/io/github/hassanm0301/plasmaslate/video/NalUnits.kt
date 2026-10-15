package io.github.hassanm0301.plasmaslate.video

/** Helpers for Annex-B byte streams. */
object NalUnits {
    /** Start offsets (of the NAL header, after the start code) of every NAL unit. */
    fun nalStarts(data: ByteArray): List<Int> {
        val out = ArrayList<Int>()
        var i = 0
        while (i + 2 < data.size) {
            if (data[i] == 0.toByte() && data[i + 1] == 0.toByte()) {
                if (data[i + 2] == 1.toByte()) {
                    out.add(i + 3); i += 3; continue
                }
                if (i + 3 < data.size && data[i + 2] == 0.toByte() && data[i + 3] == 1.toByte()) {
                    out.add(i + 4); i += 4; continue
                }
            }
            i++
        }
        return out
    }

    fun h264Type(b: Byte): Int = b.toInt() and 0x1f
    fun hevcType(b: Byte): Int = (b.toInt() shr 1) and 0x3f

    /**
     * Splits a keyframe access unit into (codec config, rest): SPS/PPS (H.264) or VPS/SPS/PPS
     * (HEVC) go into the config part, everything else stays in the frame. Start codes are kept.
     */
    fun splitConfig(data: ByteArray, hevc: Boolean): Pair<ByteArray, ByteArray> {
        val starts = nalStarts(data)
        if (starts.isEmpty()) return ByteArray(0) to data
        val config = java.io.ByteArrayOutputStream()
        val rest = java.io.ByteArrayOutputStream()
        for ((idx, s) in starts.withIndex()) {
            val begin = startCodeBegin(data, s)
            val end = if (idx + 1 < starts.size) startCodeBegin(data, starts[idx + 1]) else data.size
            val isConfig = if (hevc) hevcType(data[s]) in 32..34 else h264Type(data[s]) in 7..8
            (if (isConfig) config else rest).write(data, begin, end - begin)
        }
        return config.toByteArray() to rest.toByteArray()
    }

    /** Access-unit delimiter NAL (with start code): H.264 type 9 (`primary_pic_type` = any), HEVC type 35. */
    fun accessUnitDelimiter(hevc: Boolean): ByteArray =
        if (hevc) byteArrayOf(0, 0, 0, 1, 0x46, 0x01, 0x50) else byteArrayOf(0, 0, 0, 1, 0x09, 0xF0.toByte())

    /**
     * [data] followed by an access-unit delimiter. Decoders only know a frame is complete when
     * the next one starts; the delimiter says so right away, so they need not wait for the next
     * frame to output this one.
     */
    fun withTrailingAud(data: ByteArray, hevc: Boolean): ByteArray {
        val aud = accessUnitDelimiter(hevc)
        val out = data.copyOf(data.size + aud.size)
        System.arraycopy(aud, 0, out, data.size, aud.size)
        return out
    }

    private fun startCodeBegin(data: ByteArray, nalStart: Int): Int =
        if (nalStart >= 4 && data[nalStart - 4] == 0.toByte() && data[nalStart - 3] == 0.toByte() && data[nalStart - 2] == 0.toByte()) nalStart - 4
        else nalStart - 3
}

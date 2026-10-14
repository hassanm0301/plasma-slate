package io.github.hassanm0301.plasmaslate.net

import io.github.hassanm0301.plasmaslate.proto.Envelope
import java.io.DataInputStream
import java.io.EOFException
import java.io.IOException
import java.io.OutputStream

/** Wire framing: big-endian u32 length followed by one serialized [Envelope] (see proto/). */
object Framing {
    const val MAX_FRAME = 8 * 1024 * 1024

    fun encode(env: Envelope): ByteArray {
        val body = env.toByteArray()
        val out = ByteArray(4 + body.size)
        val n = body.size
        out[0] = (n ushr 24).toByte()
        out[1] = (n ushr 16).toByte()
        out[2] = (n ushr 8).toByte()
        out[3] = n.toByte()
        System.arraycopy(body, 0, out, 4, n)
        return out
    }

    fun write(out: OutputStream, env: Envelope) {
        out.write(encode(env))
        out.flush()
    }

    /** Returns null on a clean end of stream at a frame boundary. */
    fun read(input: DataInputStream): Envelope? {
        val len = try {
            input.readInt()
        } catch (e: EOFException) {
            return null
        }
        if (len < 0 || len > MAX_FRAME) throw IOException("bad frame length $len")
        val buf = ByteArray(len)
        input.readFully(buf)
        return Envelope.parseFrom(buf)
    }
}

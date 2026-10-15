package io.github.hassanm0301.plasmaslate

import com.google.protobuf.ByteString
import io.github.hassanm0301.plasmaslate.data.Prefs
import io.github.hassanm0301.plasmaslate.input.TouchMapper
import io.github.hassanm0301.plasmaslate.net.Framing
import io.github.hassanm0301.plasmaslate.net.PinningTrustManager
import io.github.hassanm0301.plasmaslate.proto.Envelope
import io.github.hassanm0301.plasmaslate.proto.Ping
import io.github.hassanm0301.plasmaslate.proto.VideoFrame
import io.github.hassanm0301.plasmaslate.video.NalUnits
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream

class ProtocolTest {
    @Test
    fun framingRoundTrip() {
        val out = ByteArrayOutputStream()
        val a = Envelope.newBuilder().setPing(Ping.newBuilder().setId(7).setSenderTimeUs(42)).build()
        val b = Envelope.newBuilder().setVideoFrame(
            VideoFrame.newBuilder().setSequence(3).setKeyframe(true).setData(ByteString.copyFrom(byteArrayOf(0, 0, 1, 0x65))),
        ).build()
        Framing.write(out, a)
        Framing.write(out, b)
        val bytes = out.toByteArray()
        // Big-endian length prefix as in the Rust implementation.
        assertEquals(a.serializedSize, (bytes[0].toInt() shl 24) or (bytes[1].toInt() shl 16) or (bytes[2].toInt() shl 8) or bytes[3].toInt())
        val input = DataInputStream(ByteArrayInputStream(bytes))
        assertEquals(a, Framing.read(input))
        assertEquals(b, Framing.read(input))
        assertNull(Framing.read(input))
    }

    @Test
    fun touchMapperFitCentre() {
        // 1920x1200 picture in a 1920x1200 view: identity.
        val m = TouchMapper(1920, 1200, 1920, 1200)
        assertEquals(0.5f to 0.5f, m.normalize(960f, 600f))
        // Picture letterboxed in a taller view: clamp outside.
        val l = TouchMapper(1920, 1400, 1920, 1200)
        assertEquals(0f, l.normalize(0f, 50f).second, 0.0001f)
        assertEquals(0.5f, l.normalize(960f, 700f).second, 0.0001f)
        assertEquals(1f, l.normalize(5000f, 5000f).first, 0.0001f)
    }

    @Test
    fun nalSplitH264() {
        val sps = byteArrayOf(0, 0, 0, 1, 0x67, 1, 2)
        val pps = byteArrayOf(0, 0, 0, 1, 0x68, 3)
        val idr = byteArrayOf(0, 0, 1, 0x65, 9, 9)
        val (config, rest) = NalUnits.splitConfig(sps + pps + idr, hevc = false)
        assertArrayEquals(sps + pps, config)
        assertArrayEquals(idr, rest)
        assertEquals(listOf(4, 11, 16), NalUnits.nalStarts(sps + pps + idr))
    }

    @Test
    fun nalSplitHevc() {
        val vps = byteArrayOf(0, 0, 0, 1, 0x40, 1)
        val sps = byteArrayOf(0, 0, 0, 1, 0x42, 1)
        val pps = byteArrayOf(0, 0, 0, 1, 0x44, 1)
        val idr = byteArrayOf(0, 0, 0, 1, 0x26, 1)
        val (config, rest) = NalUnits.splitConfig(vps + sps + pps + idr, hevc = true)
        assertArrayEquals(vps + sps + pps, config)
        assertArrayEquals(idr, rest)
    }

    @Test
    fun trailingAccessUnitDelimiter() {
        val frame = byteArrayOf(0, 0, 0, 1, 0x41, 9, 9)
        val h264 = NalUnits.withTrailingAud(frame, hevc = false)
        assertEquals(listOf(4, 11), NalUnits.nalStarts(h264))
        assertEquals(9, NalUnits.h264Type(h264[11]))
        val hevc = NalUnits.withTrailingAud(frame, hevc = true)
        assertEquals(35, NalUnits.hevcType(hevc[11]))
        // The frame itself is untouched.
        assertArrayEquals(frame, h264.copyOfRange(0, frame.size))
    }

    @Test
    fun fillerFlagRoundTrips() {
        val f = VideoFrame.newBuilder().setSequence(1).setFiller(true).setData(ByteString.copyFrom(byteArrayOf(0, 0, 1, 0x41))).build()
        val env = Envelope.newBuilder().setVideoFrame(f).build()
        val back = Envelope.parseFrom(env.toByteArray())
        assertEquals(true, back.videoFrame.filler)
        assertEquals(false, VideoFrame.getDefaultInstance().filler)
    }

    @Test
    fun parseEndpoints() {
        assertEquals("192.168.1.10:47420", Prefs.parse("192.168.1.10")!!.key)
        assertEquals("pc.local:5000", Prefs.parse(" pc.local:5000 ")!!.key)
        assertEquals("fe80::1:47420", Prefs.parse("[fe80::1]")!!.key)
        assertNull(Prefs.parse("host:99999"))
        assertNull(Prefs.parse(""))
    }

    @Test
    fun shortFingerprint() {
        assertEquals("2B96-541E", PinningTrustManager.short("2b96541e169c"))
    }
}

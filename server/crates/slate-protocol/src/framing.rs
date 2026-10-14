//! Length-prefixed framing: `u32 big-endian length` + `Envelope` bytes.

use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Envelope, MAX_FRAME_LEN};

/// Framing errors.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame too large: {0} bytes")]
    TooLarge(usize),
    #[error("decode: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("connection closed")]
    Closed,
}

/// Encodes an envelope into a complete frame (length prefix included).
pub fn encode_frame(env: &Envelope) -> Vec<u8> {
    let len = env.encoded_len();
    let mut buf = Vec::with_capacity(4 + len);
    buf.extend_from_slice(&(len as u32).to_be_bytes());
    // Encoding into a Vec with enough capacity cannot fail.
    let _ = env.encode(&mut buf);
    buf
}

/// Writes one frame and flushes.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    env: &Envelope,
) -> Result<(), FrameError> {
    let frame = encode_frame(env);
    if frame.len() - 4 > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(frame.len() - 4));
    }
    w.write_all(&frame).await?;
    w.flush().await?;
    Ok(())
}

/// Reads one frame. Returns [`FrameError::Closed`] on a clean EOF at a frame boundary.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Envelope, FrameError> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(Envelope::decode(buf.as_slice())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pb;
    use crate::Msg;

    #[tokio::test]
    async fn roundtrip_multiple_frames() {
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        let frames = vec![
            Envelope::new(Msg::Ping(pb::Ping {
                id: 7,
                sender_time_us: 42,
            })),
            Envelope::new(Msg::VideoFrame(pb::VideoFrame {
                sequence: 1,
                keyframe: true,
                data: vec![0, 0, 0, 1, 0x65, 1, 2, 3],
                ..Default::default()
            })),
            Envelope::new(Msg::Ready(pb::Ready {})),
        ];
        for f in &frames {
            write_frame(&mut a, f).await.unwrap();
        }
        drop(a);
        for f in &frames {
            assert_eq!(&read_frame(&mut b).await.unwrap(), f);
        }
        assert!(matches!(read_frame(&mut b).await, Err(FrameError::Closed)));
    }

    #[tokio::test]
    async fn rejects_oversized_frame() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&((MAX_FRAME_LEN as u32) + 1).to_be_bytes())
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut b).await,
            Err(FrameError::TooLarge(_))
        ));
    }

    #[test]
    fn frame_has_length_prefix() {
        let env = Envelope::new(Msg::Ready(pb::Ready {}));
        let frame = encode_frame(&env);
        let len = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        assert_eq!(len, frame.len() - 4);
    }
}

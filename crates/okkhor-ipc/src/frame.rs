#![forbid(unsafe_code)]
//! Length-prefixed framing: `u32` little-endian length, then UTF-8 JSON,
//! max [`crate::MAX_FRAME_BYTES`].
//!
//! Length 0 and lengths above the cap are rejected before any payload is
//! allocated. Payload bytes are validated as UTF-8 by the caller (the
//! transport decodes with `String::from_utf8`).

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::protocol::MAX_FRAME_BYTES;

/// Framing failures. None of these carry payload bytes.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("zero-length frame")]
    Empty,
    #[error("frame of {0} bytes exceeds the 1 MiB limit")]
    TooLarge(u32),
    #[error("connection closed mid-frame")]
    Truncated,
    #[error("frame read timed out")]
    Timeout,
    #[error("frame I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Write one frame: `u32` LE length followed by the payload.
///
/// Empty payloads are rejected for symmetry with [`read_frame`], which
/// rejects length 0.
pub async fn write_frame<W>(writer: &mut W, payload: &[u8]) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
{
    if payload.is_empty() {
        return Err(FrameError::Empty);
    }
    if payload.len() > MAX_FRAME_BYTES {
        let len = payload
            .len()
            .try_into()
            .map_err(|_| FrameError::TooLarge(u32::MAX))?;
        return Err(FrameError::TooLarge(len));
    }
    let len = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge(u32::MAX))?;
    writer.write_u32_le(len).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one frame, failing after `timeout` instead of blocking forever.
pub async fn read_frame<R>(
    reader: &mut R,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, FrameError>
where
    R: AsyncRead + Unpin,
{
    tokio::time::timeout(timeout, read_frame_inner(&mut *reader))
        .await
        .map_err(|_| FrameError::Timeout)?
}

async fn read_frame_inner<R>(reader: &mut R) -> Result<Vec<u8>, FrameError>
where
    R: AsyncRead + Unpin,
{
    let len = reader.read_u32_le().await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            FrameError::Truncated
        } else {
            FrameError::Io(error)
        }
    })?;
    if len == 0 {
        return Err(FrameError::Empty);
    }
    if len as usize > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let mut payload = vec![0u8; len as usize];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|_| FrameError::Truncated)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUICK: std::time::Duration = std::time::Duration::from_secs(5);

    #[tokio::test]
    async fn round_trip() {
        let (mut left, mut right) = tokio::io::duplex(64 * 1024);
        write_frame(&mut left, b"{}").await.expect("writes");
        let got = read_frame(&mut right, QUICK).await.expect("reads");
        assert_eq!(got, b"{}");
    }

    #[tokio::test]
    async fn rejects_empty_write() {
        let (mut left, _right) = tokio::io::duplex(64);
        assert!(matches!(
            write_frame(&mut left, b"").await,
            Err(FrameError::Empty)
        ));
    }

    #[tokio::test]
    async fn rejects_zero_length() {
        let (mut left, mut right) = tokio::io::duplex(64);
        left.write_u32_le(0).await.expect("writes len");
        assert!(matches!(
            read_frame(&mut right, QUICK).await,
            Err(FrameError::Empty)
        ));
    }

    #[tokio::test]
    async fn rejects_oversized_length_without_reading_body() {
        let (mut left, mut right) = tokio::io::duplex(64);
        left.write_u32_le((MAX_FRAME_BYTES as u32) + 1)
            .await
            .expect("writes len");
        drop(left);
        assert!(matches!(
            read_frame(&mut right, QUICK).await,
            Err(FrameError::TooLarge(_))
        ));
    }

    #[tokio::test]
    async fn rejects_oversized_write() {
        let (mut left, _) = tokio::io::duplex(64);
        let big = vec![0u8; MAX_FRAME_BYTES + 1];
        assert!(matches!(
            write_frame(&mut left, &big).await,
            Err(FrameError::TooLarge(_))
        ));
    }

    #[tokio::test]
    async fn truncation_is_an_error() {
        let (mut left, mut right) = tokio::io::duplex(64);
        // Declare 10 bytes, deliver 5, then close.
        left.write_u32_le(10).await.expect("writes len");
        left.write_all(b"12345").await.expect("writes part");
        drop(left);
        assert!(matches!(
            read_frame(&mut right, QUICK).await,
            Err(FrameError::Truncated)
        ));
    }

    #[tokio::test]
    async fn eof_before_length_is_truncation() {
        let (left, mut right) = tokio::io::duplex(64);
        drop(left);
        assert!(matches!(
            read_frame(&mut right, QUICK).await,
            Err(FrameError::Truncated)
        ));
    }

    #[tokio::test]
    async fn read_times_out() {
        let (_left, mut right) = tokio::io::duplex(64);
        // `_left` is held open but sends nothing, so the read must time out.
        assert!(matches!(
            read_frame(&mut right, std::time::Duration::from_millis(50)).await,
            Err(FrameError::Timeout)
        ));
    }
}

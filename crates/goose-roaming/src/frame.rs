//! Minimal length-prefixed framing used for the roaming handshake.
//!
//! Only the handshake (hello + accept/reject) is framed this way.
//! Once the handshake succeeds the raw stream is handed to the ACP protocol,
//! which does its own JSON-RPC framing.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::RoamingError;

/// Guard against a malicious peer announcing a huge handshake frame.
const MAX_FRAME_BYTES: u32 = 64 * 1024;

/// Write a `u32`-length-prefixed frame.
pub async fn write_frame<W>(w: &mut W, body: &[u8]) -> Result<(), RoamingError>
where
    W: AsyncWrite + Unpin,
{
    if body.len() as u64 > MAX_FRAME_BYTES as u64 {
        return Err(RoamingError::Transport("handshake frame too large".into()));
    }
    w.write_all(&(body.len() as u32).to_le_bytes())
        .await
        .map_err(RoamingError::Io)?;
    w.write_all(body).await.map_err(RoamingError::Io)?;
    w.flush().await.map_err(RoamingError::Io)?;
    Ok(())
}

/// Read a `u32`-length-prefixed frame.
pub async fn read_frame<R>(r: &mut R) -> Result<Vec<u8>, RoamingError>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await.map_err(RoamingError::Io)?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME_BYTES {
        return Err(RoamingError::Transport("handshake frame too large".into()));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await.map_err(RoamingError::Io)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trips() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let payload = b"hello roaming".to_vec();
        let p2 = payload.clone();
        let writer = tokio::spawn(async move {
            write_frame(&mut client, &p2).await.unwrap();
        });
        let got = read_frame(&mut server).await.unwrap();
        writer.await.unwrap();
        assert_eq!(got, payload);
    }

    #[tokio::test]
    async fn write_rejects_oversized_frame() {
        let (mut client, _server) = tokio::io::duplex(1024);
        let body = vec![0u8; MAX_FRAME_BYTES as usize + 1];
        assert!(write_frame(&mut client, &body).await.is_err());
    }

    #[tokio::test]
    async fn read_rejects_oversized_announcement() {
        // A peer announcing a huge frame must be rejected before allocation.
        let (mut client, mut server) = tokio::io::duplex(64);
        tokio::io::AsyncWriteExt::write_all(&mut client, &(MAX_FRAME_BYTES + 1).to_le_bytes())
            .await
            .unwrap();
        assert!(read_frame(&mut server).await.is_err());
    }
}

//! Transparent ACP proxy: splice a local ACP transport to a remote roaming stream.
//!
//! `roam connect`/`delegate` embed an ACP *client* with a built-in terminal UI.
//! This module is the opposite composition: it exposes a remote agent as a
//! *local ACP endpoint* so any ACP client (Zed, another editor)
//! can drive it as if it were local.
//!
//! The trick is that once the roaming handshake completes, the stream carries
//! raw ACP JSON-RPC framing — byte-for-byte what `goose acp` speaks over stdio.
//! So bridging is a pure copy in both directions: nothing runs an agent here and
//! nothing is deserialized. We just pump bytes local↔remote until both halves
//! close.

use anyhow::Result;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

/// Splice a local ACP transport (`local_read`/`local_write`) to a remote
/// roaming stream (`remote_write`/`remote_read`).
///
/// Runs both directions to completion: when the local client closes its input
/// we finish the remote's send half, and when the remote agent closes we finish
/// the local output half. This avoids truncating an in-flight ACP turn, which a
/// "return on first close" splice would do.
pub async fn splice<LR, LW, RW, RR>(
    mut local_read: LR,
    mut local_write: LW,
    mut remote_write: RW,
    mut remote_read: RR,
) -> Result<()>
where
    LR: AsyncRead + Unpin + Send,
    LW: AsyncWrite + Unpin + Send,
    RW: AsyncWrite + Unpin + Send,
    RR: AsyncRead + Unpin + Send,
{
    let client_to_host = async {
        tokio::io::copy(&mut local_read, &mut remote_write).await?;
        remote_write.shutdown().await
    };
    let host_to_client = async {
        tokio::io::copy(&mut remote_read, &mut local_write).await?;
        local_write.shutdown().await
    };

    tokio::try_join!(client_to_host, host_to_client)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// Bytes written by the local client reach the remote, and bytes from the
    /// remote reach the local client — both directions, to completion.
    ///
    /// The test ends are used as whole `DuplexStream`s (not split): a split end
    /// only reaches EOF once *both* halves are dropped, so `splice`'s copies
    /// would never observe EOF and the bridge would hang. Dropping the whole
    /// endpoint is what signals EOF and lets `splice` finish cleanly.
    #[tokio::test]
    async fn splices_both_directions() {
        let (mut local_client, local_endpoint) = tokio::io::duplex(64);
        let (remote_endpoint, mut remote_agent) = tokio::io::duplex(64);
        let (local_read, local_write) = tokio::io::split(local_endpoint);
        let (remote_read, remote_write) = tokio::io::split(remote_endpoint);

        let bridge = tokio::spawn(async move {
            splice(local_read, local_write, remote_write, remote_read).await
        });

        local_client.write_all(b"initialize").await.unwrap();
        let mut buf = [0u8; 10];
        remote_agent.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"initialize");

        remote_agent.write_all(b"session-ok").await.unwrap();
        let mut buf = [0u8; 10];
        local_client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"session-ok");

        drop(local_client);
        drop(remote_agent);
        bridge.await.unwrap().unwrap();
    }
}

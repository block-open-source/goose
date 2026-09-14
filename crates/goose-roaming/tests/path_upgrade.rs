//! Relay -> direct path upgrade under continuous traffic.
//!
//! Field report on #10906 (via discussion #11024): on same-NAT/hairpin
//! topologies, a comparable overlay stack saw the relay path work and then a
//! direct-upgrade rekey desync silently drop queued messages until restart.
//! This test pins the equivalent seam in roam: two nodes meet through a relay
//! (an in-process iroh test relay), the client dials with a relay-only
//! address (exactly what a browser card connect does), traffic flows, iroh
//! holepunches a direct localhost path mid-stream, and every frame sent
//! before, during, and after the migration must come back intact and in
//! order.
//!
//! Localhost holepunching is the closest CI-runnable stand-in for the
//! same-NAT hairpin case: both sides observe reflexive addresses that end up
//! on the loopback/LAN, and the upgrade + rekey machinery is the same code
//! path that runs behind a hairpinning NAT.

use std::sync::Arc;

use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use goose_roaming::{
    AcpStreamServer, Directory, RelayEntry, RelaySettings, RoamingConfig, RoamingIdentity,
    RoamingNode, TrustBook,
};
use iroh::{EndpointId, TransportAddr};

/// Echoes every byte back until the client closes its send side. Unlike the
/// one-shot echo in `end_to_end.rs`, this keeps the duplex busy across the
/// path migration so a rekey desync would surface as lost or corrupted data.
#[derive(Debug)]
struct StreamingEchoServer;

impl AcpStreamServer for StreamingEchoServer {
    fn serve_stream(
        &self,
        _client: EndpointId,
        mut recv: Box<dyn AsyncRead + Send + Unpin>,
        mut send: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async move {
            let mut buf = [0u8; 4096];
            loop {
                let n = recv.read(&mut buf).await?;
                if n == 0 {
                    return Ok(());
                }
                send.write_all(&buf[..n]).await?;
                send.flush().await?;
            }
        })
    }

    fn agent_id(&self) -> String {
        "streaming-echo".to_string()
    }
}

async fn bind_node_with_relay(relay: RelaySettings) -> Arc<RoamingNode> {
    RoamingNode::bind(RoamingConfig {
        identity: RoamingIdentity::generate(),
        relay,
        trust: TrustBook::new(),
        trust_path: None,
        directory: Directory::new(),
        bind_addr: None,
        relay_tls: Some(iroh::tls::CaTlsConfig::insecure_skip_verify()),
    })
    .await
    .expect("bind node")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_to_direct_upgrade_loses_no_data() {
    let (_relay_map, relay_url, _relay_guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("run test relay");
    let relay = RelaySettings::Custom(vec![RelayEntry::new(relay_url.to_string())]);

    let host = bind_node_with_relay(relay.clone()).await;
    host.share(Arc::new(StreamingEchoServer))
        .await
        .expect("share");

    let client = bind_node_with_relay(relay).await;
    host.trust().lock().await.accept(&client.endpoint_id());

    assert!(
        host.wait_online(std::time::Duration::from_secs(15)).await,
        "host never reached the test relay"
    );
    assert!(
        client.wait_online(std::time::Duration::from_secs(15)).await,
        "client never reached the test relay"
    );

    // Dial with a RELAY-ONLY address — the browser-card shape. The direct
    // path must be discovered by holepunching, not seeded by the dialer.
    let mut addr = iroh::EndpointAddr::new(host.endpoint_id());
    addr.addrs.insert(TransportAddr::Relay(
        relay_url_of(&host).expect("host has a relay addr"),
    ));
    let mut stream = client
        .connect_with_addr(addr, Some("hairpin-test".into()))
        .await
        .expect("connect through relay");

    // Watch path events on the client side of the connection.
    let mut path_events = stream.conn.path_events();

    // Prove we really started on the relay: the dial address was relay-only,
    // so a relay path must exist on the connection right now. Without this
    // the test could silently pass on a direct-from-the-start connection and
    // never exercise the migration at all.
    let has_relay_path = stream
        .conn
        .paths()
        .iter()
        .any(|p| matches!(p.remote_addr(), TransportAddr::Relay(_)));
    assert!(
        has_relay_path,
        "expected the connection to start with a relay path (dialed relay-only)"
    );

    // Phase 1: traffic while on the relay path.
    let mut counter: u64 = 0;
    exchange_frames(&mut stream, &mut counter, 50).await;

    // Wait for a direct (IP) path to open and be selected, pumping traffic
    // the whole time so the migration happens under load.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut direct_selected = false;
    while !direct_selected {
        tokio::select! {
            event = futures::StreamExt::next(&mut path_events) => {
                match event {
                    Some(iroh::endpoint::PathEvent::Selected { remote_addr: TransportAddr::Ip(_), .. }) => {
                        direct_selected = true;
                    }
                    Some(_) => {}
                    None => panic!("path event stream ended before direct upgrade"),
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                exchange_frames(&mut stream, &mut counter, 5).await;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no direct path was selected within 30s (holepunching failed on localhost)"
        );
    }

    // Phase 2: the rekey/migration just happened under load. Everything must
    // still round-trip exactly.
    exchange_frames(&mut stream, &mut counter, 50).await;

    assert!(counter >= 100, "test exchanged {counter} frames");
    stream.send.finish().unwrap();
    host.shutdown().await.unwrap();
}

/// Send `n` numbered frames and require each to echo back verbatim, in order.
/// Any dropped or reordered frame during path migration fails loudly here.
async fn exchange_frames(
    stream: &mut goose_roaming::RoamingClientStream,
    counter: &mut u64,
    n: usize,
) {
    for _ in 0..n {
        let msg = format!("frame-{:08}", *counter);
        stream
            .send
            .write_all(msg.as_bytes())
            .await
            .unwrap_or_else(|e| panic!("write failed at frame {counter}: {e}"));
        let mut buf = vec![0u8; msg.len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            stream.recv.read_exact(&mut buf),
        )
        .await
        .unwrap_or_else(|_| panic!("echo timed out at frame {counter} — data lost in migration"))
        .unwrap_or_else(|e| panic!("read failed at frame {counter}: {e}"));
        assert_eq!(
            buf,
            msg.as_bytes(),
            "frame {counter} corrupted across path migration"
        );
        *counter += 1;
    }
}

/// Burst of concurrent dials to one host (field report: a comparable stack
/// lost replies under parallel opens until dials were serialized per
/// process). Roam multiplexes streams over one QUIC connection per peer
/// pair, so parallel connects must all succeed and each stream must echo
/// independently.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_dial_burst() {
    let (_relay_map, relay_url, _relay_guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("run test relay");
    let relay = RelaySettings::Custom(vec![RelayEntry::new(relay_url.to_string())]);

    let host = bind_node_with_relay(relay.clone()).await;
    host.share(Arc::new(StreamingEchoServer))
        .await
        .expect("share");
    assert!(host.wait_online(std::time::Duration::from_secs(15)).await);

    let client = bind_node_with_relay(relay).await;
    host.trust().lock().await.accept(&client.endpoint_id());
    assert!(client.wait_online(std::time::Duration::from_secs(15)).await);

    let addr = {
        let mut a = iroh::EndpointAddr::new(host.endpoint_id());
        a.addrs.insert(TransportAddr::Relay(
            relay_url_of(&host).expect("host has a relay addr"),
        ));
        a
    };

    let mut tasks = Vec::new();
    for i in 0..8u32 {
        let client = client.clone();
        let addr = addr.clone();
        tasks.push(tokio::spawn(async move {
            let mut stream = client
                .connect_with_addr(addr, Some(format!("burst-{i}")))
                .await
                .unwrap_or_else(|e| panic!("parallel dial {i} failed: {e}"));
            let msg = format!("burst-payload-{i:04}");
            stream.send.write_all(msg.as_bytes()).await.unwrap();
            let mut buf = vec![0u8; msg.len()];
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                stream.recv.read_exact(&mut buf),
            )
            .await
            .unwrap_or_else(|_| panic!("dial {i}: echo timed out under burst"))
            .unwrap();
            assert_eq!(buf, msg.as_bytes(), "dial {i}: reply corrupted under burst");
            stream.send.finish().unwrap();
        }));
    }
    for t in tasks {
        t.await.expect("burst task panicked");
    }

    host.shutdown().await.unwrap();
}

/// The relay transport addr the host's endpoint currently advertises.
fn relay_url_of(node: &RoamingNode) -> Option<iroh::RelayUrl> {
    node.endpoint()
        .addr()
        .addrs
        .into_iter()
        .find_map(|a| match a {
            TransportAddr::Relay(url) => Some(url),
            _ => None,
        })
}

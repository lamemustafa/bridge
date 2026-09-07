//! MCP input lifecycle tests; no simulated Tally protocol is needed.
use super::*;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn cancellation_drops_pending_post_before_its_side_effect() {
    let (mut client, source) = tokio::io::duplex(1024);
    let mut reader = BufReader::new(source);
    let mut framer = Framer::default();
    let mut pending = std::collections::VecDeque::new();
    client.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n").await.unwrap();
    let future = async {
        std::future::pending::<()>().await;
        panic!("must not dispatch");
    };
    assert!(
        await_post(future, &json!(7), &mut reader, &mut framer, &mut pending)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn disconnect_drops_pending_post() {
    let mut reader = BufReader::new(&b""[..]);
    let result = await_post(
        std::future::pending(),
        &json!(7),
        &mut reader,
        &mut Framer::default(),
        &mut std::collections::VecDeque::new(),
    )
    .await;
    assert_eq!(result.err().as_deref(), Some("stdio_client_disconnected"));
}

#[tokio::test]
async fn interrupted_partial_frame_is_preserved() {
    let (mut client, source) = tokio::io::duplex(1024);
    let mut reader = BufReader::new(source);
    let mut framer = Framer::default();
    client.write_all(b"{\"jsonrpc\":\"2.0\",").await.unwrap();
    tokio::select! {
        biased;
        _ = framer.read(&mut reader, 1024) => panic!("not a complete frame"),
        _ = tokio::task::yield_now() => {}
    }
    client
        .write_all(b"\"id\":8,\"method\":\"ping\"}\n")
        .await
        .unwrap();
    let request = parse_request(
        framer
            .read(&mut reader, 1024)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(request["id"], 8);
}

#[tokio::test]
async fn queue_is_bounded_while_approval_waits() {
    let input = "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"ping\"}\n".repeat(9);
    let mut reader = BufReader::new(input.as_bytes());
    let mut pending = std::collections::VecDeque::new();
    let result = await_post(
        std::future::pending(),
        &json!(7),
        &mut reader,
        &mut Framer::default(),
        &mut pending,
    )
    .await;
    assert_eq!(
        result.err().as_deref(),
        Some("stdio_pending_requests_exceeded")
    );
    assert_eq!(pending.len(), 8);
}

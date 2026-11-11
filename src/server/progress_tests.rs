//! A cancelled handler must still close the progress of its blocking work.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tower_lsp_server::{LspService, Server};

use super::*;

async fn send(stream: &mut BufReader<DuplexStream>, message: Value) {
    let body = serde_json::to_vec(&message).unwrap();
    stream
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .unwrap();
    stream.write_all(&body).await.unwrap();
}

async fn read(stream: &mut BufReader<DuplexStream>) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut length = None;
        loop {
            let mut header = String::new();
            assert_ne!(stream.read_line(&mut header).await.unwrap(), 0);
            if header == "\r\n" {
                break;
            }
            if let Some(value) = header.strip_prefix("Content-Length: ") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0; length.expect("content length")];
        stream.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    })
    .await
    .expect("progress must finish even after the request is cancelled")
}

async fn cancelled_progress(token: Option<ProgressToken>, panic_in_worker: bool) {
    let created_by_server = token.is_none();
    let (service, socket) = LspService::build(Backend::new).finish();
    let backend = service.inner().clone_for_blocking();
    let (client, server) = tokio::io::duplex(1 << 16);
    let (input, output) = tokio::io::split(server);
    let server = tokio::spawn(Server::new(input, output, socket).serve(service));
    let mut client = BufReader::new(client);
    send(
        &mut client,
        json!({"jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{"capabilities":{"window":{"workDoneProgress":true}}}}),
    )
    .await;
    while read(&mut client).await["id"] != 1 {}

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let request = tokio::spawn(async move {
        backend
            .run_with_progress(
                "cancel-test",
                "Find References",
                "Scanning…",
                token,
                move |worker| {
                    started_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    worker
                        .request_progress
                        .as_ref()
                        .unwrap()
                        .set_percentage(100, "Finished scanning");
                    assert!(!panic_in_worker, "simulated worker failure");
                },
            )
            .await
    });

    let expected_token = if created_by_server {
        let create = loop {
            let message = read(&mut client).await;
            if message["method"] == "window/workDoneProgress/create" {
                break message;
            }
        };
        // Cancel while token creation is still awaiting the editor's reply.
        request.abort();
        send(
            &mut client,
            json!({"jsonrpc":"2.0", "id":create["id"], "result":null}),
        )
        .await;
        create["params"]["token"].clone()
    } else {
        json!(42)
    };
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .expect("worker starts")
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    release_tx.send(()).unwrap();

    let mut kinds = Vec::new();
    loop {
        let message = read(&mut client).await;
        if message["method"] != "$/progress" {
            continue;
        }
        assert_eq!(message["params"]["token"], expected_token);
        let kind = message["params"]["value"]["kind"]
            .as_str()
            .unwrap()
            .to_string();
        if kind == "report" {
            assert_eq!(message["params"]["value"]["percentage"], 100);
        }
        let ended = kind == "end";
        kinds.push(kind);
        if ended {
            break;
        }
    }
    assert_eq!(kinds, ["begin", "report", "end"]);

    // A following response is a barrier for detecting extra notifications,
    // and verifies that cancellation did not wedge the transport.
    send(
        &mut client,
        json!({"jsonrpc":"2.0", "id":2, "method":"shutdown", "params":null}),
    )
    .await;
    loop {
        let message = read(&mut client).await;
        assert_ne!(message["method"], "$/progress", "no report after end");
        if message["id"] == 2 {
            assert!(message.get("error").is_none());
            break;
        }
    }
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_request_ends_client_progress() {
    cancelled_progress(Some(ProgressToken::Int(42)), false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_request_during_progress_creation_still_ends_progress() {
    cancelled_progress(None, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_request_with_worker_panic_still_ends_progress() {
    cancelled_progress(Some(ProgressToken::Int(42)), true).await;
}

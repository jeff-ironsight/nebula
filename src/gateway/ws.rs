use crate::{protocol::GatewayPayload, state::AppState, types::ConnectionId};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    Error,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};
use uuid::Uuid;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(err) = handle_socket(state, socket).await {
            warn!("ws closed with error: {:?}", err);
        }
    })
}

async fn handle_socket(state: Arc<AppState>, socket: WebSocket) -> Result<(), Error> {
    let connection_id = ConnectionId::from(Uuid::new_v4());
    info!(?connection_id, "ws connected");

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<Message>();

    // Store sender so other tasks can send to this connection
    state.connections.insert(connection_id, outbound_tx.clone());

    // Writer task: drain outbound_rx -> websocket
    let send_task = tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            if ws_sender.send(message).await.is_err() {
                break;
            }
        }
    });

    // Send HELLO through the same outbound queue
    let hello = GatewayPayload::Hello {
        heartbeat_interval_ms: 25_000,
    };
    if outbound_tx.send(text_msg(&hello)).is_err() {
        warn!(?connection_id, "failed to enqueue hello payload");
    }

    // Reader loop
    let mut reader_result: Result<(), Error> = Ok(());

    while let Some(result) = ws_receiver.next().await {
        match result {
            Ok(Message::Text(text)) => {
                info!(?connection_id, "ws recv: {}", text.as_str());
                let _ = &state; // placeholder
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(e) => {
                reader_result = Err(e);
                break;
            }
        }
    }

    // Cleanup: remove from map first (stop others sending)
    state.connections.remove(&connection_id);

    // Drop our last sender so outbound_rx closes and send_task exits
    drop(outbound_tx);

    // Await writer task (no abort)
    let _ = send_task.await;

    info!(?connection_id, "ws disconnected");
    reader_result
}

fn text_msg<T: serde::Serialize>(value: &T) -> Message {
    Message::Text(serde_json::to_string(value).unwrap().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app, state::AppState};
    use futures_util::StreamExt;
    use tokio::net::TcpListener;
    use tokio::time::{sleep, timeout, Duration};
    use tokio_tungstenite::connect_async;

    #[tokio::test]
    async fn hello_payload_is_sent_on_connect() {
        let state = Arc::new(AppState::new());
        let router = app::build_router(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let url = format!("ws://{}/ws", addr);
        let (mut socket, _) = connect_async(&url).await.unwrap();

        let message = socket.next().await.unwrap().unwrap();
        let text = message.into_text().unwrap();
        let expected = serde_json::to_string(&GatewayPayload::Hello {
            heartbeat_interval_ms: 25_000,
        })
        .unwrap();

        assert_eq!(text, expected);
        assert_eq!(state.connections.len(), 1);

        socket.close(None).await.unwrap();
        timeout(Duration::from_secs(1), async {
            while state.connections.len() > 0 {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        server.abort();
    }
}

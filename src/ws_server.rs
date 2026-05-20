use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{error, info, warn};

use crate::{config::Config, orchestrator, protocol::HelperMessage};

pub async fn serve(config: Arc<Config>) -> anyhow::Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list([
            "https://chessova.com".parse().unwrap(),
            "http://localhost:3000".parse().unwrap(),
        ]))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    let app = Router::new()
        .route("/uci", get(ws_handler))
        .layer(cors)
        .with_state(config.clone());

    let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
    info!(addr = %config.bind_addr, "WS server listening");

    axum::serve(listener, app).await?;
    Ok(())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(config): State<Arc<Config>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, config))
}

async fn handle_socket(socket: WebSocket, config: Arc<Config>) {
    info!("client connected");

    // Per-connection orchestrator state (tracks in-flight analyze requests).
    let state = orchestrator::State::new();

    // Channel: engine tasks → WS writer half
    let (tx, mut rx) = mpsc::channel::<HelperMessage>(256);

    let (mut ws_tx, mut ws_rx) = socket.split();

    // Writer task: drain the channel → WS text frames
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    if ws_tx.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    error!(err = %e, "serialize HelperMessage failed");
                }
            }
        }
    });

    // Reader loop: parse ClientMessage → dispatch to orchestrator
    while let Some(frame) = ws_rx.next().await {
        let frame = match frame {
            Ok(f) => f,
            Err(e) => {
                warn!(err = %e, "WS read error");
                break;
            }
        };

        let text = match frame {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };

        let client_msg = match serde_json::from_str::<crate::protocol::ClientMessage>(&text) {
            Ok(m) => m,
            Err(e) => {
                warn!(err = %e, raw = %text, "parse ClientMessage failed");
                let _ = tx
                    .send(HelperMessage::Error {
                        id: None,
                        code: "parse-error".into(),
                        message: e.to_string(),
                    })
                    .await;
                continue;
            }
        };

        let config2 = config.clone();
        let state2 = state.clone();
        let tx2 = tx.clone();
        tokio::spawn(async move {
            orchestrator::handle(client_msg, &config2, &state2, &tx2).await;
        });
    }

    info!("client disconnected");
    writer.abort();
}

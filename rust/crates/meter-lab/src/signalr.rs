//! A minimal SignalR client (JSON hub protocol over WebSockets), covering what AlbionLabService used from
//! Microsoft.AspNetCore.SignalR.Client: negotiate with a bearer token, handshake, invocations and their completions,
//! keep-alive pings, the server's close message and automatic reconnect (0, 2, 10 and 30 seconds, then give up).

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const SEPARATOR: char = '\u{1e}';
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const KEEP_ALIVE: Duration = Duration::from_secs(15);
const SERVER_TIMEOUT: Duration = Duration::from_secs(30);
const RECONNECT_DELAYS: [u64; 4] = [0, 2, 10, 30];
/// How long an invocation waits for its completion, and a frame for the socket to take it: a stalled link fails the
/// call instead of holding the publish loop.
const INVOKE_TIMEOUT: Duration = Duration::from_secs(15);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubError {
    /// The hub method failed on the server (C# `HubException`); the message is the server's.
    Hub(String),
    /// Negotiate answered 401: the token is no longer accepted.
    Unauthorized,
    /// Negotiate failed or the server could not be reached (C# `HttpRequestException`).
    Http(String),
    /// The connection is not active (C# `InvalidOperationException`).
    NotConnected,
    /// The connection closed while the invocation waited for its completion.
    Closed(String),
    Timeout,
}

impl fmt::Display for HubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hub(message) | Self::Http(message) => f.write_str(message),
            Self::Unauthorized => f.write_str("Response status code does not indicate success: 401 (Unauthorized)."),
            Self::NotConnected => f.write_str("The connection is not active."),
            Self::Closed(reason) => write!(f, "Invocation canceled due to the underlying connection being closed. {reason}"),
            Self::Timeout => f.write_str("The SignalR connection timed out."),
        }
    }
}

impl std::error::Error for HubError {}

type TokenProvider = Box<dyn Fn() -> Option<String> + Send + Sync>;
type Callback = Box<dyn Fn() + Send + Sync>;
type Completion = oneshot::Sender<Result<Value, HubError>>;

struct Link {
    state: HubState,
    /// Bumped by every start and stop, so a connection task from before knows it no longer owns the hub.
    generation: u64,
    sender: Option<mpsc::UnboundedSender<String>>,
    pending: HashMap<String, Completion>,
}

struct Shared {
    url: String,
    http: reqwest::Client,
    token: TokenProvider,
    reconnected: Mutex<Option<Callback>>,
    link: Mutex<Link>,
    next_id: AtomicU64,
}

#[derive(Clone)]
pub struct HubConnection {
    shared: Arc<Shared>,
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// How a connection ended.
enum Ended {
    Stopped,
    /// The server sent a close message; reconnect only when it allows it.
    Closed { error: Option<String>, reconnect: bool },
    Lost(String),
}

impl HubConnection {
    /// `url` is the hub's http(s) address; `token` is asked for a bearer token on every (re)connect.
    pub fn new(url: impl Into<String>, http: reqwest::Client, token: impl Fn() -> Option<String> + Send + Sync + 'static) -> Self {
        Self {
            shared: Arc::new(Shared {
                url: url.into(),
                http,
                token: Box::new(token),
                reconnected: Mutex::new(None),
                link: Mutex::new(Link { state: HubState::Disconnected, generation: 0, sender: None, pending: HashMap::new() }),
                next_id: AtomicU64::new(0),
            }),
        }
    }

    pub fn on_reconnected(&self, callback: impl Fn() + Send + Sync + 'static) {
        *self.shared.reconnected.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn state(&self) -> HubState {
        self.shared.link().state
    }

    pub async fn start(&self) -> Result<(), HubError> {
        let generation = {
            let mut link = self.shared.link();
            if link.state != HubState::Disconnected {
                return Err(HubError::NotConnected);
            }
            link.state = HubState::Connecting;
            link.generation += 1;
            link.generation
        };
        match self.shared.connect().await {
            Ok((socket, leftover)) => {
                let mut link = self.shared.link();
                if link.generation != generation {
                    return Err(HubError::Closed("The connection was stopped while it started.".into()));
                }
                let (sender, receiver) = mpsc::unbounded_channel();
                link.sender = Some(sender);
                link.state = HubState::Connected;
                tokio::spawn(Shared::run(self.shared.clone(), generation, socket, receiver, leftover));
                Ok(())
            }
            Err(error) => {
                let mut link = self.shared.link();
                if link.generation == generation {
                    link.state = HubState::Disconnected;
                }
                Err(error)
            }
        }
    }

    /// Calls a hub method and waits for its completion.
    pub async fn invoke<T: DeserializeOwned>(&self, target: &str, arguments: impl Serialize) -> Result<T, HubError> {
        let (sender, receiver) = oneshot::channel();
        let id = {
            let mut link = self.shared.link();
            let Some(channel) = link.sender.clone().filter(|_| link.state == HubState::Connected) else { return Err(HubError::NotConnected) };
            let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed).to_string();
            let message = json!({"type": 1, "invocationId": id, "target": target, "arguments": arguments});
            link.pending.insert(id.clone(), sender);
            if channel.send(format!("{message}{SEPARATOR}")).is_err() {
                link.pending.remove(&id);
                return Err(HubError::NotConnected);
            }
            id
        };
        let Ok(answer) = tokio::time::timeout(INVOKE_TIMEOUT, receiver).await else {
            self.shared.link().pending.remove(&id);
            return Err(HubError::Timeout);
        };
        let result = answer.map_err(|_| HubError::Closed("The connection was stopped.".into()))??;
        serde_json::from_value(result).map_err(|error| HubError::Hub(format!("Unexpected result: {error}")))
    }

    /// Closes the connection and stops reconnecting; pending invocations fail. The hub can be started again.
    pub async fn stop(&self) {
        let pending = {
            let mut link = self.shared.link();
            link.generation += 1;
            link.state = HubState::Disconnected;
            link.sender = None;
            std::mem::take(&mut link.pending)
        };
        for (_, completion) in pending {
            let _ = completion.send(Err(HubError::Closed("The connection was stopped.".into())));
        }
    }
}

fn header(value: &str) -> Result<HeaderValue, HubError> {
    HeaderValue::from_str(value).map_err(|error| HubError::Http(error.to_string()))
}

/// A frame to the socket, or why it could not go (a link that stopped taking data counts).
async fn send(socket: &mut Socket, message: Message) -> Result<(), String> {
    match tokio::time::timeout(SEND_TIMEOUT, socket.send(message)).await {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => Err("The connection stopped taking data.".into()),
    }
}

impl Shared {
    fn link(&self) -> MutexGuard<'_, Link> {
        self.link.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Negotiate, open the WebSocket and do the handshake; returns the socket and what arrived after the handshake.
    async fn connect(&self) -> Result<(Socket, String), HubError> {
        let mut url = self.url.trim_end_matches('/').to_owned();
        let mut token = (self.token)();
        let mut negotiated = None;
        for _ in 0..5 {
            let mut request = self.http.post(format!("{url}/negotiate?negotiateVersion=1")).timeout(Duration::from_secs(15));
            if let Some(token) = &token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.map_err(|error| HubError::Http(error.to_string()))?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Err(HubError::Unauthorized);
            }
            if !response.status().is_success() {
                return Err(HubError::Http(format!("Response status code does not indicate success: {}.", response.status().as_u16())));
            }
            let body: Value = response.json().await.map_err(|error| HubError::Http(error.to_string()))?;
            if let Some(error) = body.get("error").and_then(Value::as_str) {
                return Err(HubError::Http(error.into()));
            }
            // Azure SignalR answers with the service to connect to instead.
            if let Some(redirect) = body.get("url").and_then(Value::as_str) {
                url = redirect.trim_end_matches('/').to_owned();
                token = body.get("accessToken").and_then(Value::as_str).map(str::to_owned).or(token);
                continue;
            }
            let websockets = body.get("availableTransports").and_then(Value::as_array).is_some_and(|transports| {
                transports.iter().any(|transport| transport.get("transport").and_then(Value::as_str) == Some("WebSockets"))
            });
            if !websockets {
                return Err(HubError::Http("The server does not support WebSockets.".into()));
            }
            negotiated = body.get("connectionToken").or_else(|| body.get("connectionId")).and_then(Value::as_str).map(str::to_owned);
            break;
        }
        let Some(id) = negotiated else { return Err(HubError::Http("Negotiate redirected too many times.".into())) };
        let address = url.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
        let separator = if address.contains('?') { '&' } else { '?' };
        let mut request = format!("{address}{separator}id={}", crate::escape_data_string(&id)).into_client_request().map_err(|error| HubError::Http(error.to_string()))?;
        if let Some(token) = &token {
            request.headers_mut().insert("Authorization", header(&format!("Bearer {token}"))?);
        }
        let opened = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            let (mut socket, _) = tokio_tungstenite::connect_async(request).await.map_err(|error| HubError::Http(error.to_string()))?;
            socket.send(Message::text(format!("{{\"protocol\":\"json\",\"version\":1}}{SEPARATOR}"))).await.map_err(|error| HubError::Http(error.to_string()))?;
            let mut buffer = String::new();
            loop {
                match socket.next().await {
                    Some(Ok(Message::Text(text))) => buffer.push_str(&text),
                    Some(Ok(Message::Binary(bytes))) => buffer.push_str(&String::from_utf8_lossy(&bytes)),
                    Some(Ok(Message::Close(_))) | None => return Err(HubError::Http("The server closed the connection during the handshake.".into())),
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => return Err(HubError::Http(error.to_string())),
                }
                if let Some((handshake, rest)) = buffer.split_once(SEPARATOR) {
                    let answer: Value = serde_json::from_str(handshake).map_err(|error| HubError::Http(error.to_string()))?;
                    if let Some(error) = answer.get("error").and_then(Value::as_str) {
                        return Err(HubError::Http(format!("Unable to complete handshake with the server: {error}")));
                    }
                    let rest = rest.to_owned();
                    return Ok((socket, rest));
                }
            }
        })
        .await;
        opened.unwrap_or(Err(HubError::Timeout))
    }

    /// Owns one connection until it ends, then reconnects while this generation still owns the hub.
    async fn run(self: Arc<Self>, generation: u64, mut socket: Socket, mut receiver: mpsc::UnboundedReceiver<String>, mut buffer: String) {
        loop {
            let ended = self.pump(&mut socket, &mut receiver, &mut buffer).await;
            let _ = socket.close(None).await;
            let reconnect = {
                let mut link = self.link();
                if link.generation != generation {
                    return;
                }
                let reason = match &ended {
                    Ended::Stopped => "The connection was stopped.".to_owned(),
                    Ended::Closed { error, .. } => error.clone().unwrap_or_else(|| "The server closed the connection.".into()),
                    Ended::Lost(reason) => reason.clone(),
                };
                for (_, completion) in std::mem::take(&mut link.pending) {
                    let _ = completion.send(Err(HubError::Closed(reason.clone())));
                }
                link.sender = None;
                let reconnect = matches!(ended, Ended::Lost(_) | Ended::Closed { reconnect: true, .. });
                link.state = if reconnect { HubState::Reconnecting } else { HubState::Disconnected };
                reconnect
            };
            if !reconnect {
                return;
            }
            let mut connected = None;
            for delay in RECONNECT_DELAYS {
                tokio::time::sleep(Duration::from_secs(delay)).await;
                if self.link().generation != generation {
                    return;
                }
                if let Ok(next) = self.connect().await {
                    connected = Some(next);
                    break;
                }
            }
            let Some((next, leftover)) = connected else {
                let mut link = self.link();
                if link.generation == generation {
                    link.state = HubState::Disconnected;
                }
                return;
            };
            {
                let mut link = self.link();
                if link.generation != generation {
                    return;
                }
                let (sender, next_receiver) = mpsc::unbounded_channel();
                link.sender = Some(sender);
                link.state = HubState::Connected;
                receiver = next_receiver;
            }
            socket = next;
            buffer = leftover;
            if let Some(callback) = self.reconnected.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() {
                callback();
            }
        }
    }

    async fn pump(&self, socket: &mut Socket, receiver: &mut mpsc::UnboundedReceiver<String>, buffer: &mut String) -> Ended {
        let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + KEEP_ALIVE, KEEP_ALIVE);
        let mut heard = tokio::time::Instant::now();
        loop {
            if let Some(ended) = self.dispatch(buffer) {
                return ended;
            }
            tokio::select! {
                outgoing = receiver.recv() => match outgoing {
                    Some(text) => if let Err(error) = send(socket, Message::text(text)).await { return Ended::Lost(error) },
                    None => return Ended::Stopped,
                },
                incoming = socket.next() => {
                    heard = tokio::time::Instant::now();
                    match incoming {
                        Some(Ok(Message::Text(text))) => buffer.push_str(&text),
                        Some(Ok(Message::Binary(bytes))) => buffer.push_str(&String::from_utf8_lossy(&bytes)),
                        Some(Ok(Message::Close(_))) | None => return Ended::Lost("The server closed the WebSocket.".into()),
                        Some(Ok(_)) => {}
                        Some(Err(error)) => return Ended::Lost(error.to_string()),
                    }
                }
                _ = ping.tick() => {
                    if let Err(error) = send(socket, Message::text(format!("{{\"type\":6}}{SEPARATOR}"))).await {
                        return Ended::Lost(error);
                    }
                }
                _ = tokio::time::sleep_until(heard + SERVER_TIMEOUT) => {
                    return Ended::Lost(format!("Server timeout ({}ms) elapsed without receiving a message from the server.", SERVER_TIMEOUT.as_millis()));
                }
            }
        }
    }

    /// Handles every complete message in the buffer; stops at a close message.
    fn dispatch(&self, buffer: &mut String) -> Option<Ended> {
        while let Some(end) = buffer.find(SEPARATOR) {
            let record: String = buffer.drain(..end + SEPARATOR.len_utf8()).collect();
            let Ok(message) = serde_json::from_str::<Value>(&record[..end]) else { continue };
            match message.get("type").and_then(Value::as_i64) {
                Some(3) => {
                    let Some(id) = message.get("invocationId").and_then(Value::as_str) else { continue };
                    let Some(completion) = self.link().pending.remove(id) else { continue };
                    let result = match message.get("error").and_then(Value::as_str) {
                        Some(error) => Err(HubError::Hub(error.into())),
                        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = completion.send(result);
                }
                Some(7) => {
                    let error = message.get("error").and_then(Value::as_str).map(|error| format!("The server closed the connection with the following error: {error}"));
                    let reconnect = message.get("allowReconnect").and_then(Value::as_bool).unwrap_or(false);
                    return Some(Ended::Closed { error, reconnect });
                }
                // Pings, and invocations of client methods this client has none of.
                _ => {}
            }
        }
        None
    }
}

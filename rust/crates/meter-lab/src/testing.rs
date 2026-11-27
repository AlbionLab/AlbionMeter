//! A fake AlbionLab site on 127.0.0.1 for the tests: plain HTTP answered by a closure, and a SignalR hub on the same
//! port that does the handshake and pings itself and hands every invocation to another closure.

use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{Request as Upgrade, Response};
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: String,
}

pub type Http = Arc<dyn Fn(Request) -> BoxFuture<'static, (u16, Vec<u8>)> + Send + Sync>;
/// An invocation message → the completion's `result` (Ok) or `error` (Err); None sends no completion.
pub type Invoke = Arc<dyn Fn(Value) -> BoxFuture<'static, Option<Result<Value, String>>> + Send + Sync>;

#[derive(Default)]
pub struct Seen {
    pub requests: Vec<Request>,
    /// The Authorization header and the query of every WebSocket the hub accepted.
    pub sockets: Vec<(Option<String>, String)>,
    pub frames: Vec<String>,
}

pub struct FakeSite {
    pub address: String,
    pub seen: Arc<Mutex<Seen>>,
    /// Raw text frames to send to the current hub connection.
    pub push: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
}

impl FakeSite {
    pub async fn start(http: Http, invoke: Invoke) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Seen::default()));
        let push = Arc::new(Mutex::new(None));
        let (site_seen, site_push) = (seen.clone(), push.clone());
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                let (http, invoke, seen, push) = (http.clone(), invoke.clone(), site_seen.clone(), site_push.clone());
                tokio::spawn(async move {
                    let mut peek = [0u8; 2048];
                    let read = stream.peek(&mut peek).await.unwrap_or(0);
                    if String::from_utf8_lossy(&peek[..read]).to_ascii_lowercase().contains("upgrade: websocket") {
                        hub(stream, invoke, seen, push).await;
                    } else {
                        serve(stream, http, seen).await;
                    }
                });
            }
        });
        Self { address, seen, push }
    }

    pub fn send(&self, frame: &str) {
        if let Some(push) = self.push.lock().unwrap().as_ref() {
            let _ = push.send(frame.to_owned());
        }
    }
}

async fn serve(mut stream: TcpStream, http: Http, seen: Arc<Mutex<Seen>>) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let Ok(read) = stream.read(&mut chunk).await else { return };
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (method, path) = (first.next().unwrap_or_default().to_owned(), first.next().unwrap_or_default().to_owned());
    let header = |name: &str| head.lines().find_map(|line| line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.trim().to_owned()));
    let length: usize = header("content-length").and_then(|value| value.parse().ok()).unwrap_or(0);
    while buffer.len() < head_end + length {
        let Ok(read) = stream.read(&mut chunk).await else { return };
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&buffer[head_end..(head_end + length).min(buffer.len())]).into_owned();
    let request = Request { method, path, authorization: header("authorization"), body };
    seen.lock().unwrap().requests.push(request.clone());
    let (status, body) = http(request).await;
    let head = format!("HTTP/1.1 {status} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(&[head.into_bytes(), body].concat()).await;
    let _ = stream.shutdown().await;
}

#[allow(clippy::result_large_err)] // tungstenite's callback signature
async fn hub(stream: TcpStream, invoke: Invoke, seen: Arc<Mutex<Seen>>, push: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>) {
    let accepted = seen.clone();
    let callback = move |request: &Upgrade, response: Response| {
        let authorization = request.headers().get("authorization").and_then(|value| value.to_str().ok()).map(str::to_owned);
        accepted.lock().unwrap().sockets.push((authorization, request.uri().query().unwrap_or_default().to_owned()));
        Ok(response)
    };
    let Ok(socket) = tokio_tungstenite::accept_hdr_async(stream, callback).await else { return };
    let (mut sink, mut source) = socket.split();
    let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
    *push.lock().unwrap() = Some(sender.clone());
    let mut buffer = String::new();
    let mut handshaken = false;
    loop {
        tokio::select! {
            frame = receiver.recv() => {
                let Some(frame) = frame else { return };
                if frame == "<close>" {
                    let _ = sink.close().await;
                    return;
                }
                if sink.send(Message::text(frame)).await.is_err() { return }
            }
            message = source.next() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text.to_string(),
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(_)) => continue,
                };
                seen.lock().unwrap().frames.push(text.clone());
                buffer.push_str(&text);
                while let Some(end) = buffer.find('\u{1e}') {
                    let record: String = buffer.drain(..end + 1).collect();
                    let message: Value = serde_json::from_str(&record[..end]).unwrap();
                    if !handshaken {
                        assert_eq!(message, json!({"protocol": "json", "version": 1}));
                        handshaken = true;
                        let _ = sender.send("{}\u{1e}".into());
                        continue;
                    }
                    if message["type"] == 1 {
                        let (invoke, sender) = (invoke.clone(), sender.clone());
                        tokio::spawn(async move {
                            let id = message["invocationId"].clone();
                            let completion = match invoke(message).await {
                                Some(Ok(result)) => json!({"type": 3, "invocationId": id, "result": result}),
                                Some(Err(error)) => json!({"type": 3, "invocationId": id, "error": error}),
                                None => return,
                            };
                            let _ = sender.send(format!("{completion}\u{1e}"));
                        });
                    }
                }
            }
        }
    }
}

/// Answers negotiate for the hub; everything else goes to `rest`.
pub fn negotiate_then(rest: impl Fn(Request) -> (u16, String) + Send + Sync + 'static) -> Http {
    let rest = Arc::new(rest);
    Arc::new(move |request: Request| {
        let rest = rest.clone();
        Box::pin(async move {
            if request.path.contains("/negotiate") {
                (200, json!({"negotiateVersion": 1, "connectionId": "c", "connectionToken": "token one", "availableTransports": [{"transport": "WebSockets", "transferFormats": ["Text", "Binary"]}]}).to_string().into_bytes())
            } else {
                let (status, body) = rest(request);
                (status, body.into_bytes())
            }
        })
    })
}

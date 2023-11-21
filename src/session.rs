use crate::config::WebsocketConfig;
use crate::server::Disconnected;
use crate::Error;
use crate::Message;
use crate::Socket;
use crate::{CloseCode, CloseFrame};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::fmt::Formatter;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

#[async_trait]
#[allow(unused_variables)]
pub trait SessionExt: Send {
    /// Custom identification number of SessionExt, usually a number or a string.
    type ID: Send + Sync + Clone + std::fmt::Debug + std::fmt::Display;
    /// Type the custom call - parameters passed to `on_call`.
    type Call: Send;

    /// Returns ID of the session.
    fn id(&self) -> &Self::ID;
    /// Handler for text messages from the client.
    async fn on_text(&mut self, text: String) -> Result<(), Error> {
        Ok(())
    }
    /// Handler for binary messages from the client.
    async fn on_binary(&mut self, bytes: Vec<u8>) -> Result<(), Error> {
        Ok(())
    }
    /// Handler for custom calls from other parts from your program.
    /// This is useful for concurrency and polymorphism.
    async fn on_call(&mut self, call: Self::Call) -> Result<(), Error> {
        Ok(())
    }
    /// Handler for disconnection of the client.
    async fn on_disconnect(
        &mut self,
        result: &Result<Option<CloseFrame>, Error>,
    ) -> Result<(), Error> {
        Ok(())
    }
}

pub struct Session<I, C> {
    pub id: I,
    socket: mpsc::Sender<Message>,
    calls: mpsc::Sender<C>,
}

impl<I: std::fmt::Debug, C> std::fmt::Debug for Session<I, C> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl<I: Clone, C> Clone for Session<I, C> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            socket: self.socket.clone(),
            calls: self.calls.clone(),
        }
    }
}

impl<I: std::fmt::Display + Clone + Send + 'static, C: Send> Session<I, C> {
    pub fn create<S: SessionExt<ID = I, Call = C> + 'static>(
        session_id: I,
        socket: Socket,
        config: &WebsocketConfig,
        session_fn: impl FnOnce(Session<I, C>) -> S,
    ) -> Self {
        let (socket_sender, socket_receiver) = mpsc::channel(config.channel_size);
        let (call_sender, call_receiver) = mpsc::channel(config.channel_size);

        let handle = Self {
            id: session_id.clone(),
            socket: socket_sender,
            calls: call_sender,
        };
        let session = session_fn(handle.clone());
        let actor = SessionActor {
            extension: session,
            id: session_id,
            socket_receiver,
            call_receiver,
            socket,
            last_alive: Instant::now(),
        };
        tokio::spawn(async move {
            let tx = actor.socket.disconnected.clone().unwrap();
            let id = actor.id.clone();
            let result = actor.run().await;
            tx.send(Box::new(Disconnected::<I> { id, result }))
        });
        handle
    }
}

impl<I: std::fmt::Display + Clone, C> Session<I, C> {
    /// Checks if the Session is still alive, if so you can proceed sending calls or messages.
    pub fn alive(&self) -> bool {
        !self.socket.is_closed() && !self.calls.is_closed()
    }

    /// Sends a Text message to the server
    pub async fn text(&self, text: String) {
        self.socket
            .send(Message::Text(text))
            .await
            .unwrap_or_else(|_| tracing::warn!("Session::text {PANIC_MESSAGE_UNHANDLED_CLOSE}"));
    }

    /// Sends a Binary message to the server
    pub async fn binary(&self, bytes: Vec<u8>) {
        self.socket
            .send(Message::Binary(bytes))
            .await
            .unwrap_or_else(|_| tracing::warn!("Session::binary {PANIC_MESSAGE_UNHANDLED_CLOSE}"));
    }

    /// Calls a method on the session
    pub async fn call(&self, call: C) {
        self.calls
            .send(call)
            .await
            .unwrap_or_else(|_| tracing::warn!("Session::call {PANIC_MESSAGE_UNHANDLED_CLOSE}"));
    }

    /// Calls a method on the session, allowing the Session to respond with oneshot::Sender.
    /// This is just for easier construction of the call which happen to contain oneshot::Sender in it.
    pub async fn call_with<R: std::fmt::Debug>(
        &self,
        f: impl FnOnce(oneshot::Sender<R>) -> C,
    ) -> Option<R> {
        let (sender, receiver) = oneshot::channel();
        let call = f(sender);

        self.calls.send(call).await.ok()?;
        receiver.await.ok()
    }
}

pub(crate) struct SessionActor<E: SessionExt> {
    pub extension: E,
    id: E::ID,
    socket_receiver: mpsc::Receiver<Message>,
    call_receiver: mpsc::Receiver<E::Call>,
    socket: Socket,
    last_alive: Instant,
}

impl<E: SessionExt> SessionActor<E> {
    pub(crate) async fn run(mut self) -> Result<Option<CloseFrame>, Error> {
        let ret = self.run_inner().await;
        self.extension.on_disconnect(&ret).await?;
        ret
    }

    async fn handle_timeout_ping(&mut self) -> Result<bool, Error> {
        if self.last_alive.elapsed() > self.socket.config.timeout {
            tracing::info!("closing connection due to timeout");
            self.socket
                .stream
                .send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Normal,
                    reason: String::from("client didn't respond to Ping frame"),
                })))
                .await?;
            return Ok(true);
        }
        let timestamp = Utc::now().timestamp_micros();
        let bytes = timestamp.to_be_bytes();
        self.socket
            .stream
            .send(Message::Ping(bytes.to_vec()))
            .await?;
        Ok(false)
    }
    async fn handle_send_message(&mut self, message: Message) -> Result<(), Error> {
        self.socket.stream.send(message).await?;

        Ok(())
    }
    async fn handle_recv_message(
        &mut self,
        message: Message,
    ) -> Result<Option<Option<CloseFrame>>, Error> {
        let result = match message {
            Message::Text(text) => self.extension.on_text(text).await,
            Message::Binary(bytes) => self.extension.on_binary(bytes).await,
            Message::Pong(_pong) => {
                if let Ok(bytes) = _pong.try_into() {
                    let bytes: [u8; 8] = bytes;
                    let timestamp = i64::from_be_bytes(bytes);
                    let timestamp = Utc.timestamp_micros(timestamp).single().unwrap();
                    let latency = (Utc::now() - timestamp).num_milliseconds();
                    tracing::trace!("latency: {}ms", latency);
                }
                self.last_alive = Instant::now();

                Ok(())
            }
            Message::Close(frame) => return Ok(Some(frame.map(CloseFrame::from))),
            _ => Ok(()),
        };
        if let Err(err) = result {
            tracing::error!(id = %self.id, "error while handling message: {error}", error = err);
            while let Some(msg) = self.socket_receiver.try_recv().ok() {
                self.socket.stream.send(msg).await?;
            }
            self.socket
                .stream
                .send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Error,
                    reason: format!("{}", err),
                })))
                .await?;
            return Err(err.into());
        }
        Ok(None)
    }

    pub(crate) async fn run_inner(&mut self) -> Result<Option<CloseFrame>, Error> {
        let mut interval = tokio::time::interval(self.socket.config.heartbeat);
        self.last_alive = Instant::now();
        loop {
            tokio::select! {
                _tick = interval.tick() => {
                    if self.handle_timeout_ping().await? {
                        break;
                    }
                }
                Some(message) = self.socket_receiver.recv() => {
                    let close = if let Message::Close(frame) = &message {
                        Some(Ok(frame.clone()))
                    } else {
                        None
                    };
                    self.handle_send_message(message).await?;
                    if let Some(frame) = close {
                        return frame
                    }
                }
                Some(call) = self.call_receiver.recv() => {
                    self.extension.on_call(call).await?;
                }
                message = self.socket.stream.next() => {
                    match message {
                        Some(Ok(message)) => {
                            if let Some(frame) = self.handle_recv_message(message).await? {
                                return Ok(frame)
                            }
                        }
                        Some(Err(error)) => {
                            tracing::error!(id = %self.id, "connection error: {error}");
                            return Err(error.into())
                        }
                        None => {
                            tracing::debug!(id = %self.id, "connection closed");
                            break
                        }
                    };
                }
            }
        }
        Ok(None)
    }
}

const PANIC_MESSAGE_UNHANDLED_CLOSE: &str = "should not be called after Session close. Try handling Server::disconnect or Session::drop, also you can check whether the Session is alive using Session::alive";

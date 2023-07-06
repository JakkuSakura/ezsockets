use crate::tungstenite::tungstenite::handshake::server::ErrorResponse;
use crate::CloseCode;
use crate::CloseFrame;
use crate::Error;
use crate::Message;
use crate::Request;
use crate::Server;
use crate::ServerExt;
use crate::Socket;
use tokio_tungstenite::tungstenite;
use tungstenite::protocol::frame::coding::CloseCode as TungsteniteCloseCode;

use crate::config::WebsocketConfig;
use crate::server::CreateServer;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tracing::info;

impl<'t> From<tungstenite::protocol::CloseFrame<'t>> for CloseFrame {
    fn from(frame: tungstenite::protocol::CloseFrame) -> Self {
        Self {
            code: frame.code.into(),
            reason: frame.reason.into(),
        }
    }
}

impl<'t> From<CloseFrame> for tungstenite::protocol::CloseFrame<'t> {
    fn from(frame: CloseFrame) -> Self {
        Self {
            code: frame.code.into(),
            reason: frame.reason.into(),
        }
    }
}

impl From<CloseCode> for TungsteniteCloseCode {
    fn from(code: CloseCode) -> Self {
        match code {
            CloseCode::Normal => Self::Normal,
            CloseCode::Away => Self::Away,
            CloseCode::Protocol => Self::Protocol,
            CloseCode::Unsupported => Self::Unsupported,
            CloseCode::Status => Self::Status,
            CloseCode::Abnormal => Self::Abnormal,
            CloseCode::Invalid => Self::Invalid,
            CloseCode::Policy => Self::Policy,
            CloseCode::Size => Self::Size,
            CloseCode::Extension => Self::Extension,
            CloseCode::Error => Self::Error,
            CloseCode::Restart => Self::Restart,
            CloseCode::Again => Self::Again,
        }
    }
}

impl From<TungsteniteCloseCode> for CloseCode {
    fn from(code: TungsteniteCloseCode) -> Self {
        match code {
            TungsteniteCloseCode::Normal => Self::Normal,
            TungsteniteCloseCode::Away => Self::Away,
            TungsteniteCloseCode::Protocol => Self::Protocol,
            TungsteniteCloseCode::Unsupported => Self::Unsupported,
            TungsteniteCloseCode::Status => Self::Status,
            TungsteniteCloseCode::Abnormal => Self::Abnormal,
            TungsteniteCloseCode::Invalid => Self::Invalid,
            TungsteniteCloseCode::Policy => Self::Policy,
            TungsteniteCloseCode::Size => Self::Size,
            TungsteniteCloseCode::Extension => Self::Extension,
            TungsteniteCloseCode::Error => Self::Error,
            TungsteniteCloseCode::Restart => Self::Restart,
            TungsteniteCloseCode::Again => Self::Again,
            code => unimplemented!("could not handle close code: {code:?}"),
        }
    }
}

impl From<Message> for tungstenite::Message {
    fn from(message: Message) -> Self {
        match message {
            Message::Text(text) => Self::Text(text),
            Message::Binary(bytes) => Self::Binary(bytes),
            Message::Ping(bytes) => Self::Ping(bytes),
            Message::Pong(bytes) => Self::Pong(bytes),
            Message::Close(frame) => Self::Close(frame.map(CloseFrame::into)),
        }
    }
}

impl From<tungstenite::Message> for Message {
    fn from(message: tungstenite::Message) -> Self {
        match message {
            tungstenite::Message::Text(text) => Self::Text(text),
            tungstenite::Message::Binary(bytes) => Self::Binary(bytes),
            tungstenite::Message::Ping(bytes) => Self::Ping(bytes),
            tungstenite::Message::Pong(bytes) => Self::Pong(bytes),
            tungstenite::Message::Close(frame) => Self::Close(frame.map(CloseFrame::from)),
            tungstenite::Message::Frame(_) => unreachable!(),
        }
    }
}

pub enum Acceptor {
    Plain,
    #[cfg(feature = "native-tls")]
    NativeTls(tokio_native_tls::TlsAcceptor),
    #[cfg(feature = "rustls")]
    Rustls(tokio_rustls::TlsAcceptor),
}

impl Acceptor {
    async fn accept(
        &self,
        stream: TcpStream,
        config: WebsocketConfig,
    ) -> Result<(Socket, Request), Error> {
        let mut req0 = None;
        let callback = |req: &http::Request<()>,
                        resp: http::Response<()>|
         -> Result<http::Response<()>, ErrorResponse> {
            let mut req1 = Request::builder()
                .method(req.method().clone())
                .uri(req.uri().clone())
                .version(req.version());
            for (k, v) in req.headers() {
                req1 = req1.header(k, v);
            }
            req0 = Some(req1.body(()).unwrap());

            Ok(resp)
        };
        let socket = match self {
            Acceptor::Plain => {
                let socket = tokio_tungstenite::accept_hdr_async(stream, callback).await?;
                Socket::new(socket, config)
            }
            #[cfg(feature = "native-tls")]
            Acceptor::NativeTls(acceptor) => {
                let tls_stream = acceptor.accept(stream).await?;
                let socket = tokio_tungstenite::accept_hdr_async(tls_stream, callback).await?;
                Socket::new(socket, config)
            }
            #[cfg(feature = "rustls")]
            Acceptor::Rustls(acceptor) => {
                let tls_stream = acceptor.accept(stream).await?;
                let socket = tokio_tungstenite::accept_hdr_async(tls_stream, callback).await?;
                Socket::new(socket, config)
            }
        };
        Ok((socket, req0.unwrap()))
    }
}

async fn run_acceptor<E>(
    server: Server<E>,
    listener: TcpListener,
    acceptor: Acceptor,
    config: WebsocketConfig,
) -> Result<(), Error>
where
    E: ServerExt + 'static,
{
    loop {
        // TODO: Find a better way without those stupid matches
        let (stream, address) = match listener.accept().await {
            Ok(stream) => stream,
            Err(err) => {
                tracing::error!("failed to accept tcp connection: {:?}", err);
                continue;
            }
        };
        info!("Accepted connection from {}", address);
        let (socket, request) = match acceptor.accept(stream, config.clone()).await {
            Ok(socket) => socket,
            Err(err) => {
                tracing::error!(%address, "failed to accept websocket connection: {:?}", err);
                continue;
            }
        };
        info!("Accepted websocket connection from {}", address);
        let _ = server.accept(socket, request, address).await;
    }
}

// Run the server
pub async fn run<E>(
    config: WebsocketConfig,
    acceptor: Acceptor,
    server: CreateServer<E>,
) -> Result<(), Error>
where
    E: ServerExt + 'static,
{
    info!("Starting websocket server on {}", config.address);
    let listener = TcpListener::bind(&config.address).await?;
    let (server, _) = server.create(config.clone());
    run_acceptor(server, listener, acceptor, config).await
}

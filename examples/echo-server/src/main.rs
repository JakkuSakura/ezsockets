use async_trait::async_trait;
use ezsockets::config::WebsocketConfig;
use ezsockets::server::CreateServer;
use ezsockets::tungstenite::Acceptor;
use ezsockets::Error;
use ezsockets::Request;
use ezsockets::Socket;
use std::net::SocketAddr;

type SessionID = u16;
type Session = ezsockets::Session<SessionID, ()>;

struct EchoServer {}

#[async_trait]
impl ezsockets::ServerExt for EchoServer {
    type Session = EchoSession;
    type Call = ();

    async fn on_connect(
        &mut self,
        socket: Socket,
        _request: Request,
        address: SocketAddr,
        config: &WebsocketConfig,
    ) -> Result<Session, Error> {
        let id = address.port();
        let session = Session::create(id, socket, config, |handle| EchoSession { id, handle });
        Ok(session)
    }

    async fn on_disconnect(
        &mut self,
        _id: <Self::Session as ezsockets::SessionExt>::ID,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn on_call(&mut self, call: Self::Call) -> Result<(), Error> {
        let () = call;
        Ok(())
    }
}

struct EchoSession {
    handle: Session,
    id: SessionID,
}

#[async_trait]
impl ezsockets::SessionExt for EchoSession {
    type ID = SessionID;
    type Call = ();

    fn id(&self) -> &Self::ID {
        &self.id
    }

    async fn on_text(&mut self, text: String) -> Result<(), Error> {
        self.handle.text(text).await;
        Ok(())
    }

    async fn on_binary(&mut self, _bytes: Vec<u8>) -> Result<(), Error> {
        unimplemented!()
    }

    async fn on_call(&mut self, call: Self::Call) -> Result<(), Error> {
        let () = call;
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let server = CreateServer::new(|_server| EchoServer {});
    ezsockets::tungstenite::run(
        WebsocketConfig {
            address: "127.0.0.1:8080".to_string(),
            ..WebsocketConfig::default()
        },
        Acceptor::Plain,
        server,
    )
    .await
    .unwrap();
}

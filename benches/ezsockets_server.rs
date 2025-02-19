use std::net::SocketAddr;

use async_trait::async_trait;
use ezsockets::config::WebsocketConfig;
use ezsockets::server::CreateServer;
use ezsockets::tungstenite::Acceptor;

type SessionID = u16;
type Session = ezsockets::Session<SessionID, ()>;

struct EchoServer {}

#[async_trait]
impl ezsockets::ServerExt for EchoServer {
    type Session = EchoSession;
    type Call = ();

    async fn on_connect(
        &mut self,
        socket: ezsockets::Socket,
        _request: ezsockets::Request,
        address: SocketAddr,
        config: &WebsocketConfig,
    ) -> Result<Session, ezsockets::Error> {
        let id = address.port();
        let session = Session::create(id, socket, config, |handle| EchoSession { id, handle });
        Ok(session)
    }

    async fn on_disconnect(
        &mut self,
        _id: <Self::Session as ezsockets::SessionExt>::ID,
    ) -> Result<(), ezsockets::Error> {
        Ok(())
    }

    async fn on_call(&mut self, call: Self::Call) -> Result<(), ezsockets::Error> {
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

    async fn on_text(&mut self, text: String) -> Result<(), ezsockets::Error> {
        self.handle.text(text).await;
        Ok(())
    }

    async fn on_binary(&mut self, _bytes: Vec<u8>) -> Result<(), ezsockets::Error> {
        unimplemented!()
    }

    async fn on_call(&mut self, call: Self::Call) -> Result<(), ezsockets::Error> {
        let () = call;
        Ok(())
    }
}

pub async fn run() {
    let config = WebsocketConfig::default();
    let server = CreateServer::new(|_| EchoServer {});
    ezsockets::tungstenite::run(config, Acceptor::Plain, server)
        .await
        .unwrap();
}

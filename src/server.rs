use crate::config::WebsocketConfig;
use crate::CloseFrame;
use crate::Error;
use crate::Request;
use crate::Session;
use crate::SessionExt;
use crate::Socket;
use async_trait::async_trait;
use std::any::Any;
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

struct NewConnection<E: ServerExt> {
    socket: Socket,
    address: SocketAddr,
    request: Request,
    respond_to: oneshot::Sender<<E::Session as SessionExt>::ID>,
}

pub(crate) struct Disconnected<I> {
    pub(crate) id: I,
    pub(crate) result: Result<Option<CloseFrame>, Error>,
}

struct ServerActor<E: ServerExt> {
    connections: mpsc::UnboundedReceiver<NewConnection<E>>,
    disconnections: mpsc::UnboundedReceiver<Box<dyn Any + Send>>,
    disconnections_tx: mpsc::UnboundedSender<Box<dyn Any + Send>>,
    calls: mpsc::UnboundedReceiver<E::Call>,
    extension: E,
    config: WebsocketConfig,
}

impl<E: ServerExt> ServerActor<E>
where
    E: Send + 'static,
    <E::Session as SessionExt>::ID: Send,
{
    async fn run(mut self) {
        loop {
            if let Err(err) = async {
                tokio::select! {
                    Some(NewConnection{mut socket, address, respond_to, request}) = self.connections.recv() => {
                        socket.disconnected = Some(self.disconnections_tx.clone());
                        let session = self.extension.on_connect(socket, request, address, &self.config).await?;
                        let session_id = session.id.clone();
                        tracing::info!("connection from {address} accepted");
                        let _ = respond_to.send(session_id.clone());


                    }
                    Some(x) = self.disconnections.recv() => {
                        let Disconnected{id, result}: Disconnected<<E::Session as SessionExt>::ID> = *x.downcast().unwrap();
                        self.extension.on_disconnect(id.clone()).await?;
                        match result {
                            Ok(Some(CloseFrame { code, reason })) => {
                                tracing::info!(%id, ?code, %reason, "connection closed")
                            }
                            Ok(None) => tracing::info!(%id, "connection closed"),
                            Err(err) => tracing::warn!(%id, "connection closed due to: {err:?}"),
                        };
                    }
                    Some(call) = self.calls.recv() => {
                        if let Err(err) = self.extension.on_call(call).await {
                            tracing::error!("Error when calling {:?}", err);
                        }
                    }
                }
                Ok::<_, Error>(())
            }
                .await {
                tracing::error!("error when processing: {err:?}");
            }
        }
    }
}

#[async_trait]
pub trait ServerExt: Send {
    /// Type of the session that will be created for each connection.
    type Session: SessionExt;
    /// Type the custom call - parameters passed to `on_call`.
    type Call: Send;

    /// Called when client connects to the server.
    /// Here you should create a `Session` with your own implementation of `SessionExt` and return it.
    ///If you don't want to accept the connection, return an error.
    async fn on_connect(
        &mut self,
        socket: Socket,
        request: Request,
        address: SocketAddr,
        config: &WebsocketConfig,
    ) -> Result<
        Session<<Self::Session as SessionExt>::ID, <Self::Session as SessionExt>::Call>,
        Error,
    >;
    /// Called when client disconnects from the server.
    async fn on_disconnect(&mut self, id: <Self::Session as SessionExt>::ID) -> Result<(), Error>;
    /// Handler for custom calls from other parts from your program.
    /// This is useful for concurrency and polymorphism.
    async fn on_call(&mut self, call: Self::Call) -> Result<(), Error>;
}
pub struct CreateServer<E: ServerExt> {
    create_fn: Box<dyn FnOnce(Server<E>) -> E + Send>,
}
impl<E: ServerExt + 'static> CreateServer<E> {
    pub fn new(create_fn: impl FnOnce(Server<E>) -> E + Send + 'static) -> Self {
        Self {
            create_fn: Box::new(create_fn),
        }
    }
    pub fn create(self, config: WebsocketConfig) -> (Server<E>, JoinHandle<()>) {
        <Server<E>>::create(config, self.create_fn)
    }
}
#[derive(Debug)]
pub struct Server<E: ServerExt> {
    connections: mpsc::UnboundedSender<NewConnection<E>>,
    calls: mpsc::UnboundedSender<E::Call>,
}

impl<E: ServerExt> From<Server<E>> for mpsc::UnboundedSender<E::Call> {
    fn from(server: Server<E>) -> Self {
        server.calls
    }
}

impl<E: ServerExt + 'static> Server<E> {
    pub(crate) fn create(
        config: WebsocketConfig,
        create: impl FnOnce(Self) -> E,
    ) -> (Self, JoinHandle<()>) {
        let (connection_sender, connection_receiver) = mpsc::unbounded_channel();
        let (disconnection_sender, disconnection_receiver) = mpsc::unbounded_channel();
        let (call_sender, call_receiver) = mpsc::unbounded_channel();
        let handle = Self {
            connections: connection_sender,
            calls: call_sender,
        };
        let extension = create(handle.clone());
        let actor = ServerActor {
            connections: connection_receiver,
            disconnections: disconnection_receiver,
            disconnections_tx: disconnection_sender,
            calls: call_receiver,
            extension,
            config,
        };
        let future = tokio::spawn(actor.run());

        (handle, future)
    }
}

impl<E: ServerExt> Server<E> {
    pub async fn accept(
        &self,
        socket: Socket,
        request: Request,
        address: SocketAddr,
    ) -> Option<<E::Session as SessionExt>::ID> {
        // TODO: can we refuse the connection here?
        let (sender, receiver) = oneshot::channel();
        self.connections
            .send(NewConnection {
                socket,
                request,
                address,
                respond_to: sender,
            })
            .map_err(|_| "connections is down")
            .unwrap();
        receiver.await.ok()
    }

    pub fn call(&self, call: E::Call) {
        self.calls.send(call).map_err(|_| ()).unwrap();
    }

    /// Calls a method on the session, allowing the Session to respond with oneshot::Sender.
    /// This is just for easier construction of the call which happen to contain oneshot::Sender in it.
    pub async fn call_with<R: std::fmt::Debug>(
        &self,
        f: impl FnOnce(oneshot::Sender<R>) -> E::Call,
    ) -> R {
        let (sender, receiver) = oneshot::channel();
        let call = f(sender);

        self.calls.send(call).map_err(|_| ()).unwrap();
        receiver.await.unwrap()
    }
}

impl<E: ServerExt> Clone for Server<E> {
    fn clone(&self) -> Self {
        Self {
            connections: self.connections.clone(),
            calls: self.calls.clone(),
        }
    }
}

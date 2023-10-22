//! Creating a WebSocket server or a client in Rust can be troublesome. This crate facilitates this process by providing:
//! - High-level abstraction of WebSocket, handling Ping/Pong from both Client and Server.
//! - Traits to allow declarative and event-based programming.
//! - Automatic reconnection of WebSocket Client.
//!
//! Refer to [`client`] or [`server`] module for detailed implementation guides.

extern crate core;

mod socket;

pub use socket::CloseCode;
pub use socket::CloseFrame;
pub use socket::Message;
pub use socket::Socket;

#[cfg(feature = "tokio-tungstenite")]
pub mod tungstenite;

pub mod config;
pub mod server;
pub mod session;

pub use server::Server;
pub use server::ServerExt;

pub use session::Session;
pub use session::SessionExt;
pub type Error = eyre::Error;
pub type Request = http::Request<()>;

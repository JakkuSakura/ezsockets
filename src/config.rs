use std::time::Duration;

#[derive(Clone, Debug)]
pub struct WebsocketConfig {
    pub address: String,
    pub heartbeat: Duration,
    pub timeout: Duration,
    pub channel_size: usize,
}

impl Default for WebsocketConfig {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:80".to_string(),
            heartbeat: Duration::from_secs(5),
            timeout: Duration::from_secs(10),
            channel_size: 1000,
        }
    }
}

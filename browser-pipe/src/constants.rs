use std::time::Duration;

pub const LOG_PATH: &str = "/tmp/browser-pipe.log";
pub const WS_LISTEN_ADDR: &str = "127.0.0.1:10129";
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const CHROME_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

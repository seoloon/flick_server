//! The in-memory log buffer: wiring into the state, and `GET /admin/v1/logs`.

mod common;

use std::sync::Arc;

use common::*;
use flicksync::app::AppState;
use flicksync::logs::LogBuffer;
use flicksync::settings::{Env, Settings};

#[tokio::test]
async fn the_state_serves_the_buffer_main_feeds() {
    let s = TestServer::start(&[("FLICKSYNC_LOG_BUFFER", "100")]).await;
    assert_eq!(s.state.logs.capacity(), 100);

    let dir = flicksync::settings::scratch_dir("logs");
    let data = dir.to_str().unwrap().to_owned();
    let env: Env = Arc::new(move |k: &str| match k {
        "FLICKSYNC_AUTH_KEYS" => Some(format!("k1:{SERVER}:{SECRET}")),
        "FLICKSYNC_DATA_DIR" => Some(data.clone()),
        _ => None,
    });
    let logs = Arc::new(LogBuffer::new(7));
    let state =
        AppState::with_log_buffer(Settings::open(&dir, env).unwrap(), logs.clone()).unwrap();
    assert!(Arc::ptr_eq(&state.logs, &logs));
}

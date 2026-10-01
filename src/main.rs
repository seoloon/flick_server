use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::ExitCode;
use std::time::Duration;

use flicksync::app;
use flicksync::config::{Config, LogFormat};
use tracing_subscriber::EnvFilter;

fn init_tracing(cfg: &Config) {
    let filter = EnvFilter::try_new(&cfg.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match cfg.log_format {
        LogFormat::Json => builder.json().init(),
        LogFormat::Pretty => builder.init(),
    }
}

/// `flicksync healthcheck`: dependency-free probe for container HEALTHCHECK
/// (the runtime image has no curl/wget).
fn healthcheck(cfg: &Config) -> bool {
    let host = if cfg.host == "0.0.0.0" || cfg.host == "::" {
        "127.0.0.1"
    } else {
        cfg.host.as_str()
    };
    let Ok(mut stream) = TcpStream::connect((host, cfg.port)) else {
        return false;
    };
    let timeout = Some(Duration::from_secs(3));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let req = format!("GET /health HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    let n = stream.read(&mut buf).unwrap_or(0);
    buf[..n].starts_with(b"HTTP/1.1 200")
}

#[tokio::main]
async fn main() -> ExitCode {
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::from(2);
        }
    };

    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return if healthcheck(&cfg) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    init_tracing(&cfg);
    if cfg.auth.keys.is_empty() {
        eprintln!(
            "configuration error: FLICKSYNC_AUTH_KEYS (or FLICKSYNC_AUTH_KEYS_FILE) must define at least one signing key (kid:server_id:secret)"
        );
        return ExitCode::from(2);
    }
    if let Err(e) = app::serve(cfg).await {
        tracing::error!(error = %e, "fatal error");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

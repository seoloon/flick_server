use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use flicksync::app::{self, AppState};
use flicksync::config::{Config, LogFormat};
use flicksync::invite::{self, KeySource};
use flicksync::settings::{Env, Settings};
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

/// `flicksync invite [--rotate] [--qr]`: print the invitation link (secret: stdout only).
fn invite_command(mut cfg: Config, args: &[String]) -> ExitCode {
    let mut rotate = false;
    let mut qr = false;
    for a in args {
        match a.as_str() {
            "--rotate" => rotate = true,
            "--qr" => qr = true,
            other => {
                eprintln!(
                    "unknown option '{other}'
usage: flicksync invite [--rotate] [--qr]"
                );
                return ExitCode::from(2);
            }
        }
    }
    let result = (|| -> Result<(), invite::InviteError> {
        if rotate {
            if cfg.keys_configured {
                return Err(invite::InviteError::Unsupported(
                    "keys are set through FLICKSYNC_AUTH_KEYS / FLICKSYNC_AUTH_KEYS_FILE: rotate them there (--rotate only manages the auto-generated key file)"
                        .into(),
                ));
            }
            cfg.auth.keys = invite::rotate(std::path::Path::new(&cfg.data_dir))?;
        } else {
            invite::ensure_keys(&mut cfg)?;
        }
        let inv = invite::invitation(&cfg)?;
        let (_, guessed) = invite::endpoint(&cfg);
        println!("{}", inv.to_url());
        if qr {
            match invite::qr(&inv.to_url()) {
                Some(q) => println!("{q}"),
                None => eprintln!("could not render a QR code for this invitation"),
            }
        }
        eprintln!("SECRET: this link contains the signing key. Share it only with your invitees.");
        if guessed {
            eprintln!(
                "WARNING: FLICKSYNC_PUBLIC_URL is not set; the address is a guess (plain http on the bind address)."
            );
        }
        if rotate {
            eprintln!(
                "New key added; the previous key stays valid. Restart FlickSync so the running server loads it."
            );
        }
        Ok(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let env: Env = Arc::new(|k| std::env::var(k).ok());
    let settings = match Settings::open(&Settings::data_dir_from_env(&env), env) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::from(2);
        }
    };
    let mut cfg = match settings.config() {
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

    if std::env::args().nth(1).as_deref() == Some("invite") {
        let rest: Vec<String> = std::env::args().skip(2).collect();
        return invite_command(cfg, &rest);
    }

    init_tracing(&cfg);
    let key_source = match invite::ensure_keys(&mut cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("configuration error: cannot load or create the signing key: {e}");
            return ExitCode::from(2);
        }
    };
    if let KeySource::File(path) = &key_source {
        match invite::invitation(&cfg) {
            Ok(inv) => {
                let (_, guessed) = invite::endpoint(&cfg);
                // Deliberately not through `tracing`: one readable block, never JSON-escaped or filtered.
                eprint!("{}", invite::banner(&inv, guessed, path));
            }
            Err(e) => tracing::error!(error = %e, "cannot build the invitation"),
        }
    }
    if cfg.auth.keys.is_empty() {
        eprintln!(
            "configuration error: FLICKSYNC_AUTH_KEYS (or FLICKSYNC_AUTH_KEYS_FILE) must define at least one signing key (kid:server_id:secret)"
        );
        return ExitCode::from(2);
    }
    let state = match AppState::new(settings) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = app::serve(state).await {
        tracing::error!(error = %e, "fatal error");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

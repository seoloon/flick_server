//! Development helper: mint a token the way a Flick Server would.
//!
//!     cargo run --example mint_token -- <kid> <secret> <server_id> <user_id> [name] [ttl_secs]
//!
//! Real deployments must mint tokens inside the Flick Server (which owns the secret);
//! this exists to try FlickSync by hand and to document the claim layout.

use std::time::{SystemTime, UNIX_EPOCH};

use flicksync::auth::{Claims, mint_token};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: mint_token <kid> <secret> <server_id> <user_id> [name] [ttl_secs]");
        std::process::exit(2);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs();
    let ttl: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(3600);
    let claims = Claims {
        sub: args[3].clone(),
        server_id: args[2].clone(),
        aud: "flicksync".into(),
        exp: now + ttl,
        iat: Some(now),
        name: args.get(4).cloned(),
        perms: ["rooms:create", "rooms:join", "chat:send"]
            .map(String::from)
            .to_vec(),
    };
    match mint_token(&args[0], &args[1], &claims) {
        Ok(t) => println!("{t}"),
        Err(e) => {
            eprintln!("could not mint token: {e}");
            std::process::exit(1);
        }
    }
}

//! Authentication: locally verifiable tokens issued by a Flick Server.
//!
//! FlickSync has no accounts. A Flick Server signs a short-lived HS256 JWT for
//! its user; FlickSync verifies it offline (no call to the Flick Server).
//!
//! * Each signing key is configured as `kid:server_id:secret`. A key is bound
//!   to exactly one Flick Server, and the token's `server_id` claim must equal
//!   it: a token signed with server A's key can never claim to be server B.
//! * Rotation = configure several `kid`s for the same `server_id`, switch the
//!   issuer to the new `kid`, then remove the old one.
//! * Only HS256 is accepted (pinned algorithm: no `none`, no alg confusion).

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};

use crate::errors::{Error, ErrorCode, Result};

pub const PERM_CREATE_ROOM: &str = "rooms:create";
pub const PERM_JOIN_ROOM: &str = "rooms:join";
pub const PERM_CHAT: &str = "chat:send";

const MIN_SECRET_LEN: usize = 32;
const MAX_ID_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claims {
    /// Flick user id.
    pub sub: String,
    /// Flick Server instance that issued the token.
    pub server_id: String,
    /// Audience; must equal the configured audience (default `flicksync`).
    pub aud: String,
    pub exp: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iat: Option<u64>,
    /// Display name shown to other participants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Granted permissions (`rooms:create`, `rooms:join`, `chat:send`, or `*`).
    #[serde(default)]
    pub perms: Vec<String>,
}

/// A verified identity.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub user_id: String,
    pub server_id: String,
    pub display_name: String,
    permissions: HashSet<String>,
}

impl Identity {
    pub fn new(
        user_id: impl Into<String>,
        server_id: impl Into<String>,
        display_name: impl Into<String>,
        permissions: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            server_id: server_id.into(),
            display_name: display_name.into(),
            permissions: permissions.into_iter().collect(),
        }
    }

    pub fn permits(&self, perm: &str) -> bool {
        self.permissions.contains("*") || self.permissions.contains(perm)
    }

    pub fn require(&self, perm: &str) -> Result<()> {
        if self.permits(perm) {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::Forbidden,
                format!("your token does not grant '{perm}'"),
            ))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthConfigError {
    #[error("invalid FLICKSYNC_AUTH_KEYS entry (expected kid:server_id:secret): {0}")]
    BadEntry(String),
    #[error("secret for key '{0}' is shorter than {MIN_SECRET_LEN} characters")]
    WeakSecret(String),
    #[error("duplicate key id '{0}'")]
    DuplicateKid(String),
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// `kid:server_id:secret` entries.
    pub keys: Vec<String>,
    pub audience: String,
    /// Tokens whose `exp` is further than this in the future are refused.
    pub max_token_ttl_secs: u64,
    pub leeway_secs: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            audience: "flicksync".into(),
            max_token_ttl_secs: 24 * 3600,
            leeway_secs: 30,
        }
    }
}

struct Key {
    server_id: String,
    decoding: DecodingKey,
}

pub struct Authenticator {
    keys: HashMap<String, Key>,
    validation: Validation,
    max_ttl: u64,
    leeway: u64,
}

fn valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'@'))
}

fn unauthenticated(msg: &str) -> Error {
    Error::new(ErrorCode::Unauthenticated, msg)
}

/// Split and validate a `kid:server_id:secret` entry (the secret may itself contain `:`).
pub fn parse_key_entry(entry: &str) -> std::result::Result<(&str, &str, &str), AuthConfigError> {
    let mut parts = entry.splitn(3, ':');
    let (Some(kid), Some(server_id), Some(secret)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(AuthConfigError::BadEntry("<redacted>".into()));
    };
    if !valid_ident(kid) || !valid_ident(server_id) {
        return Err(AuthConfigError::BadEntry(format!(
            "{kid}:{server_id}:<redacted>"
        )));
    }
    if secret.len() < MIN_SECRET_LEN {
        return Err(AuthConfigError::WeakSecret(kid.to_owned()));
    }
    Ok((kid, server_id, secret))
}

impl Authenticator {
    pub fn new(cfg: &AuthConfig) -> std::result::Result<Self, AuthConfigError> {
        let mut keys = HashMap::new();
        for entry in &cfg.keys {
            let (kid, server_id, secret) = parse_key_entry(entry)?;
            let key = Key {
                server_id: server_id.to_owned(),
                decoding: DecodingKey::from_secret(secret.as_bytes()),
            };
            if keys.insert(kid.to_owned(), key).is_some() {
                return Err(AuthConfigError::DuplicateKid(kid.to_owned()));
            }
        }
        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = cfg.leeway_secs;
        validation.validate_exp = true;
        validation.set_audience(&[cfg.audience.as_str()]);
        validation.set_required_spec_claims(&["exp", "sub", "aud"]);
        Ok(Self {
            keys,
            validation,
            max_ttl: cfg.max_token_ttl_secs,
            leeway: cfg.leeway_secs,
        })
    }

    pub fn has_keys(&self) -> bool {
        !self.keys.is_empty()
    }

    /// Verify a token. The error message is deliberately generic; the token is never logged.
    pub fn verify(&self, token: &str) -> Result<Identity> {
        let header = decode_header(token).map_err(|_| unauthenticated("malformed token"))?;
        if header.alg != Algorithm::HS256 {
            return Err(unauthenticated("unsupported token algorithm"));
        }
        let kid = header
            .kid
            .ok_or_else(|| unauthenticated("token has no key id"))?;
        let key = self
            .keys
            .get(&kid)
            .ok_or_else(|| unauthenticated("unknown signing key"))?;
        let data = decode::<Claims>(token, &key.decoding, &self.validation)
            .map_err(|_| unauthenticated("invalid or expired token"))?;
        let c = data.claims;

        // Bind the token to the Flick Server that owns the signing key.
        if c.server_id != key.server_id {
            return Err(unauthenticated(
                "token server_id does not match its signing key",
            ));
        }
        if !valid_ident(&c.sub) {
            return Err(unauthenticated("invalid subject"));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if c.exp > now + self.max_ttl + self.leeway {
            return Err(unauthenticated(
                "token lifetime exceeds the allowed maximum",
            ));
        }
        let display_name = c
            .name
            .as_deref()
            .map(|n| crate::chat::sanitize_text(n, 64))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| c.sub.clone());
        Ok(Identity::new(c.sub, c.server_id, display_name, c.perms))
    }
}

/// Mint a token (used by tests and the `mint_token` example; real tokens come from the Flick Server).
pub fn mint_token(
    kid: &str,
    secret: &str,
    claims: &Claims,
) -> std::result::Result<String, jsonwebtoken::errors::Error> {
    let mut header = jsonwebtoken::Header::new(Algorithm::HS256);
    header.kid = Some(kid.to_owned());
    jsonwebtoken::encode(
        &header,
        claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SECRET_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn auth() -> Authenticator {
        Authenticator::new(&AuthConfig {
            keys: vec![
                format!("a1:server-a:{SECRET_A}"),
                format!("b1:server-b:{SECRET_B}"),
            ],
            ..AuthConfig::default()
        })
        .unwrap()
    }

    fn claims(server: &str) -> Claims {
        Claims {
            sub: "alice".into(),
            server_id: server.into(),
            aud: "flicksync".into(),
            exp: now() + 600,
            iat: Some(now()),
            name: Some("Alice".into()),
            perms: vec![PERM_CREATE_ROOM.into(), PERM_JOIN_ROOM.into()],
        }
    }

    #[test]
    fn valid_token_yields_identity() {
        let t = mint_token("a1", SECRET_A, &claims("server-a")).unwrap();
        let id = auth().verify(&t).unwrap();
        assert_eq!(id.user_id, "alice");
        assert_eq!(id.server_id, "server-a");
        assert_eq!(id.display_name, "Alice");
        assert!(id.permits(PERM_CREATE_ROOM));
        assert!(!id.permits(PERM_CHAT));
        assert!(id.require(PERM_CHAT).is_err());
    }

    #[test]
    fn wildcard_permission() {
        let mut c = claims("server-a");
        c.perms = vec!["*".into()];
        let t = mint_token("a1", SECRET_A, &c).unwrap();
        assert!(auth().verify(&t).unwrap().permits(PERM_CHAT));
    }

    #[test]
    fn expired_token_is_rejected() {
        let mut c = claims("server-a");
        c.exp = now() - 3600;
        let t = mint_token("a1", SECRET_A, &c).unwrap();
        assert_eq!(
            auth().verify(&t).unwrap_err().code,
            ErrorCode::Unauthenticated
        );
    }

    #[test]
    fn token_cannot_impersonate_another_flick_server() {
        // Signed with server A's key but claims to be server B.
        let t = mint_token("a1", SECRET_A, &claims("server-b")).unwrap();
        assert!(auth().verify(&t).is_err());
        // Signed with server A's secret under server B's kid: signature fails.
        let t = mint_token("b1", SECRET_A, &claims("server-b")).unwrap();
        assert!(auth().verify(&t).is_err());
    }

    #[test]
    fn wrong_audience_unknown_kid_and_bad_signature_rejected() {
        let mut c = claims("server-a");
        c.aud = "other".into();
        assert!(
            auth()
                .verify(&mint_token("a1", SECRET_A, &c).unwrap())
                .is_err()
        );
        assert!(
            auth()
                .verify(&mint_token("zzz", SECRET_A, &claims("server-a")).unwrap())
                .is_err()
        );
        assert!(
            auth()
                .verify(&mint_token("a1", SECRET_B, &claims("server-a")).unwrap())
                .is_err()
        );
    }

    #[test]
    fn garbage_and_alg_none_rejected() {
        let a = auth();
        for t in ["", "abc", "a.b.c", "eyJhbGciOiJub25lIn0.e30."] {
            assert!(a.verify(t).is_err(), "{t}");
        }
    }

    #[test]
    fn overlong_lifetime_rejected() {
        let mut c = claims("server-a");
        c.exp = now() + 30 * 24 * 3600;
        assert!(
            auth()
                .verify(&mint_token("a1", SECRET_A, &c).unwrap())
                .is_err()
        );
    }

    #[test]
    fn key_rotation_accepts_both_kids_for_same_server() {
        let a = Authenticator::new(&AuthConfig {
            keys: vec![
                format!("old:server-a:{SECRET_A}"),
                format!("new:server-a:{SECRET_B}"),
            ],
            ..AuthConfig::default()
        })
        .unwrap();
        assert!(
            a.verify(&mint_token("old", SECRET_A, &claims("server-a")).unwrap())
                .is_ok()
        );
        assert!(
            a.verify(&mint_token("new", SECRET_B, &claims("server-a")).unwrap())
                .is_ok()
        );
    }

    #[test]
    fn config_validation() {
        let bad = |k: &str| {
            Authenticator::new(&AuthConfig {
                keys: vec![k.into()],
                ..AuthConfig::default()
            })
            .is_err()
        };
        assert!(bad("only-two:parts"));
        assert!(bad("kid:server:short"));
        assert!(bad(&format!("bad kid:server:{SECRET_A}")));
        let dup = AuthConfig {
            keys: vec![format!("k:s:{SECRET_A}"), format!("k:s:{SECRET_B}")],
            ..AuthConfig::default()
        };
        assert!(Authenticator::new(&dup).is_err());
    }

    #[test]
    fn display_name_falls_back_to_user_id() {
        let mut c = claims("server-a");
        c.name = None;
        let id = auth()
            .verify(&mint_token("a1", SECRET_A, &c).unwrap())
            .unwrap();
        assert_eq!(id.display_name, "alice");
    }
}

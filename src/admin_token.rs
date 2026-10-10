//! The admin API token: derived from `PANEL_PASSWORD`, so one secret serves the panel and the API.
//!
//! `token = hex(HMAC-SHA256(key = PANEL_PASSWORD, msg = "flick-admin-api-v1"))`. HMAC (RFC 2104)
//! is built here on the `sha2` dependency the crate already has.

use sha2::{Digest, Sha256};

use crate::api::auth::constant_time_eq;
use crate::config::Config;

const BLOCK: usize = 64;
const DOMAIN: &[u8] = b"flick-admin-api-v1";
/// Same minimum as the panel's own password rule.
pub const MIN_PASSWORD_LEN: usize = 10;

pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(Sha256::digest(key).as_slice());
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner);
    let mut out = [0u8; 32];
    out.copy_from_slice(outer.finalize().as_slice());
    out
}

/// The admin token for `password`, or `None` when it is too short to protect anything.
pub fn derive(password: &str) -> Option<String> {
    if password.len() < MIN_PASSWORD_LEN {
        return None;
    }
    Some(
        hmac_sha256(password.as_bytes(), DOMAIN)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// What the admin API accepts: the token derived from `PANEL_PASSWORD`.
pub struct AdminTokens {
    pub(crate) derived: Option<String>,
}

impl AdminTokens {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            derived: cfg.http.panel_password.as_deref().and_then(derive),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.derived.is_some()
    }

    /// Constant-time comparison against the derived token; `false` when there is none.
    pub fn accepts(&self, presented: &str) -> bool {
        self.derived
            .as_deref()
            .is_some_and(|t| constant_time_eq(presented.as_bytes(), t.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        // Test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a key longer than the block size.
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn the_derived_token_matches_the_value_the_panel_computes() {
        // Same vector computed with Node: createHmac("sha256", password).update("flick-admin-api-v1")
        assert_eq!(
            derive("correct horse battery staple").as_deref(),
            Some("602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011")
        );
    }

    #[test]
    fn a_short_password_gives_no_token() {
        assert_eq!(derive("123456789"), None);
        assert!(derive("1234567890").is_some());
    }

    #[test]
    fn tokens_accept_only_the_derived_one() {
        let t = AdminTokens {
            derived: derive("correct horse battery staple"),
        };
        assert!(t.is_enabled());
        assert!(t.accepts("602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011"));
        assert!(!t.accepts("legacy-token-0123456789"));
        assert!(!AdminTokens { derived: None }.is_enabled());
    }
}

//! Signing-key bootstrap and invitation links.
//!
//! * When no key is configured through the environment, a key is generated on first start and
//!   persisted in `FLICKSYNC_DATA_DIR` (file `auth_keys`, mode 0600, one `kid:server_id:secret`
//!   per line, newest last). It is never silently regenerated.
//! * An *invitation* packs the public address and that key into one copyable link:
//!
//!   `flickserver://<host>[:<port>]/?v=1&tls=<0|1>#k=<base64url(kid:server_id:secret)>`
//!
//!   The key sits in the fragment so it never reaches a proxy log. Specified in
//!   `docs/flick-integration.md`.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt;

use crate::auth::{AuthConfigError, parse_key_entry};
use crate::config::Config;

pub const SCHEME: &str = "flickserver://";
pub const INVITE_VERSION: u32 = 1;
pub const KEY_FILE: &str = "auth_keys";
const DEFAULT_KID: &str = "main";
const DEFAULT_SERVER_ID: &str = "default";
const SECRET_LEN: usize = 48;
/// base64url alphabet.
const SECRET_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Debug, thiserror::Error)]
pub enum InviteError {
    #[error("{0}")]
    Invalid(String),
    #[error("{path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error(transparent)]
    Key(#[from] AuthConfigError),
    #[error("{0}")]
    Unsupported(String),
}

fn invalid(msg: impl Into<String>) -> InviteError {
    InviteError::Invalid(msg.into())
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> InviteError + '_ {
    move |source| InviteError::Io {
        path: path.display().to_string(),
        source,
    }
}

// ------------------------------------------------------------------ endpoint

/// Public address of the service as clients must reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// `host` or `host:port` (IPv6 hosts in brackets).
    pub authority: String,
    /// Path prefix when the service sits behind a reverse proxy under one (`/services`): empty, or
    /// `/seg` or `/seg/seg`, never a trailing slash.
    pub path: String,
    pub tls: bool,
}

/// Validate and normalise a path prefix: `""`, `"/"` and `"/services/"` become `""`, `""`, `"/services"`.
fn normalize_path(raw: &str) -> Result<String, InviteError> {
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let ok = trimmed.len() <= 128
        && trimmed.starts_with('/')
        && trimmed[1..].split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
        });
    if ok {
        Ok(trimmed.to_owned())
    } else {
        Err(invalid(
            "invalid path prefix (use letters, digits and - . _ ~ in segments, e.g. /services)",
        ))
    }
}

fn valid_authority(a: &str) -> bool {
    let port = if let Some(rest) = a.strip_prefix('[') {
        let Some((h, tail)) = rest.split_once(']') else {
            return false;
        };
        if h.is_empty()
            || !h
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
        {
            return false;
        }
        match tail {
            "" => None,
            t => match t.strip_prefix(':') {
                Some(p) => Some(p),
                None => return false,
            },
        }
    } else {
        let (h, p) = match a.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (a, None),
        };
        let host_ok = !h.is_empty()
            && h.len() <= 253
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        if !host_ok {
            return false;
        }
        p
    };
    port.is_none_or(|p| {
        p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n > 0)
    })
}

impl Endpoint {
    /// Parse `FLICKSYNC_PUBLIC_URL` (`http(s)://host[:port][/prefix]`).
    pub fn from_public_url(raw: &str) -> Result<Self, InviteError> {
        let raw = raw.trim();
        let (tls, rest) = if let Some(r) = raw.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = raw.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(invalid("expected an http:// or https:// URL"));
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains(['?', '#', '@']) || path.contains(['?', '#']) {
            return Err(invalid(
                "expected scheme://host[:port][/prefix] only (no query, fragment or credentials)",
            ));
        }
        let authority = authority.to_ascii_lowercase();
        if !valid_authority(&authority) {
            return Err(invalid("invalid host or port"));
        }
        Ok(Self {
            authority,
            path: normalize_path(path)?,
            tls,
        })
    }

    /// Best guess when `FLICKSYNC_PUBLIC_URL` is unset: the bind address (plain http).
    pub fn from_bind(host: &str, port: u16) -> Self {
        let host = match host {
            "0.0.0.0" | "::" | "" => "localhost".to_owned(),
            h if h.contains(':') && !h.starts_with('[') => format!("[{h}]"),
            h => h.to_owned(),
        };
        Self {
            authority: format!("{host}:{port}"),
            path: String::new(),
            tls: false,
        }
    }

    /// Base URL of the HTTP API (`http(s)://host[:port][/prefix]`). Every API path, `ws_path`
    /// included, is relative to it.
    pub fn http_base(&self) -> String {
        format!(
            "{}://{}{}",
            if self.tls { "https" } else { "http" },
            self.authority,
            self.path
        )
    }
}

// ------------------------------------------------------------------ invitation

/// Everything a client needs: where the service is and the signing key.
#[derive(Clone, PartialEq, Eq)]
pub struct Invitation {
    pub endpoint: Endpoint,
    /// `kid:server_id:secret`
    pub key: String,
}

// The secret must never reach a log through `{:?}`.
impl fmt::Debug for Invitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Invitation")
            .field("endpoint", &self.endpoint)
            .field("key", &"<redacted>")
            .finish()
    }
}

impl Invitation {
    pub fn new(endpoint: Endpoint, key: impl Into<String>) -> Result<Self, InviteError> {
        let key = key.into();
        parse_key_entry(&key)?;
        Ok(Self { endpoint, key })
    }

    /// The copyable link.
    pub fn to_url(&self) -> String {
        format!(
            "{SCHEME}{}{}/?v={INVITE_VERSION}&tls={}#k={}",
            self.endpoint.authority,
            self.endpoint.path,
            u8::from(self.endpoint.tls),
            URL_SAFE_NO_PAD.encode(self.key.as_bytes()),
        )
    }
}

impl FromStr for Invitation {
    type Err = InviteError;

    fn from_str(s: &str) -> Result<Self, InviteError> {
        let s = s.trim();
        let rest = s
            .strip_prefix(SCHEME)
            .ok_or_else(|| invalid("not a flickserver:// invitation"))?;
        let (before, fragment) = rest
            .split_once('#')
            .ok_or_else(|| invalid("invitation has no key (missing #k=...)"))?;
        let (location, query) = before.split_once('?').unwrap_or((before, ""));
        let (authority, raw_path) = match location.find('/') {
            Some(i) => (&location[..i], &location[i..]),
            None => (location, ""),
        };
        let path = normalize_path(raw_path)?;
        if !valid_authority(authority) {
            return Err(invalid("invalid host or port in invitation"));
        }

        let param = |text: &str, name: &str| {
            text.split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_owned())
        };
        let version = param(query, "v").ok_or_else(|| invalid("invitation has no version (v)"))?;
        if version.parse::<u32>().ok() != Some(INVITE_VERSION) {
            return Err(InviteError::Unsupported(format!(
                "unsupported invitation version '{version}' (this build understands v={INVITE_VERSION})"
            )));
        }
        let tls = match param(query, "tls").as_deref() {
            Some("1") => true,
            Some("0") => false,
            _ => return Err(invalid("tls must be 0 or 1")),
        };
        let encoded = param(fragment, "k").ok_or_else(|| invalid("invitation has no key (k)"))?;
        let key = URL_SAFE_NO_PAD
            .decode(encoded.as_bytes())
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or_else(|| invalid("invitation key is not valid base64url"))?;
        Invitation::new(
            Endpoint {
                authority: authority.to_ascii_lowercase(),
                path,
                tls,
            },
            key,
        )
    }
}

// ------------------------------------------------------------------ key generation / storage

pub fn generate_secret() -> String {
    let mut rng = rand::rng();
    (0..SECRET_LEN)
        .map(|_| SECRET_ALPHABET[rng.random_range(0..SECRET_ALPHABET.len())] as char)
        .collect()
}

fn read_keys(path: &Path) -> Result<Vec<String>, InviteError> {
    let content = std::fs::read_to_string(path).map_err(io_err(path))?;
    let keys: Vec<String> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect();
    if keys.is_empty() {
        // Never fall back to generating a new key behind the operator's back.
        return Err(invalid(format!(
            "{} exists but holds no key; restore it or delete it explicitly to start over",
            path.display()
        )));
    }
    for k in &keys {
        parse_key_entry(k)?;
    }
    Ok(keys)
}

/// Write `content` to a new file readable by the owner only.
fn write_private(path: &Path, content: &str) -> Result<(), InviteError> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(io_err(path))?;
    f.write_all(content.as_bytes()).map_err(io_err(path))?;
    f.sync_all().map_err(io_err(path))
}

fn new_key_line(kid: &str, server_id: &str) -> String {
    format!("{kid}:{server_id}:{}", generate_secret())
}

/// Keys stored in `dir`, generating the first one when the file does not exist yet.
pub fn load_or_create(dir: &Path) -> Result<Vec<String>, InviteError> {
    let path = dir.join(KEY_FILE);
    if path.exists() {
        return read_keys(&path);
    }
    std::fs::create_dir_all(dir).map_err(io_err(dir))?;
    let line = new_key_line(DEFAULT_KID, DEFAULT_SERVER_ID);
    match write_private(&path, &format!("{line}\n")) {
        Ok(()) => Ok(vec![line]),
        // Lost a race with another process: use what it wrote.
        Err(InviteError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            read_keys(&path)
        }
        Err(e) => Err(e),
    }
}

/// Append a new key (new kid, same server id as the newest key) and keep the old ones valid.
/// Returns the full key list; the new key is last.
pub fn rotate(dir: &Path) -> Result<Vec<String>, InviteError> {
    let mut keys = load_or_create(dir)?;
    let (_, server_id, _) = parse_key_entry(keys.last().expect("non-empty"))?;
    let server_id = server_id.to_owned();
    let kid = loop {
        let mut rng = rand::rng();
        let suffix: String = (0..6)
            .map(|_| b"abcdefghijklmnopqrstuvwxyz0123456789"[rng.random_range(0..36usize)] as char)
            .collect();
        let candidate = format!("k-{suffix}");
        if !keys
            .iter()
            .any(|k| k.split(':').next() == Some(candidate.as_str()))
        {
            break candidate;
        }
    };
    keys.push(new_key_line(&kid, &server_id));

    // Write a sibling file then rename: a crash never leaves a truncated key file.
    let path = dir.join(KEY_FILE);
    let tmp = dir.join(format!("{KEY_FILE}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    write_private(&tmp, &(keys.join("\n") + "\n"))?;
    std::fs::rename(&tmp, &path).map_err(io_err(&path))?;
    Ok(keys)
}

// ------------------------------------------------------------------ wiring

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// Keys come from `FLICKSYNC_AUTH_KEYS[_FILE]`; nothing is generated or printed.
    Env,
    /// Auto-managed key file.
    File(PathBuf),
}

/// Fill `cfg.auth.keys` from the data directory when no key was configured explicitly.
pub fn ensure_keys(cfg: &mut Config) -> Result<KeySource, InviteError> {
    if cfg.keys_configured {
        return Ok(KeySource::Env);
    }
    let dir = PathBuf::from(&cfg.data_dir);
    cfg.auth.keys = load_or_create(&dir)?;
    Ok(KeySource::File(dir.join(KEY_FILE)))
}

/// Public endpoint for invitations, plus whether it had to be guessed.
pub fn endpoint(cfg: &Config) -> (Endpoint, bool) {
    match &cfg.public {
        Some(e) => (e.clone(), false),
        None => (Endpoint::from_bind(&cfg.host, cfg.port), true),
    }
}

/// Invitation for the newest configured key.
pub fn invitation(cfg: &Config) -> Result<Invitation, InviteError> {
    let key = cfg
        .auth
        .keys
        .last()
        .ok_or_else(|| invalid("no signing key is loaded"))?;
    Invitation::new(endpoint(cfg).0, key.clone())
}

/// Human-readable startup banner. The only place besides `flicksync invite` that prints the secret.
pub fn banner(inv: &Invitation, guessed_endpoint: bool, key_file: &Path) -> String {
    let mut b = String::new();
    b.push_str("\n==================== FlickSync invitation ====================\n");
    b.push_str("Paste this single link into Flick to connect:\n\n  ");
    b.push_str(&inv.to_url());
    b.push_str("\n\nSECRET: it contains the signing key. Share it only with your invitees.\n");
    b.push_str(&format!("Key file: {}\n", key_file.display()));
    b.push_str(
        "Show it again with: flicksync invite   (docker compose exec flick-modules flicksync invite)\n",
    );
    if guessed_endpoint {
        b.push_str(
            "WARNING: FLICKSYNC_PUBLIC_URL is not set, so the address above is a guess (plain http on the bind\n\
             address). Set FLICKSYNC_PUBLIC_URL=https://your.domain behind a reverse proxy.\n",
        );
    }
    b.push_str("==============================================================\n");
    b
}

/// ASCII/Unicode QR code of `text`.
pub fn qr(text: &str) -> Option<String> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    Some(code.render::<Dense1x2>().quiet_zone(true).build())
}

/// QR code as rows of `'1'` (dark) / `'0'` (light) modules, for clients that draw it themselves.
pub fn qr_modules(text: &str) -> Option<Vec<String>> {
    use qrcode::Color;
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let w = code.width();
    Some(
        code.to_colors()
            .chunks(w)
            .map(|row| {
                row.iter()
                    .map(|c| if *c == Color::Dark { '1' } else { '0' })
                    .collect()
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("flicksync-test-{}", generate_secret()));
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn cfg(vars: &[(&str, &str)]) -> Config {
        let m: std::collections::HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(&move |k| m.get(k).cloned()).unwrap()
    }

    #[test]
    fn generated_secret_is_long_url_safe_and_random() {
        let a = generate_secret();
        assert!(a.len() >= 48);
        assert!(a.bytes().all(|b| SECRET_ALPHABET.contains(&b)));
        assert_ne!(a, generate_secret());
    }

    #[test]
    fn first_start_generates_then_restarts_reuse_the_same_key() {
        let d = TempDir::new();
        let first = load_or_create(d.path()).unwrap();
        assert_eq!(first.len(), 1);
        let (kid, server_id, secret) = parse_key_entry(&first[0]).unwrap();
        assert_eq!((kid, server_id), ("main", "default"));
        assert!(secret.len() >= 48);
        assert_eq!(load_or_create(d.path()).unwrap(), first);
        assert_eq!(load_or_create(d.path()).unwrap(), first);
    }

    #[cfg(unix)]
    #[test]
    fn key_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = TempDir::new();
        load_or_create(d.path()).unwrap();
        let mode = std::fs::metadata(d.path().join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        rotate(d.path()).unwrap();
        let mode = std::fs::metadata(d.path().join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_empty_or_corrupt_key_file_is_an_error_not_a_regeneration() {
        let d = TempDir::new();
        std::fs::create_dir_all(d.path()).unwrap();
        let f = d.path().join(KEY_FILE);
        std::fs::write(&f, "\n# nothing\n").unwrap();
        assert!(load_or_create(d.path()).is_err());
        std::fs::write(&f, "garbage\n").unwrap();
        assert!(load_or_create(d.path()).is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "garbage\n");
    }

    #[test]
    fn explicit_keys_take_priority_and_nothing_is_written() {
        let d = TempDir::new();
        let mut c = cfg(&[
            (
                "FLICKSYNC_AUTH_KEYS",
                "k1:srv:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            ("FLICKSYNC_DATA_DIR", d.path().to_str().unwrap()),
        ]);
        assert_eq!(ensure_keys(&mut c).unwrap(), KeySource::Env);
        assert_eq!(c.auth.keys.len(), 1);
        assert!(!d.path().exists());
    }

    #[test]
    fn keys_file_variable_also_disables_generation() {
        let d = TempDir::new();
        std::fs::create_dir_all(d.path()).unwrap();
        let f = d.path().join("ext_keys");
        std::fs::write(
            &f,
            "k9:srv:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
",
        )
        .unwrap();
        let data = d.path().join("data");
        let mut c = cfg(&[
            ("FLICKSYNC_AUTH_KEYS_FILE", f.to_str().unwrap()),
            ("FLICKSYNC_DATA_DIR", data.to_str().unwrap()),
        ]);
        assert_eq!(ensure_keys(&mut c).unwrap(), KeySource::Env);
        assert_eq!(c.auth.keys.len(), 1);
        assert!(!data.exists());
    }

    #[test]
    fn no_variables_generates_into_data_dir_and_persists_across_restarts() {
        let d = TempDir::new();
        let vars = [("FLICKSYNC_DATA_DIR", d.path().to_str().unwrap())];
        let mut c1 = cfg(&vars);
        assert!(matches!(ensure_keys(&mut c1).unwrap(), KeySource::File(_)));
        let mut c2 = cfg(&vars);
        ensure_keys(&mut c2).unwrap();
        assert_eq!(c1.auth.keys, c2.auth.keys);
        // The generated key is accepted by the authenticator.
        assert!(
            crate::auth::Authenticator::new(&c1.auth)
                .unwrap()
                .has_keys()
        );
    }

    #[test]
    fn rotation_adds_a_key_with_new_kid_same_server_and_keeps_the_old_one() {
        let d = TempDir::new();
        let old = load_or_create(d.path()).unwrap();
        let after = rotate(d.path()).unwrap();
        assert_eq!(after.len(), 2);
        assert_eq!(after[0], old[0]);
        let (k0, s0, sec0) = parse_key_entry(&after[0]).unwrap();
        let (k1, s1, sec1) = parse_key_entry(&after[1]).unwrap();
        assert_ne!(k0, k1);
        assert_eq!(s0, s1);
        assert_ne!(sec0, sec1);
        // Persisted, and both keys load into an authenticator.
        assert_eq!(load_or_create(d.path()).unwrap(), after);
        let auth = crate::auth::AuthConfig {
            keys: after.clone(),
            ..Default::default()
        };
        crate::auth::Authenticator::new(&auth).unwrap();
        // A second rotation keeps both earlier keys.
        assert_eq!(rotate(d.path()).unwrap().len(), 3);
    }

    fn sample_key() -> String {
        format!("main:default:{}", generate_secret())
    }

    #[test]
    fn invitation_round_trips() {
        for (url, tls, auth) in [
            ("https://sync.example.com", true, "sync.example.com"),
            ("http://192.168.1.10:8787/", false, "192.168.1.10:8787"),
            ("https://[::1]:8443", true, "[::1]:8443"),
        ] {
            let ep = Endpoint::from_public_url(url).unwrap();
            assert_eq!((ep.tls, ep.authority.as_str()), (tls, auth));
            let key = sample_key();
            let inv = Invitation::new(ep, key.clone()).unwrap();
            let link = inv.to_url();
            assert!(link.starts_with("flickserver://"));
            assert!(link.contains("/?v=1&tls="));
            assert!(!link.contains(&key), "key is encoded, not literal");
            assert!(!link.contains(char::is_whitespace));
            assert!(
                !link.split('#').next().unwrap().contains("k="),
                "key is in the fragment only"
            );
            let parsed: Invitation = link.parse().unwrap();
            assert_eq!(parsed, inv);
            assert_eq!(parsed.key, key);
        }
    }

    #[test]
    fn invitation_example_is_stable() {
        let inv = Invitation::new(
            Endpoint::from_public_url("https://sync.example.com").unwrap(),
            "main:default:0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();
        assert_eq!(
            inv.to_url(),
            "flickserver://sync.example.com/?v=1&tls=1#k=bWFpbjpkZWZhdWx0OjAxMjM0NTY3ODlhYmNkZWYwMTIzNDU2Nzg5YWJjZGVmMDEyMzQ1Njc4OWFiY2RlZg"
        );
    }

    #[test]
    fn malformed_invitations_are_rejected() {
        let good = Invitation::new(
            Endpoint::from_public_url("https://a.example").unwrap(),
            sample_key(),
        )
        .unwrap()
        .to_url();
        assert!(good.parse::<Invitation>().is_ok());
        let enc = good.split("k=").nth(1).unwrap();
        for bad in [
            "".to_owned(),
            "https://a.example".to_owned(),
            "flickserver://a.example/?v=1&tls=1".to_owned(),
            // the former scheme is no longer accepted, whatever the rest of the link
            format!("flicksync://a.example/?v=1&tls=1#k={enc}"),
            format!("flickserver://a.example/?v=2&tls=1#k={enc}"),
            format!("flickserver://a.example/?tls=1#k={enc}"),
            format!("flickserver://a.example/?v=1&tls=2#k={enc}"),
            format!("flickserver://a.example/x//y/?v=1&tls=1#k={enc}"),
            format!("flickserver://a.example/../?v=1&tls=1#k={enc}"),
            format!("flickserver://a.example/a b/?v=1&tls=1#k={enc}"),
            format!("flickserver://a b/?v=1&tls=1#k={enc}"),
            format!("flickserver://a.example:99999/?v=1&tls=1#k={enc}"),
            "flickserver://a.example/?v=1&tls=1#k=!!!".to_owned(),
            // key that is valid base64url but not a kid:server_id:secret entry
            format!(
                "flickserver://a.example/?v=1&tls=1#k={}",
                URL_SAFE_NO_PAD.encode("short")
            ),
        ] {
            assert!(bad.parse::<Invitation>().is_err(), "{bad}");
        }
        let err = format!("flickserver://a.example/?v=2&tls=1#k={enc}")
            .parse::<Invitation>()
            .unwrap_err();
        assert!(matches!(err, InviteError::Unsupported(_)));
    }

    #[test]
    fn a_path_prefix_round_trips_and_is_normalised() {
        for (url, path, base) in [
            (
                "https://flick.example.com/services",
                "/services",
                "https://flick.example.com/services",
            ),
            (
                "https://flick.example.com/services/",
                "/services",
                "https://flick.example.com/services",
            ),
            (
                "http://h:8080/a/b-c_d.e~f",
                "/a/b-c_d.e~f",
                "http://h:8080/a/b-c_d.e~f",
            ),
            (
                "https://flick.example.com/",
                "",
                "https://flick.example.com",
            ),
        ] {
            let ep = Endpoint::from_public_url(url).unwrap();
            assert_eq!(ep.path, path, "{url}");
            assert_eq!(ep.http_base(), base);
            let inv = Invitation::new(ep, sample_key()).unwrap();
            let link = inv.to_url();
            assert!(link.starts_with(&format!(
                "flickserver://{}{}/?v=1&tls=",
                inv.endpoint.authority, path
            )));
            let parsed: Invitation = link.parse().unwrap();
            assert_eq!(parsed, inv);
            assert_eq!(parsed.endpoint.http_base(), base);
        }
    }

    #[test]
    fn unknown_parameters_are_ignored_for_forward_compatibility() {
        let key = sample_key();
        let link = format!(
            "flickserver://a.example/?v=1&tls=0&future=x#k={}&other=1",
            URL_SAFE_NO_PAD.encode(&key)
        );
        assert_eq!(link.parse::<Invitation>().unwrap().key, key);
    }

    #[test]
    fn public_url_validation() {
        for bad in [
            "",
            "ftp://x",
            "sync.example.com",
            "https://",
            "https://a/b c",
            "https://a/../x",
            "https://a//x",
            "https://a/x?y=1",
            "https://a?x=1",
            "https://u:p@a",
            "https://a:0",
        ] {
            assert!(Endpoint::from_public_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn bind_address_fallback() {
        assert_eq!(
            Endpoint::from_bind("0.0.0.0", 8787).authority,
            "localhost:8787"
        );
        assert_eq!(Endpoint::from_bind("::1", 1).authority, "[::1]:1");
        assert!(!Endpoint::from_bind("10.0.0.2", 80).tls);
    }

    #[test]
    fn debug_output_never_contains_the_secret() {
        let key = sample_key();
        let secret = key.rsplit(':').next().unwrap().to_owned();
        let inv = Invitation::new(Endpoint::from_bind("h", 1), key).unwrap();
        assert!(!format!("{inv:?}").contains(&secret));
    }

    #[test]
    fn banner_and_qr_render() {
        let inv = Invitation::new(Endpoint::from_bind("h", 1), sample_key()).unwrap();
        let b = banner(&inv, true, Path::new("/data/auth_keys"));
        assert!(b.contains(&inv.to_url()));
        assert!(b.contains("SECRET"));
        assert!(b.contains("FLICKSYNC_PUBLIC_URL"));
        assert!(qr(&inv.to_url()).is_some_and(|q| q.lines().count() > 10));
    }
}

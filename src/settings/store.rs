//! The settings file: `<data_dir>/settings.json`, written atomically with mode 0600.
//!
//! Keys are the names of the environment variables they replace and values are strings, so the
//! stored settings plug straight into the existing environment parsing.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "settings.json";
pub const SCHEMA_VERSION: u32 = 1;

/// Variable name -> value.
pub type Values = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Server,
    FlickSync,
    FlickDd,
}

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::Server, Scope::FlickSync, Scope::FlickDd];

    pub fn id(self) -> &'static str {
        match self {
            Scope::Server => "server",
            Scope::FlickSync => "flicksync",
            Scope::FlickDd => "flickdd",
        }
    }

    pub fn from_id(id: &str) -> Option<Scope> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }
}

fn schema_version() -> u32 {
    SCHEMA_VERSION
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    #[serde(default = "schema_version")]
    pub version: u32,
    /// Increases on every write (informational).
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub server: Values,
    #[serde(default)]
    pub flicksync: Values,
    #[serde(default)]
    pub flickdd: Values,
    /// Unknown top-level keys, kept when the file is rewritten.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for Stored {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            revision: 0,
            server: Values::new(),
            flicksync: Values::new(),
            flickdd: Values::new(),
            extra: BTreeMap::new(),
        }
    }
}

/// Values of secret (or unknown) settings, printed as `<redacted>`.
struct RedactedValues<'a>(&'a Values);

impl std::fmt::Debug for RedactedValues<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use super::fields::{Kind, field};
        f.debug_map()
            .entries(self.0.iter().map(|(name, value)| {
                let shown = match field(name) {
                    Some(fd) if fd.kind != Kind::Secret => value.as_str(),
                    _ => "<redacted>",
                };
                (name, shown)
            }))
            .finish()
    }
}

/// Manual `Debug`: stored secrets must never reach logs. Unknown top-level keys are listed by
/// name only.
impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stored")
            .field("version", &self.version)
            .field("revision", &self.revision)
            .field("server", &RedactedValues(&self.server))
            .field("flicksync", &RedactedValues(&self.flicksync))
            .field("flickdd", &RedactedValues(&self.flickdd))
            .field("extra", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Stored {
    pub fn scope(&self, s: Scope) -> &Values {
        match s {
            Scope::Server => &self.server,
            Scope::FlickSync => &self.flicksync,
            Scope::FlickDd => &self.flickdd,
        }
    }

    pub fn scope_mut(&mut self, s: Scope) -> &mut Values {
        match s {
            Scope::Server => &mut self.server,
            Scope::FlickSync => &mut self.flicksync,
            Scope::FlickDd => &mut self.flickdd,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error(
        "{path} is not usable settings ({reason}); fix it or move it away, it is never replaced silently"
    )]
    Corrupt { path: String, reason: String },
    #[error("cannot write {path}: {source}")]
    Write {
        path: String,
        source: std::io::Error,
    },
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// The stored settings, or the empty default when there is no file yet. An unreadable, empty,
/// malformed or newer-than-supported file is an error: it is never silently replaced.
pub fn load(dir: &Path) -> Result<Stored, StoreError> {
    let path = path_in(dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Stored::default()),
        Err(source) => {
            return Err(StoreError::Read {
                path: path.display().to_string(),
                source,
            });
        }
    };
    let corrupt = |reason: String| StoreError::Corrupt {
        path: path.display().to_string(),
        reason,
    };
    let stored: Stored = serde_json::from_str(&text).map_err(|e| corrupt(e.to_string()))?;
    if stored.version > SCHEMA_VERSION {
        return Err(corrupt(format!(
            "schema version {} is newer than the {SCHEMA_VERSION} this server understands",
            stored.version
        )));
    }
    Ok(stored)
}

/// Write the file atomically: a private sibling, `fsync`, then rename over the real one.
pub fn save(dir: &Path, stored: &Stored) -> Result<(), StoreError> {
    let path = path_in(dir);
    let werr = |source: std::io::Error| StoreError::Write {
        path: path.display().to_string(),
        source,
    };
    std::fs::create_dir_all(dir).map_err(werr)?;
    let tmp = dir.join(format!("{FILE_NAME}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut json = serde_json::to_string_pretty(stored).expect("settings always serialise");
    json.push('\n');
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).map_err(werr)?;
    f.write_all(json.as_bytes()).map_err(werr)?;
    f.sync_all().map_err(werr)?;
    drop(f);
    std::fs::rename(&tmp, &path).map_err(werr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::scratch_dir;

    #[test]
    fn a_missing_file_is_the_empty_default() {
        let d = scratch_dir("store-missing");
        assert_eq!(load(&d).unwrap(), Stored::default());
    }

    #[test]
    fn values_round_trip_and_unknown_keys_survive() {
        let d = scratch_dir("store-roundtrip");
        std::fs::write(
            path_in(&d),
            r#"{"version":1,"revision":3,"future":{"a":1},"flickdd":{"FLICKDD_MAX_PARALLEL":"20"}}"#,
        )
        .unwrap();
        let mut s = load(&d).unwrap();
        assert_eq!(s.revision, 3);
        assert_eq!(
            s.scope(Scope::FlickDd)
                .get("FLICKDD_MAX_PARALLEL")
                .map(String::as_str),
            Some("20")
        );
        s.revision += 1;
        s.scope_mut(Scope::Server)
            .insert("FLICKSYNC_LOG_LEVEL".into(), "debug".into());
        save(&d, &s).unwrap();
        let back = load(&d).unwrap();
        assert_eq!(back, s);
        assert!(back.extra.contains_key("future"), "unknown keys are kept");
    }

    #[test]
    fn an_empty_garbage_or_newer_file_is_refused_and_left_alone() {
        for (tag, content) in [
            ("empty", String::new()),
            ("garbage", "{not json".to_owned()),
            ("newer", format!(r#"{{"version":{}}}"#, SCHEMA_VERSION + 1)),
        ] {
            let d = scratch_dir(&format!("store-{tag}"));
            std::fs::write(path_in(&d), &content).unwrap();
            let err = load(&d).unwrap_err();
            assert!(matches!(err, StoreError::Corrupt { .. }), "{tag}: {err}");
            assert!(err.to_string().contains(FILE_NAME), "{tag}: names the file");
            assert_eq!(std::fs::read_to_string(path_in(&d)).unwrap(), content);
        }
    }

    #[test]
    fn a_stale_temp_file_does_not_block_a_save() {
        let d = scratch_dir("store-stale");
        std::fs::write(d.join(format!("{FILE_NAME}.tmp")), "junk from a crash").unwrap();
        let s = Stored {
            revision: 9,
            ..Stored::default()
        };
        save(&d, &s).unwrap();
        assert_eq!(load(&d).unwrap().revision, 9);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch_dir("store-mode");
        save(&d, &Stored::default()).unwrap();
        let mode = std::fs::metadata(path_in(&d)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn debug_never_prints_stored_secrets() {
        let mut s = Stored::default();
        s.server
            .insert("FLICKSYNC_AUTH_KEYS".into(), "k:s:s3cr3t-key".into());
        s.server
            .insert("FLICKSYNC_LOG_LEVEL".into(), "debug".into());
        s.flickdd
            .insert("FLICKDD_PLEX_TOKEN".into(), "s3cr3t-plex".into());
        s.flicksync
            .insert("SOMETHING_UNKNOWN".into(), "s3cr3t-unknown".into());
        s.extra
            .insert("future".into(), serde_json::json!("s3cr3t-extra"));
        let out = format!("{s:?}");
        assert!(!out.contains("s3cr3t"), "{out}");
        assert!(
            out.contains("debug") && out.contains("FLICKDD_PLEX_TOKEN"),
            "{out}"
        );
    }

    #[test]
    fn scope_ids_round_trip() {
        for s in Scope::ALL {
            assert_eq!(Scope::from_id(s.id()), Some(s));
        }
        assert_eq!(Scope::from_id("nope"), None);
    }
}

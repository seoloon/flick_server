//! Settings edited at runtime: the stored file layered over the environment.

pub mod fields;
pub mod store;

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard};

use serde::Serialize;
use tracing_subscriber::EnvFilter;

use crate::auth::Authenticator;
use crate::config::{Config, ConfigError, parse_bool};
use crate::dd::config::DdConfig;
use fields::{Field, Kind, field};
use store::{Scope, StoreError, Stored};

/// The process environment, or a map in tests.
pub type Env = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Longest value accepted for one setting.
const MAX_VALUE_LEN: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Panel,
    Environment,
    Default,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldView {
    pub name: &'static str,
    pub kind: &'static str,
    pub secret: bool,
    /// Current value; `None` for a secret.
    pub value: Option<String>,
    /// A value is set somewhere (panel or environment).
    pub set: bool,
    pub source: Source,
    /// Code default; `None` for a secret.
    pub default: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<&'static [&'static str]>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScopeView {
    pub scope: &'static str,
    pub revision: u64,
    pub fields: Vec<FieldView>,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("unknown setting {0}")]
    UnknownField(String),
    #[error("{0} is too long")]
    TooLong(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub struct Settings {
    dir: Option<PathBuf>,
    env: Env,
    stored: RwLock<Stored>,
}

impl Settings {
    /// `FLICKSYNC_DATA_DIR` (default `./data`): where `settings.json` and the key file live.
    pub fn data_dir_from_env(env: &Env) -> PathBuf {
        PathBuf::from(
            env("FLICKSYNC_DATA_DIR")
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "./data".to_owned()),
        )
    }

    pub fn open(dir: &Path, env: Env) -> Result<Arc<Self>, StoreError> {
        let stored = store::load(dir)?;
        Ok(Arc::new(Self {
            dir: Some(dir.to_path_buf()),
            env,
            stored: RwLock::new(stored),
        }))
    }

    /// No file: changes live in memory only.
    pub fn in_memory(env: Env) -> Arc<Self> {
        Arc::new(Self {
            dir: None,
            env,
            stored: RwLock::new(Stored::default()),
        })
    }

    fn read(&self) -> RwLockReadGuard<'_, Stored> {
        self.stored.read().unwrap_or_else(|e| e.into_inner())
    }

    fn lookup_in(&self, stored: &Stored, name: &str) -> Option<String> {
        if let Some(f) = field(name)
            && let Some(v) = stored.scope(f.scope).get(name)
        {
            return Some(v.clone());
        }
        (self.env)(name)
    }

    /// The effective value of `name`: stored, else environment.
    pub fn lookup(&self, name: &str) -> Option<String> {
        self.lookup_in(&self.read(), name)
    }

    /// The shared configuration (server and FlickSync) as seen through the stored settings.
    /// FlickDD is not part of it: see [`Settings::dd_config`].
    pub fn config(&self) -> Result<Config, ConfigError> {
        Config::from_lookup(&|k| self.lookup(k))
    }

    /// FlickDD's configuration, parsed and validated on its own so that an invalid or
    /// incomplete FlickDD setting only ever fails FlickDD.
    pub fn dd_config(&self) -> Result<DdConfig, ConfigError> {
        DdConfig::from_lookup(&|k| self.lookup(k))
    }

    pub fn revision(&self) -> u64 {
        self.read().revision
    }

    fn source_in(&self, stored: &Stored, f: &Field) -> Source {
        if stored.scope(f.scope).contains_key(f.name) {
            Source::Panel
        } else if (self.env)(f.name).is_some_and(|v| !v.trim().is_empty()) {
            Source::Environment
        } else {
            Source::Default
        }
    }

    fn view_of(&self, stored: &Stored, scope: Scope) -> ScopeView {
        let fields = fields::FIELDS
            .iter()
            .filter(|f| f.scope == scope)
            .map(|f| {
                let current = self
                    .lookup_in(stored, f.name)
                    .filter(|v| !v.trim().is_empty());
                let secret = f.kind == Kind::Secret;
                FieldView {
                    name: f.name,
                    kind: f.kind.label(),
                    secret,
                    value: (!secret)
                        .then(|| current.clone().unwrap_or_else(|| f.default.to_owned())),
                    set: current.is_some(),
                    source: self.source_in(stored, f),
                    default: (!secret).then_some(f.default),
                    choices: f.kind.choices(),
                }
            })
            .collect();
        ScopeView {
            scope: scope.id(),
            revision: stored.revision,
            fields,
        }
    }

    pub fn view(&self, scope: Scope) -> ScopeView {
        self.view_of(&self.read(), scope)
    }

    /// Changes whenever any effective value of `scope` changes (stored or environment).
    pub fn fingerprint(&self, scope: Scope) -> u64 {
        let stored = self.read();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for f in fields::FIELDS.iter().filter(|f| f.scope == scope) {
            f.name.hash(&mut h);
            self.lookup_in(&stored, f.name).hash(&mut h);
        }
        h.finish()
    }

    fn enabled_var(scope: Scope) -> Option<&'static str> {
        match scope {
            Scope::FlickSync => Some("FLICKSYNC_ENABLED"),
            Scope::FlickDd => Some("FLICKDD_ENABLED"),
            Scope::Server => None,
        }
    }

    /// The module's on/off switch. An unparseable value counts as "on": the module is then
    /// started and fails with a message naming the variable, instead of silently staying off.
    pub fn is_enabled(&self, scope: Scope) -> bool {
        Self::enabled_var(scope)
            .is_some_and(|name| parse_bool(&|k| self.lookup(k), name, false).unwrap_or(true))
    }

    fn commit(&self, guard: &mut Stored, mut next: Stored) -> Result<(), SettingsError> {
        next.revision += 1;
        if let Some(dir) = &self.dir {
            store::save(dir, &next)?;
        }
        *guard = next;
        Ok(())
    }

    /// Persist the module's on/off switch (no validation of the other settings).
    pub fn set_enabled(&self, scope: Scope, enabled: bool) -> Result<(), SettingsError> {
        let name = Self::enabled_var(scope)
            .ok_or_else(|| SettingsError::UnknownField("enabled".into()))?;
        let mut guard = self.stored.write().unwrap_or_else(|e| e.into_inner());
        let mut next = guard.clone();
        next.scope_mut(scope)
            .insert(name.to_owned(), enabled.to_string());
        self.commit(&mut guard, next)
    }

    /// Apply `patch` (`Some` sets, `None` removes) to `scope`. The whole resulting configuration
    /// is validated first; on any error nothing is written.
    pub fn put(
        &self,
        scope: Scope,
        patch: BTreeMap<String, Option<String>>,
    ) -> Result<ScopeView, SettingsError> {
        for (name, value) in &patch {
            match field(name) {
                Some(f) if f.scope == scope => {}
                _ => return Err(SettingsError::UnknownField(name.clone())),
            }
            if value.as_ref().is_some_and(|v| v.len() > MAX_VALUE_LEN) {
                return Err(SettingsError::TooLong(name.clone()));
            }
        }
        let mut guard = self.stored.write().unwrap_or_else(|e| e.into_inner());
        let mut next = guard.clone();
        for (name, value) in patch {
            match value {
                Some(v) => {
                    next.scope_mut(scope).insert(name, v);
                }
                None => {
                    next.scope_mut(scope).remove(&name);
                }
            }
        }
        self.validate(&next, scope)?;
        self.commit(&mut guard, next)?;
        Ok(self.view_of(&guard, scope))
    }

    /// Validate `scope` as it would be after the change. Server and FlickSync settings share one
    /// configuration and are checked together; FlickDD settings are independent of it, so
    /// neither side can block the other's saves.
    fn validate(&self, next: &Stored, scope: Scope) -> Result<(), SettingsError> {
        let look = |k: &str| self.lookup_in(next, k);
        let invalid = |e: &dyn std::fmt::Display| SettingsError::Invalid(e.to_string());
        match scope {
            Scope::Server => {
                let cfg = Config::from_lookup(&look).map_err(|e| invalid(&e))?;
                Authenticator::new(&cfg.auth)
                    .map_err(|e| SettingsError::Invalid(format!("FLICKSYNC_AUTH_KEYS: {e}")))?;
                EnvFilter::try_new(&cfg.log_level)
                    .map_err(|e| SettingsError::Invalid(format!("FLICKSYNC_LOG_LEVEL: {e}")))?;
            }
            Scope::FlickSync => {
                Config::from_lookup(&look).map_err(|e| invalid(&e))?;
            }
            Scope::FlickDd => {
                let enabled =
                    parse_bool(&look, "FLICKDD_ENABLED", false).map_err(|e| invalid(&e))?;
                DdConfig::check(&look, enabled).map_err(|e| invalid(&e))?;
            }
        }
        Ok(())
    }
}

/// A fresh empty directory under the system temp dir (for tests; not removed afterwards).
pub fn scratch_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "flick-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("cannot create a scratch directory");
    dir
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env_of(vars: &[(&str, &str)]) -> Env {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Arc::new(move |k| m.get(k).cloned())
    }

    fn patch(pairs: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.map(str::to_owned)))
            .collect()
    }

    fn source(s: &Settings, scope: Scope, name: &str) -> Source {
        s.view(scope)
            .fields
            .iter()
            .find(|f| f.name == name)
            .unwrap()
            .source
    }

    #[test]
    fn stored_beats_environment_beats_default() {
        let s = Settings::in_memory(env_of(&[("FLICKSYNC_MAX_ROOM_SIZE", "5")]));
        assert_eq!(s.lookup("FLICKSYNC_MAX_ROOM_SIZE").as_deref(), Some("5"));
        assert_eq!(
            source(&s, Scope::FlickSync, "FLICKSYNC_MAX_ROOM_SIZE"),
            Source::Environment
        );
        assert_eq!(s.config().unwrap().manager.room.max_participants, 5);

        s.put(
            Scope::FlickSync,
            patch(&[("FLICKSYNC_MAX_ROOM_SIZE", Some("7"))]),
        )
        .unwrap();
        assert_eq!(s.config().unwrap().manager.room.max_participants, 7);
        assert_eq!(
            source(&s, Scope::FlickSync, "FLICKSYNC_MAX_ROOM_SIZE"),
            Source::Panel
        );

        s.put(
            Scope::FlickSync,
            patch(&[("FLICKSYNC_MAX_ROOM_SIZE", None)]),
        )
        .unwrap();
        assert_eq!(s.config().unwrap().manager.room.max_participants, 5);
        assert_eq!(
            source(&s, Scope::FlickSync, "FLICKSYNC_MAX_ROOM_SIZE"),
            Source::Environment
        );

        assert_eq!(
            source(&s, Scope::FlickSync, "FLICKSYNC_MAX_ROOMS"),
            Source::Default
        );
    }

    #[test]
    fn invalid_values_are_refused_and_nothing_is_written() {
        let dir = scratch_dir("settings-invalid");
        let s = Settings::open(&dir, env_of(&[])).unwrap();
        let err = s
            .put(
                Scope::FlickSync,
                patch(&[("FLICKSYNC_MAX_ROOM_SIZE", Some("0"))]),
            )
            .unwrap_err();
        assert!(err.to_string().contains("FLICKSYNC_MAX_ROOM_SIZE"), "{err}");
        assert_eq!(s.revision(), 0);
        assert!(!store::path_in(&dir).exists());
        // A malformed signing-key list and a bad log filter are refused too.
        assert!(
            s.put(
                Scope::Server,
                patch(&[("FLICKSYNC_AUTH_KEYS", Some("no-colons"))])
            )
            .is_err()
        );
        assert!(
            s.put(
                Scope::Server,
                patch(&[("FLICKSYNC_LOG_LEVEL", Some("[[bad"))])
            )
            .is_err()
        );
    }

    #[test]
    fn flickdd_settings_are_validated_while_the_module_is_disabled() {
        let s = Settings::in_memory(env_of(&[]));
        assert!(
            s.put(
                Scope::FlickDd,
                patch(&[("FLICKDD_MAX_PARALLEL", Some("0"))])
            )
            .is_err()
        );
        assert!(
            s.put(Scope::FlickDd, patch(&[("FLICKDD_CHUNK_MB", Some("128"))]))
                .is_err()
        );
        assert!(
            s.put(
                Scope::FlickDd,
                patch(&[("FLICKDD_JELLYFIN_URL", Some("http://jf"))])
            )
            .is_err()
        );
        s.put(
            Scope::FlickDd,
            patch(&[
                ("FLICKDD_JELLYFIN_URL", Some("http://jf:8096")),
                ("FLICKDD_JELLYFIN_API_KEY", Some("k")),
            ]),
        )
        .unwrap();
        assert!(
            !s.is_enabled(Scope::FlickDd),
            "saving settings never enables the module"
        );
    }

    #[test]
    fn unknown_or_foreign_fields_are_rejected() {
        let s = Settings::in_memory(env_of(&[]));
        for name in ["NOPE", "FLICKSYNC_HOST", "FLICKDD_MAX_GLOBAL"] {
            assert!(
                matches!(
                    s.put(Scope::FlickSync, patch(&[(name, Some("1"))])),
                    Err(SettingsError::UnknownField(_))
                ),
                "{name}"
            );
        }
        let long = "x".repeat(MAX_VALUE_LEN + 1);
        assert!(matches!(
            s.put(
                Scope::Server,
                patch(&[("FLICKSYNC_PUBLIC_URL", Some(long.as_str()))])
            ),
            Err(SettingsError::TooLong(_))
        ));
    }

    #[test]
    fn secrets_never_appear_in_a_view() {
        let s = Settings::in_memory(env_of(&[("FLICKDD_PLEX_TOKEN", "env-plex-secret")]));
        s.put(
            Scope::FlickDd,
            patch(&[
                ("FLICKDD_JELLYFIN_URL", Some("http://jf:8096")),
                ("FLICKDD_JELLYFIN_API_KEY", Some("panel-jf-secret")),
                ("FLICKDD_PLEX_URL", Some("http://plex:32400")),
            ]),
        )
        .unwrap();
        let json = serde_json::to_string(&s.view(Scope::FlickDd)).unwrap();
        assert!(
            !json.contains("panel-jf-secret") && !json.contains("env-plex-secret"),
            "{json}"
        );
        let key = s
            .view(Scope::FlickDd)
            .fields
            .into_iter()
            .find(|f| f.name == "FLICKDD_JELLYFIN_API_KEY")
            .unwrap();
        assert!(key.secret && key.set && key.value.is_none());
    }

    #[test]
    fn the_fingerprint_follows_the_scope_it_belongs_to() {
        let s = Settings::in_memory(env_of(&[]));
        let (sync0, dd0) = (
            s.fingerprint(Scope::FlickSync),
            s.fingerprint(Scope::FlickDd),
        );
        s.put(
            Scope::FlickSync,
            patch(&[("FLICKSYNC_MAX_ROOM_SIZE", Some("9"))]),
        )
        .unwrap();
        assert_ne!(s.fingerprint(Scope::FlickSync), sync0);
        assert_eq!(s.fingerprint(Scope::FlickDd), dd0);
    }

    #[test]
    fn enabled_flags_persist_and_survive_a_reopen() {
        let dir = scratch_dir("settings-enabled");
        let s = Settings::open(&dir, env_of(&[])).unwrap();
        assert!(!s.is_enabled(Scope::FlickSync));
        s.set_enabled(Scope::FlickSync, true).unwrap();
        assert!(s.is_enabled(Scope::FlickSync));
        let again = Settings::open(&dir, env_of(&[])).unwrap();
        assert!(again.is_enabled(Scope::FlickSync));
        assert!(!again.is_enabled(Scope::FlickDd));
        // `FLICKDD_ENABLED=true` in the environment is the fallback for an existing install.
        let legacy = Settings::in_memory(env_of(&[("FLICKDD_ENABLED", "true")]));
        assert!(legacy.is_enabled(Scope::FlickDd));
    }

    #[test]
    fn an_unparseable_enabled_flag_is_refused_and_nothing_is_written() {
        let dir = scratch_dir("settings-bad-flag");
        let s = Settings::open(&dir, env_of(&[])).unwrap();
        for (scope, name) in [
            (Scope::FlickDd, "FLICKDD_ENABLED"),
            (Scope::FlickSync, "FLICKSYNC_ENABLED"),
        ] {
            let err = s.put(scope, patch(&[(name, Some("perhaps"))])).unwrap_err();
            assert!(matches!(err, SettingsError::Invalid(_)), "{name}: {err}");
            assert!(err.to_string().contains(name), "{err}");
            assert_eq!(s.lookup(name), None, "{name}");
        }
        assert_eq!(s.revision(), 0);
        assert!(!store::path_in(&dir).exists());
    }

    #[test]
    fn an_unparseable_enabled_flag_counts_as_enabled_so_the_start_reports_it() {
        for name in ["FLICKDD_ENABLED", "FLICKSYNC_ENABLED"] {
            let s = Settings::in_memory(env_of(&[(name, "perhaps")]));
            let scope = if name == "FLICKDD_ENABLED" {
                Scope::FlickDd
            } else {
                Scope::FlickSync
            };
            assert!(s.is_enabled(scope), "{name}");
        }
        let s = Settings::in_memory(env_of(&[("FLICKDD_ENABLED", "perhaps")]));
        let err = s.dd_config().unwrap_err();
        assert!(err.to_string().contains("FLICKDD_ENABLED"), "{err}");
    }
}

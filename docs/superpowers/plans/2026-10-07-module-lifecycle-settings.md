# Module lifecycle and panel-managed settings: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** FlickSync and FlickDD become modules that an operator can start, stop and reload at runtime through the admin API, with their settings stored in `settings.json` (layered over the environment) instead of being read once from env vars.

**Architecture:** The router stays fixed. `AppState` holds one swappable `Slot` per module (`SyncRuntime`, `DdState`) plus a replaceable `ServerRuntime` (auth keys, public URL, CORS, metrics). Stored settings are a flat `NAME -> string` map per scope, exposed through the same lookup function the config parser already takes, so every existing validation rule applies unchanged.

**Tech Stack:** Rust 2024 (rust-version 1.88), axum 0.8, tokio, serde_json, sha2 0.11 (HMAC-SHA256 written by hand), tower-http 0.7 CORS, tracing-subscriber `reload`.

**Spec:** `docs/superpowers/specs/2026-10-07-module-lifecycle-settings-design.md`

## Global Constraints

Every task's requirements include these (copied from the spec):

- No Docker socket anywhere; no new dependency (HMAC-SHA256 is built by hand on the existing `sha2`).
- Settings file: `<DATA_DIR>/settings.json`, owned by the server user, mode `0600`, written atomically (temp file, `fsync`, rename); keys are the names of the environment variables they replace; every value is a string.
- Precedence: stored setting > environment variable > code default. The environment keeps working as a fallback.
- A module is `stopped`, `running` or `failed`; `start` / `stop` are idempotent; one `Mutex` per module serialises `start`, `stop`, `reload`.
- A failing `start` never stops the process; invalid **boot** variables (`FLICKSYNC_HOST`, `FLICKSYNC_PORT`, ...) still abort.
- Module stopped: FlickSync routes answer `503` `MODULE_DISABLED`; FlickDD routes keep the documented `404`; admin `dd/*` GETs answer `404`; `/health` stays `200`; `/ready` means "signing keys loaded".
- Secrets (Jellyfin API key, Plex token, metrics token, JWT keys) are never returned by the API (only `"set": true`), never logged. On write: omitted keeps, a string replaces, `null` clears.
- Admin token = `hex(HMAC-SHA256(key = PANEL_PASSWORD, msg = "flick-admin-api-v1"))`; the admin API answers `404` unless `PANEL_PASSWORD` has at least 10 characters (or the legacy `FLICKSYNC_ADMIN_TOKEN` is set); comparison is constant-time; checked against the RFC 4231 vectors.
- A corrupt `settings.json` at boot makes the server refuse to start and name the file; it is never replaced by defaults.
- After every task: `cargo fmt --check`, `cargo clippy --all-targets` and `cargo test` are green.
- Commit messages end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

## Review Focus

Inputs and conditions the spec implies that a user is likely to hit; each has a test in the task that owns the code:

1. `settings.json` empty, truncated or from a newer version at boot: refuse to start, name the file, keep the file (Task 1 store tests, Task 4 `AppState::new` error).
2. Several `start` / `stop` / `reload` calls at the same time: serialised, never two runtimes, final state consistent (Task 5).
3. A request in flight while its module is stopped: it finishes safely on its own `Arc`; new requests get `503` (Task 5).
4. Secrets: absent from every response, kept when omitted, replaced by a string, cleared by `null` (Task 8).
5. Server reload with a changed key list: a removed key stops working, a malformed list is refused and the old keys stay in force (Task 6).

Also covered: settings of a **disabled** module are still validated on save (Task 2); `reload` of a disabled module must not start it (Task 5).

**Interim note for the panel.** The panel (sub-project 2) still calls `/admin/v1/rooms` and `/stats` with the legacy token. When FlickSync is stopped those routes now answer `503 MODULE_DISABLED`, which the current panel shows as a generic error. That is accepted until sub-project 2.

## File Structure

| File | Responsibility |
|---|---|
| `src/settings/store.rs` (new) | `Scope`, `Stored`, `load` / `save` of `settings.json` (atomic, 0600, corrupt refused) |
| `src/settings/fields.rs` (new) | Catalogue of editable variables (`FIELDS`), boot-only list, `Kind` |
| `src/settings/mod.rs` (new) | `Settings`: layered lookup, views, `put` with validation, `set_enabled`, `fingerprint`; `scratch_dir` for tests |
| `src/modules.rs` (new) | `ModuleId`, `Slot<T>`, `SlotState`, `ModuleStatus` |
| `src/admin_token.rs` (new) | HMAC-SHA256, token derivation, `AdminTokens` |
| `src/app.rs` | `AppState` with slots, `SyncRuntime`, `ServerRuntime`, lifecycle, sweeper, `serve` |
| `src/config/mod.rs`, `src/dd/config.rs` | `FLICKSYNC_ENABLED`, `PANEL_PASSWORD`, `DdConfig::check` |
| `src/api/*.rs`, `src/websocket/*.rs` | Handlers read runtimes through `state.sync()`, `state.dd()`, `state.server()` |
| `src/api/admin.rs` | Module and settings routes |
| `src/main.rs` | Opens `Settings`, log-level reload handle |
| `tests/common/mod.rs`, `tests/lifecycle.rs` (new), `tests/settings_api.rs` (new) | Harness and integration tests |

---

### Task 1: Settings file store

**Files:**
- Create: `src/settings/mod.rs`, `src/settings/store.rs`
- Modify: `src/lib.rs` (add `pub mod settings;`)

**Interfaces:**
- Produces:
  - `settings::scratch_dir(tag: &str) -> PathBuf` (fresh empty dir under the system temp dir, for tests)
  - `store::Scope { Server, FlickSync, FlickDd }` with `Scope::ALL: [Scope; 3]`, `id() -> &'static str` (`"server"`, `"flicksync"`, `"flickdd"`), `Scope::from_id(&str) -> Option<Scope>`
  - `store::Values = BTreeMap<String, String>`
  - `store::Stored { version: u32, revision: u64, server, flicksync, flickdd: Values, extra }` with `scope(&self, Scope) -> &Values`, `scope_mut(&mut self, Scope) -> &mut Values`, `Default`
  - `store::StoreError { Read, Corrupt, Write }`
  - `store::load(dir: &Path) -> Result<Stored, StoreError>`, `store::save(dir: &Path, &Stored) -> Result<(), StoreError>`, `store::path_in(dir) -> PathBuf`, `store::FILE_NAME`, `store::SCHEMA_VERSION`

- [ ] **Step 1: Create the module skeleton with the failing tests**

`src/lib.rs`: add `pub mod settings;` in alphabetical position (after `room`, before `sync`).

`src/settings/mod.rs`:

```rust
//! Settings edited at runtime: the stored file layered over the environment.

pub mod store;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

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
```

`src/settings/store.rs`: write only the `tests` module for now (the items it uses are implemented in Step 3, so the build fails until then, which is the expected red):

```rust
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
            s.scope(Scope::FlickDd).get("FLICKDD_MAX_PARALLEL").map(String::as_str),
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
        let mut s = Stored::default();
        s.revision = 9;
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
    fn scope_ids_round_trip() {
        for s in Scope::ALL {
            assert_eq!(Scope::from_id(s.id()), Some(s));
        }
        assert_eq!(Scope::from_id("nope"), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib settings::store`
Expected: compile error, `cannot find ... Stored / load / save / Scope` (nothing implemented yet).

- [ ] **Step 3: Write the implementation (above the `tests` module in `store.rs`)**

```rust
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib settings::store`
Expected: PASS (6 tests; the `unix` one only on Linux/macOS).

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -3 && cargo test 2>&1 | grep -E "^test result|FAILED"
git add src/settings src/lib.rs
git commit -m "feat(settings): settings.json store, atomic and private

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Field catalogue, layered lookup, validated writes

**Files:**
- Create: `src/settings/fields.rs`
- Modify: `src/settings/mod.rs`, `src/config/mod.rs`, `src/dd/config.rs`

**Interfaces:**
- Consumes: `store::{Scope, Stored, StoreError, load, save}` from Task 1.
- Produces:
  - `fields::{Kind, Field, FIELDS, BOOT_ONLY, field(name) -> Option<&'static Field>}`
  - `settings::{Env, Settings, Source, FieldView, ScopeView, SettingsError}`
  - `Settings::open(dir: &Path, env: Env) -> Result<Arc<Settings>, StoreError>`
  - `Settings::in_memory(env: Env) -> Arc<Settings>`
  - `Settings::data_dir_from_env(env: &Env) -> PathBuf`
  - `Settings::lookup(&self, name: &str) -> Option<String>`
  - `Settings::config(&self) -> Result<Config, ConfigError>`
  - `Settings::view(&self, Scope) -> ScopeView`
  - `Settings::put(&self, Scope, BTreeMap<String, Option<String>>) -> Result<ScopeView, SettingsError>`
  - `Settings::set_enabled(&self, Scope, bool) -> Result<(), SettingsError>`
  - `Settings::is_enabled(&self, Scope) -> bool`
  - `Settings::fingerprint(&self, Scope) -> u64`
  - `Settings::revision(&self) -> u64`
  - `Config.sync_enabled: bool` (`FLICKSYNC_ENABLED`, default false)
  - `DdConfig::check(env: Lookup, require_backend: bool) -> Result<(), ConfigError>`

- [ ] **Step 1: Write the failing tests**

In `src/config/mod.rs`, inside `mod tests`, add:

```rust
    #[test]
    fn flicksync_is_disabled_unless_enabled_explicitly() {
        assert!(!cfg(&[]).unwrap().sync_enabled);
        assert!(cfg(&[("FLICKSYNC_ENABLED", "true")]).unwrap().sync_enabled);
        assert!(cfg(&[("FLICKSYNC_ENABLED", "perhaps")]).is_err());
    }
```

In `src/dd/config.rs`, inside `mod tests`, add:

```rust
    #[test]
    fn check_validates_even_while_disabled() {
        let m = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        // Limits are checked although FLICKDD_ENABLED is not set.
        assert!(DdConfig::check(&m(&[("FLICKDD_MAX_PARALLEL", "0")]), false).is_err());
        // No backend is fine when the module is not enabled, refused when it is.
        assert!(DdConfig::check(&m(&[]), false).is_ok());
        assert!(DdConfig::check(&m(&[]), true).is_err());
        assert!(
            DdConfig::check(
                &m(&[
                    ("FLICKDD_JELLYFIN_URL", "http://jf:8096"),
                    ("FLICKDD_JELLYFIN_API_KEY", "k")
                ]),
                true
            )
            .is_ok()
        );
        // A half-configured backend is always an error.
        assert!(DdConfig::check(&m(&[("FLICKDD_PLEX_URL", "http://plex")]), false).is_err());
    }
```

Create `src/settings/fields.rs` with only the drift tests at first:

```rust
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::Config;

    fn quoted_names(src: &str) -> Vec<&str> {
        src.split('"')
            .skip(1)
            .step_by(2)
            .filter(|s| {
                (s.starts_with("FLICKSYNC_") || s.starts_with("FLICKDD_") || *s == "PANEL_PASSWORD")
                    && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            })
            .collect()
    }

    #[test]
    fn every_variable_the_config_reads_is_catalogued_or_boot_only() {
        for src in [include_str!("../config/mod.rs"), include_str!("../dd/config.rs")] {
            for name in quoted_names(src) {
                assert!(
                    field(name).is_some() || BOOT_ONLY.contains(&name),
                    "{name} is read by the config but is neither in FIELDS nor in BOOT_ONLY"
                );
            }
        }
    }

    #[test]
    fn a_name_is_listed_once() {
        let mut seen = std::collections::HashSet::new();
        for f in FIELDS {
            assert!(seen.insert(f.name), "{} is listed twice", f.name);
            assert!(!BOOT_ONLY.contains(&f.name), "{} is both editable and boot-only", f.name);
        }
    }

    #[test]
    fn catalogue_defaults_match_the_code_defaults() {
        let base = [
            ("FLICKDD_ENABLED", "true"),
            ("FLICKDD_JELLYFIN_URL", "http://jf:8096"),
            ("FLICKDD_JELLYFIN_API_KEY", "k"),
        ];
        let build = |extra: Option<(&str, &str)>| {
            let mut m: HashMap<String, String> = base
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            if let Some((k, v)) = extra {
                m.insert(k.into(), v.into());
            }
            format!("{:?}", Config::from_lookup(&move |k| m.get(k).cloned()).unwrap())
        };
        let reference = build(None);
        for f in FIELDS.iter().filter(|f| {
            f.kind != Kind::Secret
                && !f.default.is_empty()
                && !base.iter().any(|(k, _)| *k == f.name)
        }) {
            assert_eq!(build(Some((f.name, f.default))), reference, "{}", f.name);
        }
    }
}
```

In `src/settings/mod.rs`, add a `#[cfg(test)] mod tests` with the behaviour tests:

```rust
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
        assert_eq!(source(&s, Scope::FlickSync, "FLICKSYNC_MAX_ROOM_SIZE"), Source::Environment);
        assert_eq!(s.config().unwrap().manager.room.max_participants, 5);

        s.put(Scope::FlickSync, patch(&[("FLICKSYNC_MAX_ROOM_SIZE", Some("7"))])).unwrap();
        assert_eq!(s.config().unwrap().manager.room.max_participants, 7);
        assert_eq!(source(&s, Scope::FlickSync, "FLICKSYNC_MAX_ROOM_SIZE"), Source::Panel);

        s.put(Scope::FlickSync, patch(&[("FLICKSYNC_MAX_ROOM_SIZE", None)])).unwrap();
        assert_eq!(s.config().unwrap().manager.room.max_participants, 5);
        assert_eq!(source(&s, Scope::FlickSync, "FLICKSYNC_MAX_ROOM_SIZE"), Source::Environment);

        assert_eq!(source(&s, Scope::FlickSync, "FLICKSYNC_MAX_ROOMS"), Source::Default);
    }

    #[test]
    fn invalid_values_are_refused_and_nothing_is_written() {
        let dir = scratch_dir("settings-invalid");
        let s = Settings::open(&dir, env_of(&[])).unwrap();
        let err = s
            .put(Scope::FlickSync, patch(&[("FLICKSYNC_MAX_ROOM_SIZE", Some("0"))]))
            .unwrap_err();
        assert!(err.to_string().contains("FLICKSYNC_MAX_ROOM_SIZE"), "{err}");
        assert_eq!(s.revision(), 0);
        assert!(!store::path_in(&dir).exists());
        // A malformed signing-key list and a bad log filter are refused too.
        assert!(s.put(Scope::Server, patch(&[("FLICKSYNC_AUTH_KEYS", Some("no-colons"))])).is_err());
        assert!(s.put(Scope::Server, patch(&[("FLICKSYNC_LOG_LEVEL", Some("[[bad"))])).is_err());
    }

    #[test]
    fn flickdd_settings_are_validated_while_the_module_is_disabled() {
        let s = Settings::in_memory(env_of(&[]));
        assert!(s.put(Scope::FlickDd, patch(&[("FLICKDD_MAX_PARALLEL", Some("0"))])).is_err());
        assert!(s.put(Scope::FlickDd, patch(&[("FLICKDD_CHUNK_MB", Some("128"))])).is_err());
        assert!(s.put(Scope::FlickDd, patch(&[("FLICKDD_JELLYFIN_URL", Some("http://jf"))])).is_err());
        s.put(
            Scope::FlickDd,
            patch(&[
                ("FLICKDD_JELLYFIN_URL", Some("http://jf:8096")),
                ("FLICKDD_JELLYFIN_API_KEY", Some("k")),
            ]),
        )
        .unwrap();
        assert!(!s.is_enabled(Scope::FlickDd), "saving settings never enables the module");
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
            s.put(Scope::Server, patch(&[("FLICKSYNC_PUBLIC_URL", Some(long.as_str()))])),
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
        assert!(!json.contains("panel-jf-secret") && !json.contains("env-plex-secret"), "{json}");
        let key = s.view(Scope::FlickDd).fields.into_iter()
            .find(|f| f.name == "FLICKDD_JELLYFIN_API_KEY").unwrap();
        assert!(key.secret && key.set && key.value.is_none());
    }

    #[test]
    fn the_fingerprint_follows_the_scope_it_belongs_to() {
        let s = Settings::in_memory(env_of(&[]));
        let (sync0, dd0) = (s.fingerprint(Scope::FlickSync), s.fingerprint(Scope::FlickDd));
        s.put(Scope::FlickSync, patch(&[("FLICKSYNC_MAX_ROOM_SIZE", Some("9"))])).unwrap();
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
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib settings config::tests::flicksync_is_disabled dd::config::tests::check_validates`
Expected: compile errors (missing `fields`, `Settings`, `sync_enabled`, `DdConfig::check`).

- [ ] **Step 3: Implement**

`src/config/mod.rs`: add `pub sync_enabled: bool,` to `Config` (after `shutdown_grace_secs`, with the doc comment `/// FlickSync starts at boot (FLICKSYNC_ENABLED, default false).`) and in `from_lookup`'s final struct literal add `sync_enabled: parse_bool(env, "FLICKSYNC_ENABLED", false)?,`.

`src/dd/config.rs`: split `validate` and add `check`:

```rust
    /// Validate a configuration that is not being started: every value is read and checked
    /// like an enabled one, and a missing backend is only an error when `require_backend`.
    pub fn check(env: Lookup, require_backend: bool) -> Result<(), ConfigError> {
        let c = Self::read(env, true)?;
        if require_backend {
            c.validate_backend()?;
        }
        c.validate_limits()
    }

    fn validate_backend(&self) -> Result<(), ConfigError> {
        if self.jellyfin.is_none() && self.plex.is_none() {
            return Err(ConfigError::Inconsistent(
                "FLICKDD_ENABLED=true requires at least one backend (Jellyfin or Plex)".to_owned(),
            ));
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), ConfigError> {
        self.validate_backend()?;
        self.validate_limits()
    }
```

and turn the body of the old `validate` after the backend check into `fn validate_limits(&self) -> Result<(), ConfigError>` (same `inconsistent` closure and the same four `if` blocks, ending in `Ok(())`).

`src/settings/fields.rs`, above the tests:

```rust
//! Catalogue of the variables the panel can edit, with their scope and code default.
//!
//! A drift test checks that every variable the config parsers read is listed here or in
//! `BOOT_ONLY`, and that each default below equals the one the code applies.

use super::store::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Float,
    Text,
    Choice(&'static [&'static str]),
    List,
    Secret,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Bool => "bool",
            Kind::Int => "int",
            Kind::Float => "float",
            Kind::Text => "text",
            Kind::Choice(_) => "choice",
            Kind::List => "list",
            Kind::Secret => "secret",
        }
    }

    pub fn choices(self) -> Option<&'static [&'static str]> {
        match self {
            Kind::Choice(c) => Some(c),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub scope: Scope,
    pub kind: Kind,
    /// The value the code applies when nothing is set (empty = unset).
    pub default: &'static str,
}

const fn f(name: &'static str, scope: Scope, kind: Kind, default: &'static str) -> Field {
    Field { name, scope, kind, default }
}

use Kind::*;
use Scope::{FlickDd as Dd, FlickSync as Sy, Server as Srv};

pub const FIELDS: &[Field] = &[
    // --- server
    f("FLICKSYNC_PUBLIC_URL", Srv, Text, ""),
    f("FLICKSYNC_AUTH_KEYS", Srv, Secret, ""),
    f("FLICKSYNC_AUTH_AUDIENCE", Srv, Text, "flicksync"),
    f("FLICKSYNC_AUTH_MAX_TOKEN_TTL", Srv, Int, "86400"),
    f("FLICKSYNC_AUTH_LEEWAY", Srv, Int, "30"),
    f("FLICKSYNC_CORS_ORIGINS", Srv, List, ""),
    f("FLICKSYNC_METRICS_ENABLED", Srv, Bool, "false"),
    f("FLICKSYNC_METRICS_TOKEN", Srv, Secret, ""),
    f("FLICKSYNC_LOG_LEVEL", Srv, Text, "info"),
    // --- FlickSync
    f("FLICKSYNC_ENABLED", Sy, Bool, "false"),
    f("FLICKSYNC_MAX_ROOM_SIZE", Sy, Int, "100"),
    f("FLICKSYNC_MAX_ROOMS", Sy, Int, "10000"),
    f("FLICKSYNC_ROOM_CREATE_PER_MINUTE", Sy, Int, "6"),
    f("FLICKSYNC_ROOM_TIMEOUT", Sy, Int, "60"),
    f("FLICKSYNC_ROOM_IDLE_TIMEOUT", Sy, Int, "43200"),
    f("FLICKSYNC_RECONNECT_GRACE", Sy, Int, "30"),
    f("FLICKSYNC_CONNECT_GRACE", Sy, Int, "60"),
    f("FLICKSYNC_HOST_LEAVE_POLICY", Sy, Choice(&["transfer", "close"]), "transfer"),
    f("FLICKSYNC_DEFAULT_CONTROL_MODE", Sy, Choice(&["everyone", "host_only"]), "everyone"),
    f("FLICKSYNC_SYNC_DRIFT_IGNORE", Sy, Float, "100"),
    f("FLICKSYNC_SYNC_DRIFT_SOFT", Sy, Float, "500"),
    f("FLICKSYNC_SYNC_DRIFT_HARD", Sy, Float, "1500"),
    f("FLICKSYNC_SYNC_RATE_SOFT", Sy, Float, "0.03"),
    f("FLICKSYNC_SYNC_RATE_STRONG", Sy, Float, "0.08"),
    f("FLICKSYNC_SYNC_SEEK_COOLDOWN_MS", Sy, Int, "3000"),
    f("FLICKSYNC_SYNC_HEARTBEAT", Sy, Int, "10"),
    f("FLICKSYNC_RATE_MIN", Sy, Float, "0.25"),
    f("FLICKSYNC_RATE_MAX", Sy, Float, "4.0"),
    f("FLICKSYNC_MAX_POSITION", Sy, Int, "604800"),
    f("FLICKSYNC_CHAT_ENABLED", Sy, Bool, "true"),
    f("FLICKSYNC_CHAT_MAX_LENGTH", Sy, Int, "500"),
    f("FLICKSYNC_CHAT_HISTORY", Sy, Int, "100"),
    f("FLICKSYNC_CHAT_RATE_PER_SEC", Sy, Float, "1"),
    f("FLICKSYNC_CHAT_BURST", Sy, Int, "5"),
    f("FLICKSYNC_MAX_CONNECTIONS", Sy, Int, "10000"),
    f("FLICKSYNC_WS_MAX_MESSAGE_BYTES", Sy, Int, "16384"),
    f("FLICKSYNC_WS_PING_INTERVAL", Sy, Int, "20"),
    f("FLICKSYNC_WS_IDLE_TIMEOUT", Sy, Int, "60"),
    f("FLICKSYNC_WS_SEND_TIMEOUT", Sy, Int, "10"),
    f("FLICKSYNC_MSG_RATE_PER_SEC", Sy, Int, "20"),
    f("FLICKSYNC_MSG_BURST", Sy, Int, "40"),
    f("FLICKSYNC_WS_RATE_LIMIT_STRIKES", Sy, Int, "20"),
    f("FLICKSYNC_WS_OUTBOUND_BUFFER", Sy, Int, "256"),
    // --- FlickDD
    f("FLICKDD_ENABLED", Dd, Bool, "false"),
    f("FLICKDD_JELLYFIN_URL", Dd, Text, ""),
    f("FLICKDD_JELLYFIN_API_KEY", Dd, Secret, ""),
    f("FLICKDD_PLEX_URL", Dd, Text, ""),
    f("FLICKDD_PLEX_TOKEN", Dd, Secret, ""),
    f("FLICKDD_MAX_PARALLEL", Dd, Int, "10"),
    f("FLICKDD_MAX_GLOBAL", Dd, Int, "100"),
    f("FLICKDD_RATE_MBPS", Dd, Int, "10"),
    f("FLICKDD_CHUNK_MB", Dd, Int, "8"),
    f("FLICKDD_MAX_RANGE_MB", Dd, Int, "64"),
    f("FLICKDD_MAX_REQUESTS_PER_MIN", Dd, Int, "120"),
    f("FLICKDD_MAX_OVERSERVE", Dd, Int, "2"),
    f("FLICKDD_GRANT_TTL", Dd, Int, "21600"),
    f("FLICKDD_GRANT_MAX_AGE", Dd, Int, "86400"),
    f("FLICKDD_STALL_TIMEOUT", Dd, Int, "30"),
    f("FLICKDD_UPSTREAM_TIMEOUT", Dd, Int, "15"),
    f("FLICKDD_UPSTREAM_RETRIES", Dd, Int, "2"),
];

/// Read by the config but fixed at process start (or bootstrap secrets): never editable.
pub const BOOT_ONLY: &[&str] = &[
    "FLICKSYNC_HOST",
    "FLICKSYNC_PORT",
    "FLICKSYNC_DATA_DIR",
    "FLICKSYNC_LOG_FORMAT",
    "FLICKSYNC_AUTH_KEYS_FILE",
    "FLICKSYNC_SHUTDOWN_GRACE",
    "FLICKSYNC_SWEEP_INTERVAL_MS",
    "FLICKSYNC_MAX_BODY_BYTES",
    "FLICKSYNC_ADMIN_TOKEN",
    "PANEL_PASSWORD",
];

pub fn field(name: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.name == name)
}
```

`src/settings/mod.rs`: add `pub mod fields;` and the implementation above the tests:

```rust
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
        if let Some(f) = field(name) {
            if let Some(v) = stored.scope(f.scope).get(name) {
                return Some(v.clone());
            }
        }
        (self.env)(name)
    }

    /// The effective value of `name`: stored, else environment.
    pub fn lookup(&self, name: &str) -> Option<String> {
        self.lookup_in(&self.read(), name)
    }

    /// The whole configuration as seen through the stored settings.
    pub fn config(&self) -> Result<Config, ConfigError> {
        Config::from_lookup(&|k| self.lookup(k))
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

    pub fn is_enabled(&self, scope: Scope) -> bool {
        Self::enabled_var(scope)
            .is_some_and(|name| parse_bool(&|k| self.lookup(k), name, false).unwrap_or(false))
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

    fn validate(&self, next: &Stored, scope: Scope) -> Result<(), SettingsError> {
        let look = |k: &str| self.lookup_in(next, k);
        let invalid = |e: &dyn std::fmt::Display| SettingsError::Invalid(e.to_string());
        let cfg = Config::from_lookup(&look).map_err(|e| invalid(&e))?;
        match scope {
            Scope::Server => {
                Authenticator::new(&cfg.auth)
                    .map_err(|e| SettingsError::Invalid(format!("FLICKSYNC_AUTH_KEYS: {e}")))?;
                EnvFilter::try_new(&cfg.log_level)
                    .map_err(|e| SettingsError::Invalid(format!("FLICKSYNC_LOG_LEVEL: {e}")))?;
            }
            Scope::FlickSync => {}
            Scope::FlickDd => {
                let enabled = parse_bool(&look, "FLICKDD_ENABLED", false).unwrap_or(false);
                DdConfig::check(&look, enabled).map_err(|e| invalid(&e))?;
            }
        }
        Ok(())
    }
}
```

`src/config/mod.rs`: change `fn parse_bool` visibility check: it is already `pub(crate)`. `Lookup` type is `pub(crate)`; `DdConfig::check` takes `Lookup`, so `&look` (a closure reference) coerces.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib settings config dd::config`
Expected: PASS. If `catalogue_defaults_match_the_code_defaults` names a field, the catalogue default for that field is wrong: fix the catalogue (the code default is the truth). If `every_variable_the_config_reads...` names a variable, add it to `FIELDS` or `BOOT_ONLY`.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -3 && cargo test 2>&1 | grep -E "^test result|FAILED"
git add -A src
git commit -m "feat(settings): field catalogue, layered lookup and validated writes

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `MODULE_DISABLED` error and the module slot

**Files:**
- Create: `src/modules.rs`
- Modify: `src/errors/mod.rs`, `src/lib.rs` (add `pub mod modules;`)

**Interfaces:**
- Consumes: `store::Scope`.
- Produces:
  - `ErrorCode::ModuleDisabled` (HTTP 503, wire name `MODULE_DISABLED`)
  - `modules::ModuleId { FlickSync, FlickDd }` with `ALL`, `id()`, `from_id()`, `scope()`
  - `modules::Slot<T>`: `Default`, `get() -> Option<Arc<T>>`, `gate() -> MutexGuard<'_, ()>`, `replace(SlotState<T>) -> SlotState<T>`, `status(ModuleId, enabled: bool, current_fingerprint: u64) -> ModuleStatus`
  - `modules::SlotState<T> { Stopped, Running(Running<T>), Failed(String) }`, `Running::new(rt: Arc<T>, fingerprint: u64)`
  - `modules::ModuleStatus { id, state, message, enabled, pending_reload, since }` (serde)

- [ ] **Step 1: Write the failing tests**

In `src/errors/mod.rs` add a test module (or extend one if present):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_disabled_is_a_503_with_a_stable_wire_name() {
        assert_eq!(ErrorCode::ModuleDisabled.http_status(), 503);
        assert_eq!(
            serde_json::to_string(&ErrorCode::ModuleDisabled).unwrap(),
            "\"MODULE_DISABLED\""
        );
    }
}
```

`src/modules.rs` test module (the code above it comes in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_ids_round_trip() {
        for id in ModuleId::ALL {
            assert_eq!(ModuleId::from_id(id.id()), Some(id));
        }
        assert_eq!(ModuleId::from_id("nope"), None);
        assert_eq!(ModuleId::FlickSync.scope(), Scope::FlickSync);
        assert_eq!(ModuleId::FlickDd.scope(), Scope::FlickDd);
    }

    #[test]
    fn a_slot_goes_stopped_running_failed() {
        let slot: Slot<u32> = Slot::default();
        assert!(slot.get().is_none());
        let s = slot.status(ModuleId::FlickSync, false, 1);
        assert_eq!((s.state, s.enabled, s.pending_reload, s.since), ("stopped", false, false, None));

        assert!(matches!(slot.replace(SlotState::Running(Running::new(Arc::new(7), 1))), SlotState::Stopped));
        assert_eq!(*slot.get().unwrap(), 7);
        let s = slot.status(ModuleId::FlickSync, true, 1);
        assert_eq!((s.state, s.pending_reload), ("running", false));
        assert!(s.since.is_some());

        // The stored settings moved on since the module started.
        assert!(slot.status(ModuleId::FlickSync, true, 2).pending_reload);

        let prev = slot.replace(SlotState::Failed("boom".into()));
        assert!(matches!(prev, SlotState::Running(_)));
        assert!(slot.get().is_none());
        let s = slot.status(ModuleId::FlickDd, true, 2);
        assert_eq!((s.state, s.message.as_deref(), s.pending_reload), ("failed", Some("boom"), false));
    }

    #[test]
    fn the_gate_serialises_critical_sections() {
        let slot: Arc<Slot<u32>> = Arc::new(Slot::default());
        let inside = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let overlap = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (slot, inside, overlap) = (slot.clone(), inside.clone(), overlap.clone());
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        let _g = slot.gate();
                        if inside.fetch_add(1, std::sync::atomic::Ordering::SeqCst) != 0 {
                            overlap.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                        std::thread::yield_now();
                        inside.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert!(!overlap.load(std::sync::atomic::Ordering::SeqCst));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib errors modules`
Expected: compile errors (`ModuleDisabled`, `modules` missing).

- [ ] **Step 3: Implement**

`src/errors/mod.rs`: add `ModuleDisabled,` to the enum (before `Internal`) and change the 503 arm to `TooManyConnections | ModuleDisabled => 503,`.

`src/lib.rs`: add `pub mod modules;` (after `metrics`).

`src/modules.rs`:

```rust
//! Runtime slot of a module: what is running, if anything.

use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::settings::store::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleId {
    FlickSync,
    FlickDd,
}

impl ModuleId {
    pub const ALL: [ModuleId; 2] = [ModuleId::FlickSync, ModuleId::FlickDd];

    pub fn id(self) -> &'static str {
        self.scope().id()
    }

    pub fn from_id(id: &str) -> Option<ModuleId> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }

    pub fn scope(self) -> Scope {
        match self {
            ModuleId::FlickSync => Scope::FlickSync,
            ModuleId::FlickDd => Scope::FlickDd,
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct Running<T> {
    pub rt: Arc<T>,
    /// Fingerprint of the module's settings when it started.
    pub fingerprint: u64,
    pub since_ms: u64,
}

impl<T> Running<T> {
    pub fn new(rt: Arc<T>, fingerprint: u64) -> Self {
        Self {
            rt,
            fingerprint,
            since_ms: now_ms(),
        }
    }
}

pub enum SlotState<T> {
    Stopped,
    Running(Running<T>),
    Failed(String),
}

/// One module's runtime. Handlers call [`Slot::get`] and keep the `Arc` for the whole request,
/// so a reload can never free memory under them.
pub struct Slot<T> {
    state: RwLock<SlotState<T>>,
    gate: Mutex<()>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            state: RwLock::new(SlotState::Stopped),
            gate: Mutex::new(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleStatus {
    pub id: &'static str,
    /// `stopped`, `running` or `failed`.
    pub state: &'static str,
    /// Why the module failed to start.
    pub message: Option<String>,
    /// The persisted on/off switch.
    pub enabled: bool,
    /// Running with older settings than the ones now in force.
    pub pending_reload: bool,
    /// Start time (ms since the epoch) while running.
    pub since: Option<u64>,
}

impl<T> Slot<T> {
    pub fn get(&self) -> Option<Arc<T>> {
        match &*self.state.read().unwrap_or_else(|e| e.into_inner()) {
            SlotState::Running(r) => Some(r.rt.clone()),
            _ => None,
        }
    }

    /// Held for the whole of a start, stop or reload.
    pub fn gate(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn replace(&self, new: SlotState<T>) -> SlotState<T> {
        std::mem::replace(
            &mut *self.state.write().unwrap_or_else(|e| e.into_inner()),
            new,
        )
    }

    pub fn status(&self, id: ModuleId, enabled: bool, current_fingerprint: u64) -> ModuleStatus {
        let base = ModuleStatus {
            id: id.id(),
            state: "stopped",
            message: None,
            enabled,
            pending_reload: false,
            since: None,
        };
        match &*self.state.read().unwrap_or_else(|e| e.into_inner()) {
            SlotState::Stopped => base,
            SlotState::Running(r) => ModuleStatus {
                state: "running",
                pending_reload: r.fingerprint != current_fingerprint,
                since: Some(r.since_ms),
                ..base
            },
            SlotState::Failed(m) => ModuleStatus {
                state: "failed",
                message: Some(m.clone()),
                ..base
            },
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib errors modules`
Expected: PASS.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -3 && cargo test 2>&1 | grep -E "^test result|FAILED"
git add src
git commit -m "feat(modules): module slot, status and MODULE_DISABLED error

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `AppState` runs on slots (behaviour unchanged)

This is the large mechanical refactor. Its safety net is the existing suite: after it, every existing test passes with FlickSync started through `FLICKSYNC_ENABLED=true`.

**Files:**
- Modify: `src/app.rs`, `src/main.rs`, `src/api/{mod,admin,auth,downloads,health,rooms}.rs`, `src/websocket/{mod,connection}.rs`, `tests/common/mod.rs`, `tests/admin.rs`, `tests/downloads.rs`, `tests/integration.rs`

**Interfaces:**
- Consumes: Tasks 1 to 3.
- Produces:
  - `app::SyncRuntime { manager: Arc<RoomManager>, ws: WsConfig, conn_limit: Arc<Semaphore> }`
  - `app::ServerRuntime { cfg: Config, auth: Authenticator }`
  - `app::StartError`
  - `AppState::new(settings: Arc<Settings>) -> Result<AppState, StartError>`, `AppState::with_clock(settings, clock)`
  - fields `boot: Arc<Config>`, `settings: Arc<Settings>`, `metrics`, `started_at`
  - `AppState::server() -> Arc<ServerRuntime>`, `sync_opt() -> Option<Arc<SyncRuntime>>`, `sync() -> crate::errors::Result<Arc<SyncRuntime>>` (error `ModuleDisabled`), `dd() -> Option<Arc<DdState>>`
  - `app::serve(state: AppState)`

- [ ] **Step 1: Switch the test harness first (the suite goes red)**

`tests/common/mod.rs`: replace the `Config`/`AppState` construction in `TestServer::start`:

```rust
        set("FLICKSYNC_ENABLED", "true");
        for (k, v) in extra {
            set(k, v);
        }
        let env: Env = Arc::new(move |k| vars.get(k).cloned());
        let dir = flicksync::settings::scratch_dir("server");
        let settings = Settings::open(&dir, env).unwrap();
        let state = AppState::new(settings).unwrap();
```

(the `set("FLICKSYNC_ENABLED", "true");` line goes before the `for (k, v) in extra` loop so a test can override it), and fix the imports:

```rust
use std::sync::Arc;

use flicksync::app::{AppState, build_router, spawn_sweeper};
use flicksync::auth::{Claims, mint_token};
use flicksync::settings::{Env, Settings};
```

(remove `use flicksync::config::Config;`).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --no-run 2>&1 | head -20`
Expected: compile errors in `src/` consumers and tests (`AppState::new` takes `Arc<Settings>`).

- [ ] **Step 3: Rewrite `src/app.rs` core (replace everything above `shutdown_signal`)**

```rust
//! Application wiring: shared state, module slots, background tasks and the server loop.

use std::net::SocketAddr;
use std::sync::{Arc, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::auth::{AuthConfigError, Authenticator};
use crate::config::{Config, ConfigError, WsConfig};
use crate::dd::DdState;
use crate::errors::{Error, ErrorCode};
use crate::invite::{self, InviteError};
use crate::metrics::Metrics;
use crate::modules::{ModuleId, Running, Slot, SlotState};
use crate::room::RoomManager;
use crate::settings::Settings;
use crate::sync::{Clock, SystemClock};

/// What FlickSync needs while it runs. Dropped (after `RoomManager::shutdown`) when it stops.
pub struct SyncRuntime {
    pub manager: Arc<RoomManager>,
    pub ws: WsConfig,
    pub conn_limit: Arc<Semaphore>,
}

/// Server-wide settings in force (keys, public address, CORS, metrics). Replaced as a whole.
pub struct ServerRuntime {
    pub cfg: Config,
    pub auth: Authenticator,
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("{0}")]
    Config(#[from] ConfigError),
    #[error("cannot load or create the signing key: {0}")]
    Keys(#[from] InviteError),
    #[error("{0}")]
    Auth(#[from] AuthConfigError),
}

fn build_server(settings: &Settings) -> Result<ServerRuntime, StartError> {
    let mut cfg = settings.config()?;
    invite::ensure_keys(&mut cfg)?;
    let auth = Authenticator::new(&cfg.auth)?;
    Ok(ServerRuntime { cfg, auth })
}

#[derive(Clone)]
pub struct AppState {
    /// Read once at boot: bind address, intervals, body limit. Never swapped.
    pub boot: Arc<Config>,
    pub settings: Arc<Settings>,
    server: Arc<RwLock<Arc<ServerRuntime>>>,
    sync_slot: Arc<Slot<SyncRuntime>>,
    dd_slot: Arc<Slot<DdState>>,
    pub metrics: Arc<Metrics>,
    pub started_at: Instant,
    clock: Arc<dyn Clock>,
}

impl AppState {
    pub fn new(settings: Arc<Settings>) -> Result<Self, StartError> {
        Self::with_clock(settings, Arc::new(SystemClock::new()))
    }

    pub fn with_clock(settings: Arc<Settings>, clock: Arc<dyn Clock>) -> Result<Self, StartError> {
        let boot = settings.config()?;
        let server = build_server(&settings)?;
        let state = Self {
            boot: Arc::new(boot),
            settings,
            server: Arc::new(RwLock::new(Arc::new(server))),
            sync_slot: Arc::new(Slot::default()),
            dd_slot: Arc::new(Slot::default()),
            metrics: Arc::new(Metrics::default()),
            started_at: Instant::now(),
            clock,
        };
        for id in ModuleId::ALL {
            if state.settings.is_enabled(id.scope()) {
                let _gate = state.gate(id);
                state.start_locked(id);
            }
        }
        Ok(state)
    }

    pub fn server(&self) -> Arc<ServerRuntime> {
        self.server
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn sync_opt(&self) -> Option<Arc<SyncRuntime>> {
        self.sync_slot.get()
    }

    /// The running FlickSync, or `MODULE_DISABLED` (503).
    pub fn sync(&self) -> crate::errors::Result<Arc<SyncRuntime>> {
        self.sync_opt().ok_or_else(|| {
            Error::new(
                ErrorCode::ModuleDisabled,
                "FlickSync is not running on this server",
            )
        })
    }

    pub fn dd(&self) -> Option<Arc<DdState>> {
        self.dd_slot.get()
    }

    fn gate(&self, id: ModuleId) -> MutexGuard<'_, ()> {
        match id {
            ModuleId::FlickSync => self.sync_slot.gate(),
            ModuleId::FlickDd => self.dd_slot.gate(),
        }
    }

    fn build_sync(&self) -> Result<SyncRuntime, String> {
        let cfg = self.settings.config().map_err(|e| e.to_string())?;
        let manager = Arc::new(RoomManager::new(
            cfg.manager.clone(),
            self.clock.clone(),
            self.metrics.clone(),
        ));
        Ok(SyncRuntime {
            manager,
            conn_limit: Arc::new(Semaphore::new(cfg.ws.max_connections)),
            ws: cfg.ws,
        })
    }

    fn build_dd(&self) -> Result<Arc<DdState>, String> {
        let cfg = self.settings.config().map_err(|e| e.to_string())?;
        if !cfg.dd.enabled {
            return Err("FlickDD is not enabled".to_owned());
        }
        Ok(DdState::new(cfg.dd))
    }

    /// Start `id` with the settings in force. The caller holds the module's gate. A module that
    /// is already running is left alone; a failure leaves it `Failed`, never panics.
    fn start_locked(&self, id: ModuleId) {
        let fingerprint = self.settings.fingerprint(id.scope());
        match id {
            ModuleId::FlickSync => {
                if self.sync_slot.get().is_some() {
                    return;
                }
                match self.build_sync() {
                    Ok(rt) => {
                        self.sync_slot
                            .replace(SlotState::Running(Running::new(Arc::new(rt), fingerprint)));
                        info!("FlickSync started");
                    }
                    Err(message) => {
                        warn!(error = %message, "FlickSync could not start");
                        self.sync_slot.replace(SlotState::Failed(message));
                    }
                }
            }
            ModuleId::FlickDd => {
                if self.dd_slot.get().is_some() {
                    return;
                }
                match self.build_dd() {
                    Ok(rt) => {
                        self.dd_slot
                            .replace(SlotState::Running(Running::new(rt, fingerprint)));
                        info!("FlickDD started");
                    }
                    Err(message) => {
                        warn!(error = %message, "FlickDD could not start");
                        self.dd_slot.replace(SlotState::Failed(message));
                    }
                }
            }
        }
    }
}

pub fn build_router(state: AppState) -> Router {
    crate::api::router(state)
}

/// Periodic maintenance of whatever is running (reconnection grace, expiry, sync heartbeat).
pub fn spawn_sweeper(state: &AppState) -> JoinHandle<()> {
    let state = state.clone();
    let period = Duration::from_millis(state.boot.sweep_interval_ms.max(10));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Some(sync) = state.sync_opt() {
                sync.manager.sweep();
            }
            if let Some(dd) = state.dd() {
                dd.sweep();
            }
        }
    })
}
```

Keep `shutdown_signal` as is. Replace `serve`:

```rust
/// Bind and serve until SIGINT/SIGTERM.
pub async fn serve(state: AppState) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = format!("{}:{}", state.boot.host, state.boot.port).parse()?;
    let grace = Duration::from_secs(state.boot.shutdown_grace_secs);
    let sweeper = spawn_sweeper(&state);
    let app = build_router(state.clone());
    let listener = TcpListener::bind(addr).await?;
    info!(
        %addr,
        version = env!("CARGO_PKG_VERSION"),
        auth_keys = state.server().cfg.auth.keys.len(),
        "flicksync listening"
    );

    let stopping = state.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        shutdown_signal().await;
        info!("shutdown signal received");
        // Close rooms and sockets first, otherwise open WebSockets would hold shutdown up;
        // same for download streams, a throttled file response can last hours.
        if let Some(sync) = stopping.sync_opt() {
            sync.manager.shutdown();
        }
        if let Some(dd) = stopping.dd() {
            let cut = dd.shutdown();
            info!(streams = cut, "FlickDD download streams stopped");
        }
    });

    tokio::select! {
        res = server => {
            if let Err(e) = res {
                error!(error = %e, "server error");
                return Err(e.into());
            }
        }
        _ = async {
            shutdown_signal().await;
            tokio::time::sleep(grace).await;
        } => {
            info!("grace period elapsed, exiting");
        }
    }
    sweeper.abort();
    Ok(())
}
```

Also add to `src/settings/mod.rs` nothing new. In `Settings`, `is_enabled` exists from Task 2.

- [ ] **Step 4: Move every consumer onto the accessors**

Do these edits, then follow the compiler for the leftovers:

`src/api/auth.rs`: `Some(t) => state.auth.verify(t),` becomes `Some(t) => state.server().auth.verify(t),`.

`src/api/health.rs`:

```rust
pub async fn ready(State(state): State<AppState>) -> Response {
    let ready = state.server().auth.has_keys();
    // ... rest unchanged
}

pub async fn metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let server = state.server();
    if !server.cfg.http.metrics_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(expected) = &server.cfg.http.metrics_token {
        // ... unchanged
    }
    let mut body = state.metrics.render_prometheus();
    if let Some(dd) = state.dd() {
        body.push_str(&dd.stats.render_prometheus(dd.grants.count() as u64));
    }
    // ... unchanged
}
```

`src/api/rooms.rs`: in each of the four handlers add `let sync = state.sync()?;` as the first statement after the body parsing and replace `state.manager.` with `sync.manager.` (`create_room`, `get_room`, `join_room`, `leave_room`).

`src/websocket/mod.rs`:

```rust
    if !origin_allowed(&headers, &state.server().cfg.http.allowed_origins) {
        warn!("websocket upgrade rejected: origin not allowed");
        return Err(Error::new(ErrorCode::Forbidden, "origin not allowed").into());
    }
    let identity = authenticate(&state, &headers, query.access_token.as_deref())?;
    let sync = state.sync()?;
    // Fail with a regular HTTP error (404/403/409...) before upgrading when possible.
    sync.manager.can_attach(&room_id, &identity)?;
    let permit = sync
        .conn_limit
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::new(ErrorCode::TooManyConnections, "too many connections"))?;

    let max = sync.ws.max_message_bytes;
    Ok(ws
        .max_message_size(max)
        .max_frame_size(max)
        .on_upgrade(move |socket| connection::run(socket, state, sync, room_id, identity, permit)))
```

`src/websocket/connection.rs`: add the parameter `rt: Arc<crate::app::SyncRuntime>` after `state` in `run`, then replace `&state.cfg.ws` by `&rt.ws` and each `state.manager.` by `rt.manager.` (lines around 73, 78, 115, 186). Remove `state` uses that become unused (keep it if `state.metrics` is used).

`src/api/mod.rs`: `let cors = cors_layer(&state.server().cfg.http.allowed_origins);` and `let body_limit = state.boot.http.max_body_bytes;`.

`src/api/downloads.rs`: change `enabled`:

```rust
fn enabled(state: &AppState) -> Result<Arc<DdState>, DdError> {
    state
        .dd()
        .ok_or_else(|| Error::new(ErrorCode::DownloadNotFound, "FlickDD is disabled").into())
}
```

Callers hold an `Arc<DdState>` now: where they passed `dd` (a `&Arc<DdState>`) pass `&dd`; let the compiler point at each.

`src/api/admin.rs`:

- `AdminAuth`: `let Some(expected) = &state.boot.http.admin_token else {` (Task 7 replaces this).
- `overview`:

```rust
pub async fn overview(_: AdminAuth, State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let sync = state.sync_opt();
    no_store(
        StatusCode::OK,
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "now": now_ms(),
            "uptime_secs": state.started_at.elapsed().as_secs(),
            "running": sync.is_some(),
            "accepting": sync.as_ref().is_some_and(|s| s.manager.is_accepting()),
            "ready": state.server().auth.has_keys(),
            "rooms": sync.as_ref().map_or(0, |s| s.manager.room_count()),
            "max_rooms": sync.as_ref().map_or(0, |s| s.manager.config().max_rooms),
            "participants": m.participants_active.get(),
            "connections": m.ws_connections.get(),
            "max_connections": sync.as_ref().map_or(0, |s| s.ws.max_connections),
            "rtt_avg_ms": m.rtt_avg_us.get() as f64 / 1000.0,
        }),
    )
}
```

- `invite`: `let server = state.server();` then `invite::invitation(&server.cfg)`, `invite::endpoint(&server.cfg)`, `server.cfg.keys_configured`, `server.cfg.auth.keys.len()`.
- `list_rooms` returns `Result<Response, ApiError>`: `let sync = state.sync()?;`, `sync.manager.admin_rooms()`, wrap the final `no_store(...)` in `Ok(...)`.
- `close_room`: `state.sync()?.manager.admin_close_room(&room_id)?;`.
- `stats` returns `Result<Response, ApiError>`: `let sync = state.sync()?; let d = &sync.manager.config().room.drift;` and `Ok(no_store(...))`.
- `dd_state`:

```rust
fn dd_state(state: &AppState) -> Result<Arc<DdState>, ApiError> {
    state
        .dd()
        .ok_or_else(|| Error::new(ErrorCode::DownloadNotFound, "FlickDD is disabled").into())
}
```

and add `use std::sync::Arc;`.

`src/main.rs`: replace the config loading and the final `serve` call:

```rust
use std::sync::Arc;

use flicksync::app::{self, AppState};
use flicksync::settings::{Env, Settings};
```

```rust
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
    // ... healthcheck / invite / init_tracing / ensure_keys / banner / keys check unchanged ...
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
```

(remove the old `use flicksync::app;` and `Config::from_env()` block; `Config::from_env` can stay in the library, unused by `main`.)

Tests: replace `s.state.dd.as_ref().unwrap()`-style uses with `s.state.dd().unwrap()` and `state.manager` with `state.sync().unwrap().manager` in `tests/admin.rs`, `tests/downloads.rs`, `tests/integration.rs` (find them with `grep -n "state\.\(dd\|manager\)" tests/*.rs`).

- [ ] **Step 5: Run the whole suite**

Run: `cargo fmt && cargo clippy --all-targets 2>&1 | tail -5 && cargo test 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: PASS, same test counts as before this task. If a CORS preflight test fails because the layer is absent/present differently, leave it: Task 6 changes the CORS layer deliberately and re-checks.

- [ ] **Step 6: Commit**

```bash
git add -A src tests
git commit -m "refactor: AppState runs on module slots and a server runtime

Behaviour is unchanged: FlickSync and FlickDD start at boot when enabled.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Start, stop, reload

**Files:**
- Modify: `src/app.rs`
- Create: `tests/lifecycle.rs`

**Interfaces:**
- Consumes: `AppState` internals from Task 4 (`gate`, `start_locked`).
- Produces:
  - `AppState::module_status(&self, ModuleId) -> ModuleStatus`
  - `AppState::start_module(&self, ModuleId) -> Result<ModuleStatus, SettingsError>` (persists `enabled=true`)
  - `AppState::stop_module(&self, ModuleId) -> Result<ModuleStatus, SettingsError>` (persists `enabled=false`)
  - `AppState::reload_module(&self, ModuleId) -> ModuleStatus` (starts only an enabled module)
  - `AppState::stop_all(&self)`

- [ ] **Step 1: Write the failing tests**

`tests/lifecycle.rs`:

```rust
//! Module lifecycle: start, stop and reload at runtime.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use flicksync::modules::ModuleId;
use flicksync::settings::store::Scope;
use std::collections::BTreeMap;

fn patch(pairs: &[(&str, &str)]) -> BTreeMap<String, Option<String>> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), Some(v.to_string())))
        .collect()
}

#[tokio::test]
async fn a_stopped_module_answers_503_and_the_server_stays_up() {
    let s = TestServer::start(&[("FLICKSYNC_ENABLED", "false")]).await;
    let (st, body) = s.http(Method::POST, "/api/v1/rooms", Some(&token("alice")), None).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "MODULE_DISABLED");
    assert_eq!(s.http(Method::GET, "/health", None, None).await.0, StatusCode::OK);
    assert_eq!(s.http(Method::GET, "/ready", None, None).await.0, StatusCode::OK);
    // FlickDD keeps its documented 404.
    let (st, _) = s.http(Method::POST, "/api/v1/downloads", Some(&token("alice")), Some(serde_json::json!({}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let m = s.state.module_status(ModuleId::FlickSync);
    assert_eq!((m.state, m.enabled), ("stopped", false));
}

#[tokio::test]
async fn start_serves_and_stop_closes_open_rooms() {
    let s = TestServer::start(&[("FLICKSYNC_ENABLED", "false")]).await;
    let m = s.state.start_module(ModuleId::FlickSync).unwrap();
    assert_eq!((m.state, m.enabled), ("running", true));
    let room = s.create_room("alice").await;
    let mut ws = s.connect(&room, "alice").await;
    ws.expect("room_state").await;

    let m = s.state.stop_module(ModuleId::FlickSync).unwrap();
    assert_eq!((m.state, m.enabled), ("stopped", false));
    ws.expect_closed().await;
    let (st, body) = s.http(Method::POST, "/api/v1/rooms", Some(&token("alice")), None).await;
    assert_eq!((st, body["error"]["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("MODULE_DISABLED")));
}

#[tokio::test]
async fn start_and_stop_are_idempotent() {
    let s = TestServer::start(&[("FLICKSYNC_ENABLED", "false")]).await;
    s.state.start_module(ModuleId::FlickSync).unwrap();
    let first = s.state.sync().unwrap();
    s.state.start_module(ModuleId::FlickSync).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &s.state.sync().unwrap()), "no second runtime");
    s.state.stop_module(ModuleId::FlickSync).unwrap();
    s.state.stop_module(ModuleId::FlickSync).unwrap();
    assert!(s.state.sync().is_err());
}

#[tokio::test]
async fn reload_picks_up_new_settings_and_reports_pending_changes() {
    let s = TestServer::start(&[]).await;
    assert!(!s.state.module_status(ModuleId::FlickSync).pending_reload);
    s.state
        .settings
        .put(Scope::FlickSync, patch(&[("FLICKSYNC_MAX_ROOM_SIZE", "1")]))
        .unwrap();
    assert!(s.state.module_status(ModuleId::FlickSync).pending_reload);

    // Still the old limit until reloaded.
    let old_room = s.create_room("alice").await;
    assert_eq!(s.join_http(&old_room, "bob").await.0, StatusCode::OK);

    let m = s.state.reload_module(ModuleId::FlickSync);
    assert_eq!((m.state, m.pending_reload), ("running", false));
    let room = s.create_room("alice").await;
    assert_eq!(s.join_http(&room, "bob").await.0, StatusCode::CONFLICT, "room size 1 after reload");
}

#[tokio::test]
async fn reload_never_starts_a_disabled_module() {
    let s = TestServer::start(&[("FLICKSYNC_ENABLED", "false")]).await;
    let m = s.state.reload_module(ModuleId::FlickSync);
    assert_eq!(m.state, "stopped");
}

#[tokio::test]
async fn a_failing_start_leaves_the_module_failed_and_the_rest_running() {
    let s = TestServer::start(&[]).await;
    // FlickDD enabled but no backend configured anywhere.
    let m = s.state.start_module(ModuleId::FlickDd).unwrap();
    assert_eq!(m.state, "failed");
    assert!(m.message.unwrap().contains("backend"), "says why");
    assert!(m.enabled, "the intent is kept");
    assert!(s.state.sync().is_ok(), "FlickSync is untouched");
    assert_eq!(s.http(Method::GET, "/health", None, None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_request_in_flight_survives_a_stop() {
    let s = TestServer::start(&[]).await;
    let held = s.state.sync().unwrap(); // what a handler keeps for the whole request
    s.state.stop_module(ModuleId::FlickSync).unwrap();
    assert_eq!(held.manager.room_count(), 0, "the old runtime is still usable and closed");
    assert!(!held.manager.is_accepting());
    assert!(s.state.sync().is_err(), "new requests are refused");
}

#[test]
fn concurrent_lifecycle_calls_are_serialised() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(TestServer::start(&[]));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let state = s.state.clone();
            std::thread::spawn(move || {
                for _ in 0..20 {
                    match i % 3 {
                        0 => { state.reload_module(ModuleId::FlickSync); }
                        1 => { state.stop_module(ModuleId::FlickSync).unwrap(); }
                        _ => { state.start_module(ModuleId::FlickSync).unwrap(); }
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    // Whatever the last call was, the state is coherent and one more start settles it.
    let m = s.state.start_module(ModuleId::FlickSync).unwrap();
    assert_eq!((m.state, m.enabled), ("running", true));
    let a = s.state.sync().unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &s.state.sync().unwrap()));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test lifecycle`
Expected: compile errors (`module_status`, `start_module`, ... missing).

- [ ] **Step 3: Implement (in `impl AppState` of `src/app.rs`)**

```rust
    pub fn module_status(&self, id: ModuleId) -> crate::modules::ModuleStatus {
        let enabled = self.settings.is_enabled(id.scope());
        let fingerprint = self.settings.fingerprint(id.scope());
        match id {
            ModuleId::FlickSync => self.sync_slot.status(id, enabled, fingerprint),
            ModuleId::FlickDd => self.dd_slot.status(id, enabled, fingerprint),
        }
    }

    /// Stop `id` and free its runtime. The caller holds the module's gate.
    fn stop_locked(&self, id: ModuleId) {
        match id {
            ModuleId::FlickSync => {
                if let SlotState::Running(r) = self.sync_slot.replace(SlotState::Stopped) {
                    r.rt.manager.shutdown();
                    info!("FlickSync stopped");
                }
            }
            ModuleId::FlickDd => {
                if let SlotState::Running(r) = self.dd_slot.replace(SlotState::Stopped) {
                    let cut = r.rt.shutdown();
                    info!(streams = cut, "FlickDD stopped");
                }
            }
        }
    }

    /// Persist "enabled" and start the module.
    pub fn start_module(
        &self,
        id: ModuleId,
    ) -> Result<crate::modules::ModuleStatus, crate::settings::SettingsError> {
        self.settings.set_enabled(id.scope(), true)?;
        let _gate = self.gate(id);
        self.start_locked(id);
        Ok(self.module_status(id))
    }

    /// Persist "disabled" and stop the module.
    pub fn stop_module(
        &self,
        id: ModuleId,
    ) -> Result<crate::modules::ModuleStatus, crate::settings::SettingsError> {
        self.settings.set_enabled(id.scope(), false)?;
        let _gate = self.gate(id);
        self.stop_locked(id);
        Ok(self.module_status(id))
    }

    /// Stop then start with the settings now stored. A disabled module stays stopped.
    pub fn reload_module(&self, id: ModuleId) -> crate::modules::ModuleStatus {
        let _gate = self.gate(id);
        self.stop_locked(id);
        if self.settings.is_enabled(id.scope()) {
            self.start_locked(id);
        }
        self.module_status(id)
    }

    /// Process shutdown: close everything, keep the persisted switches as they are.
    pub fn stop_all(&self) {
        for id in ModuleId::ALL {
            let _gate = self.gate(id);
            self.stop_locked(id);
        }
    }
```

In `serve`, replace the body of the graceful-shutdown closure after `info!("shutdown signal received");` by:

```rust
        stopping.stop_all();
```

(and delete the two `if let` blocks it replaces).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test lifecycle && cargo test 2>&1 | grep -E "^test result|FAILED"`
Expected: PASS. In `a_request_in_flight_survives_a_stop`, `room_count()` is `0` because `shutdown` drains the map.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -3
git add -A src tests
git commit -m "feat(modules): start, stop and reload at runtime

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Server scope reload (keys, CORS, metrics, log level)

**Files:**
- Modify: `src/app.rs`, `src/api/mod.rs`, `src/main.rs`
- Test: `tests/lifecycle.rs`

**Interfaces:**
- Consumes: `build_server`, `AppState::server()`.
- Produces:
  - `AppState::reload_server(&self) -> Result<(), StartError>` (the old runtime stays in force on error)
  - `app::LogControl = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>` and `AppState::set_log_control(&self, LogControl)`
  - CORS reads the allowed origins of the server runtime on every request

- [ ] **Step 1: Write the failing tests (append to `tests/lifecycle.rs`)**

```rust
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

async fn preflight(s: &TestServer, origin: &str) -> Option<String> {
    let resp = flicksync::app::build_router(s.state.clone())
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/api/v1/rooms")
                .header("origin", origin)
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.headers()
        .get("access-control-allow-origin")
        .map(|v| v.to_str().unwrap().to_owned())
}

#[tokio::test]
async fn a_server_reload_changes_cors_origins_for_new_requests() {
    let s = TestServer::start(&[]).await;
    assert_eq!(preflight(&s, "https://app.example").await.as_deref(), Some("https://app.example"));
    s.state
        .settings
        .put(Scope::Server, patch(&[("FLICKSYNC_CORS_ORIGINS", "https://new.example")]))
        .unwrap();
    assert_eq!(preflight(&s, "https://app.example").await.as_deref(), Some("https://app.example"), "not before the reload");
    s.state.reload_server().unwrap();
    assert_eq!(preflight(&s, "https://app.example").await, None);
    assert_eq!(preflight(&s, "https://new.example").await.as_deref(), Some("https://new.example"));
}

#[tokio::test]
async fn a_server_reload_swaps_the_signing_keys() {
    let s = TestServer::start(&[]).await;
    let k1 = token_for("alice", SERVER, "k1", SECRET, &["*"]);
    let k2 = token_for("alice", OTHER_SERVER, "k2", OTHER_SECRET, &["*"]);
    assert_eq!(s.http(Method::POST, "/api/v1/rooms", Some(&k1), None).await.0, StatusCode::CREATED);

    // Keep only k2.
    s.state
        .settings
        .put(Scope::Server, patch(&[("FLICKSYNC_AUTH_KEYS", &format!("k2:{OTHER_SERVER}:{OTHER_SECRET}"))]))
        .unwrap();
    s.state.reload_server().unwrap();
    assert_eq!(s.http(Method::POST, "/api/v1/rooms", Some(&k1), None).await.0, StatusCode::UNAUTHORIZED, "removed key");
    assert_eq!(s.http(Method::POST, "/api/v1/rooms", Some(&k2), None).await.0, StatusCode::CREATED, "kept key");
}

#[tokio::test]
async fn a_malformed_key_list_is_refused_and_the_old_keys_stay() {
    let s = TestServer::start(&[]).await;
    assert!(
        s.state
            .settings
            .put(Scope::Server, patch(&[("FLICKSYNC_AUTH_KEYS", "not-a-key")]))
            .is_err()
    );
    s.state.reload_server().unwrap();
    assert_eq!(s.http(Method::POST, "/api/v1/rooms", Some(&token("alice")), None).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn a_server_reload_applies_the_log_level() {
    let s = TestServer::start(&[]).await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = seen.clone();
    s.state.set_log_control(std::sync::Arc::new(move |level: &str| {
        sink.lock().unwrap().push(level.to_owned());
        Ok(())
    }));
    s.state
        .settings
        .put(Scope::Server, patch(&[("FLICKSYNC_LOG_LEVEL", "debug")]))
        .unwrap();
    s.state.reload_server().unwrap();
    assert_eq!(seen.lock().unwrap().as_slice(), ["debug"]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test lifecycle server`
Expected: compile error (`reload_server`, `set_log_control` missing).

- [ ] **Step 3: Implement**

`src/app.rs`: add the field and methods.

```rust
/// Applies a new tracing filter to the running subscriber.
pub type LogControl = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
```

Add to `AppState`: `log_control: Arc<RwLock<Option<LogControl>>>,` (initialise with `Arc::new(RwLock::new(None))` in `with_clock`), and:

```rust
    pub fn set_log_control(&self, control: LogControl) {
        *self.log_control.write().unwrap_or_else(|e| e.into_inner()) = Some(control);
    }

    /// Rebuild the server runtime from the stored settings and swap it in. On error the
    /// runtime in force is kept. Running modules are not restarted.
    pub fn reload_server(&self) -> Result<(), StartError> {
        let next = build_server(&self.settings)?;
        let level = next.cfg.log_level.clone();
        *self.server.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(next);
        if let Some(control) = self
            .log_control
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            if let Err(e) = control(&level) {
                warn!(error = %e, "could not apply the log level");
            }
        }
        info!("server settings reloaded");
        Ok(())
    }
```

`src/api/mod.rs`: build the CORS layer from the state and drop the conditional:

```rust
pub fn router(state: AppState) -> Router {
    let cors = cors_layer(&state);
    let body_limit = state.boot.http.max_body_bytes;
    // ... admin router unchanged ...
    Router::new()
        // ... routes unchanged ...
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state)
        .layer(cors)
}

/// CORS follows the origins of the server runtime in force, read on every request; there is
/// no wildcard. Native clients do not send `Origin` and are unaffected either way.
fn cors_layer(state: &AppState) -> CorsLayer {
    let state = state.clone();
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
            state
                .server()
                .cfg
                .http
                .allowed_origins
                .iter()
                .any(|o| o.as_bytes().eq_ignore_ascii_case(origin.as_bytes()))
        }))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::RANGE,
            header::IF_RANGE,
        ])
        .expose_headers([
            header::CONTENT_RANGE,
            header::ETAG,
            header::ACCEPT_RANGES,
            header::CONTENT_LENGTH,
            header::RETRY_AFTER,
        ])
        .max_age(Duration::from_secs(600))
}
```

(keep the `router` body's route list exactly as it is; only the construction of `cors`, the `.with_state(state)` / `.layer(cors)` tail and the helper change; delete the old `match cors { ... }`.)

`src/main.rs`: make the filter reloadable and hand the control to the state:

```rust
use flicksync::app::{self, AppState, LogControl};

fn init_tracing(cfg: &Config) -> LogControl {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, reload};

    let filter = EnvFilter::try_new(&cfg.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    let (filter, handle) = reload::Layer::new(filter);
    let registry = tracing_subscriber::registry().with(filter);
    match cfg.log_format {
        LogFormat::Json => registry.with(fmt::layer().json()).init(),
        LogFormat::Pretty => registry.with(fmt::layer()).init(),
    }
    Arc::new(move |level: &str| {
        let next = EnvFilter::try_new(level).map_err(|e| e.to_string())?;
        handle.reload(next).map_err(|e| e.to_string())
    })
}
```

In `main`, `let log_control = init_tracing(&cfg);` and after building `state`: `state.set_log_control(log_control);`.

- [ ] **Step 4: Run to verify success**

Run: `cargo test --test lifecycle && cargo test 2>&1 | grep -E "^test result|FAILED"`
Expected: PASS. If an existing CORS test expected a `405` for `OPTIONS` without any allowed origin, update it to the new behaviour (no `access-control-allow-origin` header); a browser sees the same refusal.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -3
git add -A src tests
git commit -m "feat(server): reload keys, CORS, metrics and log level at runtime

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Admin token derived from `PANEL_PASSWORD`

**Files:**
- Create: `src/admin_token.rs`
- Modify: `src/lib.rs` (`pub mod admin_token;`), `src/config/mod.rs`, `src/app.rs`, `src/api/admin.rs`
- Test: `tests/admin.rs`

**Interfaces:**
- Produces:
  - `admin_token::hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32]`
  - `admin_token::derive(password: &str) -> Option<String>` (`None` below 10 characters)
  - `admin_token::AdminTokens::from_config(&Config) -> AdminTokens`, `AdminTokens::accepts(&self, presented: &str) -> bool`, `AdminTokens::is_enabled(&self) -> bool`, `has_legacy()`
  - `HttpConfig.panel_password: Option<String>`
  - `AppState.admin: Arc<AdminTokens>`

- [ ] **Step 1: Write the failing tests**

`src/admin_token.rs` (tests only for now; implementation in Step 3):

```rust
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
    fn tokens_accept_the_derived_and_the_legacy_one_only() {
        let t = AdminTokens {
            derived: derive("correct horse battery staple"),
            legacy: Some("legacy-token-0123456789".to_owned()),
        };
        assert!(t.accepts("602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011"));
        assert!(t.accepts("legacy-token-0123456789"));
        assert!(!t.accepts("602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591012"));
        assert!(!t.accepts(""));
        let none = AdminTokens { derived: None, legacy: None };
        assert!(!none.is_enabled());
        assert!(!none.accepts("anything"));
    }
}
```

In `tests/admin.rs` add (using the helpers that file already has; adapt the request helper name to the local one, shown here as `admin_get(&s, path, token)` returning the status):

```rust
#[tokio::test]
async fn the_admin_api_accepts_the_token_derived_from_the_panel_password() {
    let password = "correct horse battery staple";
    let derived = "602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011";
    let s = TestServer::start(&[("PANEL_PASSWORD", password)]).await;
    let (st, _) = s.http(Method::GET, "/admin/v1/overview", Some(derived), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = s.http(Method::GET, "/admin/v1/overview", Some("wrong-token-0123456789"), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // The password itself is not the token.
    let (st, _) = s.http(Method::GET, "/admin/v1/overview", Some(password), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn without_a_long_enough_password_or_legacy_token_the_admin_api_is_off() {
    for vars in [vec![], vec![("PANEL_PASSWORD", "short")]] {
        let s = TestServer::start(&vars).await;
        let (st, _) = s.http(Method::GET, "/admin/v1/overview", Some("x".repeat(40).as_str()), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib admin_token && cargo test --test admin derived`
Expected: compile errors / failures.

- [ ] **Step 3: Implement**

`src/lib.rs`: `pub mod admin_token;` (first in the list).

`src/config/mod.rs`: add `pub panel_password: Option<String>,` to `HttpConfig`, and in `from_lookup`'s `HttpConfig { ... }` literal:

```rust
            panel_password: env("PANEL_PASSWORD")
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty()),
```

(no length check here: a short password only keeps the admin API off).

`src/admin_token.rs` (above the tests):

```rust
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

/// What the admin API accepts: the derived token and, until the panel is updated, the legacy
/// `FLICKSYNC_ADMIN_TOKEN`.
pub struct AdminTokens {
    pub(crate) derived: Option<String>,
    pub(crate) legacy: Option<String>,
}

impl AdminTokens {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            derived: cfg.http.panel_password.as_deref().and_then(derive),
            legacy: cfg.http.admin_token.clone(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.derived.is_some() || self.legacy.is_some()
    }

    pub fn has_legacy(&self) -> bool {
        self.legacy.is_some()
    }

    /// Constant-time comparison against every accepted token.
    pub fn accepts(&self, presented: &str) -> bool {
        [&self.derived, &self.legacy]
            .into_iter()
            .flatten()
            .fold(false, |ok, t| {
                constant_time_eq(presented.as_bytes(), t.as_bytes()) | ok
            })
    }
}
```

`src/app.rs`: add `pub admin: Arc<AdminTokens>,` to `AppState`, initialise it in `with_clock` with `admin: Arc::new(AdminTokens::from_config(&boot))` (build it before `boot` is moved into the `Arc`), and in `serve` after the "listening" log:

```rust
    if state.admin.has_legacy() {
        warn!("FLICKSYNC_ADMIN_TOKEN is deprecated: set PANEL_PASSWORD instead, the admin token is derived from it");
    }
    if !state.admin.is_enabled() {
        info!("admin API is off: set PANEL_PASSWORD (10+ characters) to turn it on");
    }
```

with `use crate::admin_token::AdminTokens;`.

`src/api/admin.rs`: rewrite the extractor body:

```rust
        if !state.admin.is_enabled() {
            return Err(StatusCode::NOT_FOUND.into_response());
        }
        let ok = bearer_token(&parts.headers).is_some_and(|t| state.admin.accepts(t));
```

(remove the now-unused `constant_time_eq` import there if the compiler says so) and update the module doc comment: `Disabled (404) unless PANEL_PASSWORD (10+ characters) or the deprecated FLICKSYNC_ADMIN_TOKEN is set.`

- [ ] **Step 4: Run to verify success**

Run: `cargo test --lib admin_token && cargo test --test admin && cargo test 2>&1 | grep -E "^test result|FAILED"`
Expected: PASS. The existing admin tests keep passing through the legacy token.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -3
git add -A src tests
git commit -m "feat(admin): admin token derived from PANEL_PASSWORD

The legacy FLICKSYNC_ADMIN_TOKEN is still accepted, with a deprecation warning.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Admin routes for modules and settings

**Files:**
- Modify: `src/api/admin.rs`, `src/api/mod.rs`
- Create: `tests/settings_api.rs`

**Interfaces:**
- Consumes: `AppState::{module_status, start_module, stop_module, reload_module, reload_server}`, `Settings::{view, put}`.
- Produces (all under `/admin/v1`, bearer admin token, `no-store`):
  - `GET /modules` -> `{"modules": [ModuleStatus, ...]}`
  - `POST /modules/{id}/{start|stop|reload}` -> `ModuleStatus`; unknown id or action: `404`
  - `GET /settings/{server|flicksync|flickdd}` -> `ScopeView`; unknown scope: `404`
  - `PUT /settings/{scope}` with body `{"values": {"NAME": "string" | number | bool | null}}` -> `ScopeView`; invalid: `400` `INVALID_PAYLOAD` with the reason; nothing written
  - `POST /settings/server/reload` -> `{"reloaded": true}`; failure: `400` `INVALID_PAYLOAD`, old runtime kept

- [ ] **Step 1: Write the failing tests**

`tests/settings_api.rs`:

```rust
//! Admin API: module lifecycle and settings.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{Value, json};

const PASSWORD: &str = "correct horse battery staple";
const ADMIN: &str = "602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011";

async fn server(extra: &[(&str, &str)]) -> TestServer {
    let mut vars = vec![("PANEL_PASSWORD", PASSWORD)];
    vars.extend_from_slice(extra);
    TestServer::start(&vars).await
}

async fn call(s: &TestServer, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    s.http(method, path, Some(ADMIN), body).await
}

fn field<'a>(view: &'a Value, name: &str) -> &'a Value {
    view["fields"].as_array().unwrap().iter().find(|f| f["name"] == name).unwrap()
}

#[tokio::test]
async fn modules_can_be_listed_started_stopped_and_reloaded() {
    let s = server(&[("FLICKSYNC_ENABLED", "false")]).await;
    let (st, body) = call(&s, Method::GET, "/admin/v1/modules", None).await;
    assert_eq!(st, StatusCode::OK);
    let states: Vec<_> = body["modules"].as_array().unwrap().iter().map(|m| (m["id"].as_str().unwrap(), m["state"].as_str().unwrap())).collect();
    assert_eq!(states, [("flicksync", "stopped"), ("flickdd", "stopped")]);

    let (st, m) = call(&s, Method::POST, "/admin/v1/modules/flicksync/start", None).await;
    assert_eq!((st, m["state"].as_str(), m["enabled"].as_bool()), (StatusCode::OK, Some("running"), Some(true)));
    let (_, m) = call(&s, Method::POST, "/admin/v1/modules/flicksync/reload", None).await;
    assert_eq!(m["state"], "running");
    let (_, m) = call(&s, Method::POST, "/admin/v1/modules/flicksync/stop", None).await;
    assert_eq!((m["state"].as_str(), m["enabled"].as_bool()), (Some("stopped"), Some(false)));

    assert_eq!(call(&s, Method::POST, "/admin/v1/modules/nope/start", None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&s, Method::POST, "/admin/v1/modules/flicksync/explode", None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_module_routes_need_the_admin_token() {
    let s = server(&[]).await;
    let (st, _) = s.http(Method::GET, "/admin/v1/modules", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = s.http(Method::PUT, "/admin/v1/settings/flickdd", Some("wrong-token-0123456789"), Some(json!({"values": {}}))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn settings_are_read_with_their_source_and_default() {
    let s = server(&[("FLICKSYNC_MAX_ROOM_SIZE", "5")]).await;
    let (st, v) = call(&s, Method::GET, "/admin/v1/settings/flicksync", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["scope"], "flicksync");
    let f = field(&v, "FLICKSYNC_MAX_ROOM_SIZE");
    assert_eq!((f["value"].as_str(), f["source"].as_str(), f["default"].as_str()), (Some("5"), Some("environment"), Some("100")));
    assert_eq!(field(&v, "FLICKSYNC_MAX_ROOMS")["source"], "default");
    assert_eq!(call(&s, Method::GET, "/admin/v1/settings/nope", None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_put_changes_the_value_and_flags_a_pending_reload() {
    let s = server(&[]).await;
    let (st, v) = call(&s, Method::PUT, "/admin/v1/settings/flicksync", Some(json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": 7, "FLICKSYNC_CHAT_ENABLED": false}}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!((field(&v, "FLICKSYNC_MAX_ROOM_SIZE")["value"].as_str(), field(&v, "FLICKSYNC_MAX_ROOM_SIZE")["source"].as_str()), (Some("7"), Some("panel")));
    assert_eq!(field(&v, "FLICKSYNC_CHAT_ENABLED")["value"], "false");
    let (_, mods) = call(&s, Method::GET, "/admin/v1/modules", None).await;
    assert_eq!(mods["modules"][0]["pending_reload"], true);
    // null removes the stored value again.
    let (_, v) = call(&s, Method::PUT, "/admin/v1/settings/flicksync", Some(json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": null}}))).await;
    assert_eq!(field(&v, "FLICKSYNC_MAX_ROOM_SIZE")["source"], "default");
}

#[tokio::test]
async fn an_invalid_put_is_refused_and_writes_nothing() {
    let s = server(&[]).await;
    let before = s.state.settings.revision();
    for body in [
        json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": 0}}),
        json!({"values": {"NOPE": 1}}),
        json!({"values": {"FLICKDD_MAX_PARALLEL": 1}}),
        json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": [1]}}),
        json!({"nothing": true}),
    ] {
        let (st, v) = call(&s, Method::PUT, "/admin/v1/settings/flicksync", Some(body.clone())).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(v["error"]["code"], "INVALID_PAYLOAD");
    }
    assert_eq!(s.state.settings.revision(), before, "nothing was written");
}

#[tokio::test]
async fn secrets_are_write_only() {
    let s = server(&[]).await;
    let put = |values: Value| call(&s, Method::PUT, "/admin/v1/settings/flickdd", Some(json!({"values": values})));

    let (st, v) = put(json!({"FLICKDD_JELLYFIN_URL": "http://jf:8096", "FLICKDD_JELLYFIN_API_KEY": "top-secret-jf-key"})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(!v.to_string().contains("top-secret-jf-key"), "the PUT answer");
    let key = field(&v, "FLICKDD_JELLYFIN_API_KEY");
    assert_eq!((key["secret"].as_bool(), key["set"].as_bool(), key["value"].is_null()), (Some(true), Some(true), true));
    let (_, got) = call(&s, Method::GET, "/admin/v1/settings/flickdd", None).await;
    assert!(!got.to_string().contains("top-secret-jf-key"), "the GET answer");

    // Omitted: kept. The module starts with the stored key.
    let (st, _) = put(json!({"FLICKDD_MAX_PARALLEL": 3})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s.state.settings.lookup("FLICKDD_JELLYFIN_API_KEY").as_deref(), Some("top-secret-jf-key"));

    // A string replaces it, null clears it.
    put(json!({"FLICKDD_JELLYFIN_API_KEY": "second-secret"})).await;
    assert_eq!(s.state.settings.lookup("FLICKDD_JELLYFIN_API_KEY").as_deref(), Some("second-secret"));
    let (st, _) = put(json!({"FLICKDD_JELLYFIN_API_KEY": null})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "the URL alone is not a complete backend");
    let (st, _) = put(json!({"FLICKDD_JELLYFIN_URL": null, "FLICKDD_JELLYFIN_API_KEY": null})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s.state.settings.lookup("FLICKDD_JELLYFIN_API_KEY"), None);

    // Module status never carries them either.
    let (_, mods) = call(&s, Method::GET, "/admin/v1/modules", None).await;
    assert!(!mods.to_string().contains("secret"));
}

#[tokio::test]
async fn flickdd_can_be_configured_started_and_reloaded_from_the_api() {
    let s = server(&[]).await;
    let (st, m) = call(&s, Method::POST, "/admin/v1/modules/flickdd/start", None).await;
    assert_eq!((st, m["state"].as_str()), (StatusCode::OK, Some("failed")), "no backend yet: {m}");

    let (st, _) = call(&s, Method::PUT, "/admin/v1/settings/flickdd", Some(json!({"values": {"FLICKDD_JELLYFIN_URL": "http://jf:8096", "FLICKDD_JELLYFIN_API_KEY": "k"}}))).await;
    assert_eq!(st, StatusCode::OK);
    let (_, m) = call(&s, Method::POST, "/admin/v1/modules/flickdd/reload", None).await;
    assert_eq!((m["state"].as_str(), m["pending_reload"].as_bool()), (Some("running"), Some(false)), "{m}");
    assert!(s.state.dd().is_some());
}

#[tokio::test]
async fn the_server_scope_reloads_through_the_api() {
    let s = server(&[]).await;
    let (st, _) = call(&s, Method::PUT, "/admin/v1/settings/server", Some(json!({"values": {"FLICKSYNC_PUBLIC_URL": "https://flick.example.com/services"}}))).await;
    assert_eq!(st, StatusCode::OK);
    let (_, inv) = call(&s, Method::GET, "/admin/v1/invite", None).await;
    assert!(!inv["address"].as_str().unwrap().contains("services"), "not before the reload");
    let (st, body) = call(&s, Method::POST, "/admin/v1/settings/server/reload", None).await;
    assert_eq!((st, body["reloaded"].as_bool()), (StatusCode::OK, Some(true)));
    let (_, inv) = call(&s, Method::GET, "/admin/v1/invite", None).await;
    assert_eq!(inv["address"], "https://flick.example.com/services");
    assert!(inv["url"].as_str().unwrap().starts_with("flickserver://flick.example.com/services/"));
}

#[tokio::test]
async fn responses_are_never_cacheable() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let s = server(&[]).await;
    for (method, path, body) in [
        (Method::GET, "/admin/v1/modules", None),
        (Method::GET, "/admin/v1/settings/server", None),
        (Method::PUT, "/admin/v1/settings/flicksync", Some(r#"{"values":{"FLICKSYNC_MAX_ROOM_SIZE":0}}"#)),
        (Method::POST, "/admin/v1/modules/flicksync/reload", None),
    ] {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {ADMIN}"));
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let resp = flicksync::app::build_router(s.state.clone())
            .oneshot(req.body(Body::from(body.unwrap_or(""))).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.headers()["cache-control"], "no-store", "{path}");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test settings_api`
Expected: FAIL (routes answer 404/405).

- [ ] **Step 3: Implement**

`src/api/admin.rs` additions:

```rust
use std::collections::BTreeMap;

use axum::extract::rejection::JsonRejection;
use serde::Deserialize;

use crate::modules::ModuleId;
use crate::settings::store::Scope;
use crate::settings::SettingsError;

fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError(Error::new(ErrorCode::InvalidPayload, message))
}

fn settings_error(e: SettingsError) -> ApiError {
    match e {
        SettingsError::Store(inner) => {
            warn!(error = %inner, "could not save the settings");
            ApiError(Error::new(ErrorCode::Internal, "could not save the settings"))
        }
        other => bad_request(other.to_string()),
    }
}

/// `GET /admin/v1/modules`
pub async fn modules(_: AdminAuth, State(state): State<AppState>) -> Response {
    let list: Vec<_> = ModuleId::ALL
        .into_iter()
        .map(|id| state.module_status(id))
        .collect();
    no_store(StatusCode::OK, json!({ "modules": list }))
}

/// `POST /admin/v1/modules/{id}/{start|stop|reload}`
pub async fn module_action(
    _: AdminAuth,
    State(state): State<AppState>,
    Path((id, action)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let Some(id) = ModuleId::from_id(&id) else {
        return Ok(not_found());
    };
    let status = match action.as_str() {
        "start" => state.start_module(id).map_err(settings_error)?,
        "stop" => state.stop_module(id).map_err(settings_error)?,
        "reload" => state.reload_module(id),
        _ => return Ok(not_found()),
    };
    Ok(no_store(StatusCode::OK, status))
}

/// `GET /admin/v1/settings/{scope}`
pub async fn get_settings(
    _: AdminAuth,
    State(state): State<AppState>,
    Path(scope): Path<String>,
) -> Response {
    match Scope::from_id(&scope) {
        Some(scope) => no_store(StatusCode::OK, state.settings.view(scope)),
        None => not_found(),
    }
}

#[derive(Deserialize)]
pub struct PutSettings {
    values: BTreeMap<String, serde_json::Value>,
}

/// `PUT /admin/v1/settings/{scope}`: a partial update. Strings, numbers and booleans set a
/// value, `null` removes it, an omitted name is left alone.
pub async fn put_settings(
    _: AdminAuth,
    State(state): State<AppState>,
    Path(scope): Path<String>,
    body: Result<Json<PutSettings>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Some(scope) = Scope::from_id(&scope) else {
        return Ok(not_found());
    };
    let Json(body) = body.map_err(|_| bad_request("body must be {\"values\": {...}}"))?;
    let mut patch = BTreeMap::new();
    for (name, value) in body.values {
        let value = match value {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => return Err(bad_request(format!("{name}: expected a string, number, boolean or null"))),
        };
        patch.insert(name, value);
    }
    let view = state.settings.put(scope, patch).map_err(settings_error)?;
    Ok(no_store(StatusCode::OK, view))
}

/// `POST /admin/v1/settings/server/reload`
pub async fn reload_server(
    _: AdminAuth,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    state
        .reload_server()
        .map_err(|e| bad_request(e.to_string()))?;
    Ok(no_store(StatusCode::OK, json!({ "reloaded": true })))
}
```

`src/api/mod.rs`: add to the `admin` router (before the `.layer(...)`), importing `put` from `axum::routing`:

```rust
        .route("/admin/v1/modules", get(admin::modules))
        .route("/admin/v1/modules/{id}/{action}", post(admin::module_action))
        .route(
            "/admin/v1/settings/{scope}",
            get(admin::get_settings).put(admin::put_settings),
        )
        .route("/admin/v1/settings/server/reload", post(admin::reload_server))
```

- [ ] **Step 4: Run to verify success**

Run: `cargo test --test settings_api && cargo test 2>&1 | grep -E "^test result|FAILED"`
Expected: PASS. If `/settings/server/reload` is shadowed by `/settings/{scope}` (POST on a `{scope}` route that only allows GET/PUT), axum 0.8 resolves the more specific static route first, which is what the test checks.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt && cargo clippy --all-targets 2>&1 | tail -3
git add -A src tests
git commit -m "feat(admin): module and settings routes

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Documentation and final verification

**Files:**
- Modify: `docs/admin-api.md`, `docs/protocol.md`, `docs/openapi.yaml`, `docs/deployment.md`, `.env.example`, `README.md`
- Test: full suite

- [ ] **Step 1: Document the new API and the stopped-module behaviour**

`docs/admin-api.md`: update the intro (`Disabled (404) unless PANEL_PASSWORD has 10+ characters; the token is hex(HMAC-SHA256(PANEL_PASSWORD, "flick-admin-api-v1")). The old FLICKSYNC_ADMIN_TOKEN still works and is deprecated.`), add these rows to the endpoints table, and one short section each (request, answer, errors) for them:

```
| `GET /admin/v1/modules` | State of each module: `stopped`, `running` or `failed` (with the reason), the persisted `enabled` switch, `pending_reload` |
| `POST /admin/v1/modules/{id}/start` | Persist `enabled=true` and start (`flicksync` or `flickdd`) |
| `POST /admin/v1/modules/{id}/stop` | Persist `enabled=false` and stop: rooms close, downloads are cut |
| `POST /admin/v1/modules/{id}/reload` | Stop then start with the stored settings; a disabled module stays stopped |
| `GET /admin/v1/settings/{server|flicksync|flickdd}` | Every editable setting with value, source (`panel`, `environment`, `default`), default; secrets only as `set` |
| `PUT /admin/v1/settings/{scope}` | Partial update `{"values": {"NAME": value-or-null}}`; the result is validated as a whole, `400` and nothing written otherwise |
| `POST /admin/v1/settings/server/reload` | Apply the server scope (keys, public address, CORS, metrics, log level) without restarting modules |
```

Also note in `GET /admin/v1/overview` that `running` is false and the counters are zero while FlickSync is stopped, and that `/rooms`, `/stats` answer `503 MODULE_DISABLED` then.

`docs/protocol.md`: add to the error table `| MODULE_DISABLED | 503 | The module behind this route is stopped on the server (enabled from the admin panel). |`.

`docs/openapi.yaml`: add `MODULE_DISABLED` wherever the `ErrorBody` code enum lists codes, and a `503` description `MODULE_DISABLED` on `/api/v1/rooms*` operations (follow how `502` is written for the downloads operations).

`docs/deployment.md`: add a section "Upgrading to panel-managed settings" stating: settings are stored in `/data/settings.json` (the volume already mounted), stored values beat the environment, modules are **off by default**; to keep an existing install running set `FLICKSYNC_ENABLED=true` (and keep `FLICKDD_ENABLED=true` if used) once, or enable the modules from the panel; `FLICKSYNC_ADMIN_TOKEN` is replaced by `PANEL_PASSWORD`; do not expose `/admin` through the reverse proxy.

`.env.example`: add under section 3 a commented `#FLICKSYNC_ENABLED=false` with the comment `Start FlickSync at boot. The panel can also switch it (stored in /data/settings.json, which wins over this).`, and under section 2 replace the `FLICKSYNC_ADMIN_TOKEN` block by a note that the admin token is derived from `PANEL_PASSWORD` (the full `.env` rewrite is sub-project 4).

`README.md`: one line pointing to the upgrade section if it lists configuration.

- [ ] **Step 2: Full verification**

```bash
cargo fmt --check && echo FMT_OK
cargo clippy --all-targets 2>&1 | tail -3
cargo test 2>&1 | grep -E "^test result|FAILED|panicked"
git grep -n "state\.cfg\|state\.manager\|state\.auth\b" -- src tests || echo NO_OLD_ACCESS
cd panel && npx tsc --noEmit && npm test 2>&1 | grep -E "^ℹ (pass|fail)"
```

Expected: `FMT_OK`, clippy clean, every `test result: ok`, `NO_OLD_ACCESS`, panel typecheck and 10 passing tests (the panel is untouched by this plan).

- [ ] **Step 3: Manual check against a running container**

```bash
docker compose build flick-modules
PANEL_PASSWORD='correct horse battery staple' docker compose up -d flick-modules
# fresh install: nothing is running
curl -s localhost:8787/health
curl -s -o /dev/null -w "%{http_code}\n" -X POST localhost:8787/api/v1/rooms           # 401 (no token) or 503, never a crash
TOKEN=602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011
curl -s -H "Authorization: Bearer $TOKEN" localhost:8787/admin/v1/modules
curl -s -X POST -H "Authorization: Bearer $TOKEN" localhost:8787/admin/v1/modules/flicksync/start
docker compose restart flick-modules     # still enabled after a restart
```

Expected: both modules `stopped` at first; `flicksync` `running` after the start call and again after the restart. (The compose service must publish or `exec` into port 8787 for the `curl` calls; use `docker compose exec` with the image's own tools or a temporary port mapping if needed.)

- [ ] **Step 4: Commit**

```bash
git add -A docs .env.example README.md
git commit -m "docs: admin API for modules and settings, upgrade notes

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Self-review

**Spec coverage**

| Spec section | Task |
|---|---|
| 3.1 Modules, slots, idempotence, gate, `Failed`, boot survives a failing module | 3, 4, 5 |
| 3.2 Routes stay mounted, 503 / 404 per module, `/health`, `/ready`, stop closes rooms and cuts streams | 3 (error), 4 (accessors), 5 (tests) |
| 3.3 Server scope: runtime config, CORS per request, log level, reload without restarting modules | 4, 6 |
| 4.1 File, atomic, 0600, revision, version, unknown keys, fingerprint for `pending_reload` | 1, 2, 3 |
| 4.2 Layered lookup, same validation, sources, `FLICKSYNC_ENABLED` fallback | 2 |
| 4.3 What leaves the environment (catalogue and boot-only list, drift test) | 2 |
| 4.4 Secrets write-only, omitted / string / null | 2 (view), 8 (API) |
| 5 Admin API (modules, settings, server reload), derived token, legacy compat | 7, 8 |
| 6 Error handling (invalid never written, corrupt file refused, panic containment) | 1, 2, 5. Note: panic containment relies on the gate being poison-tolerant (`unwrap_or_else(into_inner)`), already used everywhere in `Slot` |
| 7 Security (no socket, admin not routed publicly, nothing in memory when stopped) | 9 docs, 5 tests |
| 8 Testing list | spread over 1 to 8 |
| 9 Rollout | task order |

**Type consistency.** `Scope`, `ModuleId`, `Slot`, `SlotState`, `Running::new(rt, fingerprint)`, `ModuleStatus`, `Settings::{put, view, set_enabled, is_enabled, fingerprint, revision, lookup, config}`, `AppState::{server, sync, sync_opt, dd, module_status, start_module, stop_module, reload_module, reload_server, set_log_control, stop_all}` are used with the same names and signatures in every task. `AppState.admin` is added in Task 7 and read only by `AdminAuth`.

**Known judgment calls** (implementer: do not "fix" silently)
- `FLICKSYNC_ENABLED` is a new variable so an existing install can keep FlickSync on; the default stays `false` as requested.
- `PUT` validates through the same `Config::from_lookup`; a bad environment value therefore also blocks saves until fixed (it would block boot anyway).
- When no origin is allowed, the CORS layer is still present and answers preflights without an allow-origin header, instead of `405`.

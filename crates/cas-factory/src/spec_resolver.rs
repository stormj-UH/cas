//! Cascade resolver for [`WorkerSpec`].
//!
//! Resolves per-worker configuration by merging six layers in order (last
//! wins):
//!
//! 1. **Built-in defaults** — Claude / no model / High effort.
//! 2. **User config** — `~/.cas/config.toml` `[factory.defaults]`.
//! 3. **Project config** — `<cwd>/.cas/config.toml` `[factory.defaults]`.
//! 4. **Project per-worker** — `[[factory.workers]]` entries (by position).
//! 5. **CLI flags** — `--worker-cli`, `--worker-model`, `--worker-effort`.
//! 6. **Per-worker JSON** — `--worker-spec '{"name":"alice","cli":"codex"}'`
//!    (repeatable; matched by name then sequential position).

use std::io;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

use cas_mux::{Effort, SupervisorCli, WorkerSpec};

// ─────────────────────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────────────────────

/// Errors produced by the cascade resolver.
#[derive(Error, Debug)]
pub enum SpecResolverError {
    /// A config file could not be read from disk.
    #[error("failed to read config file {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A config file could not be parsed as TOML.
    #[error("failed to parse config file {path}: {source}")]
    ParseConfig {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// A `--worker-spec` value was not valid JSON.
    #[error("invalid --worker-spec JSON: {0}")]
    InvalidWorkerSpec(String),

    /// An effort string was not recognised.
    #[error("invalid effort value {0:?}: {1}")]
    InvalidEffort(String, String),

    /// A CLI string was not recognised.
    #[error("invalid cli value {0:?}: expected 'claude' or 'codex'")]
    InvalidCli(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// TOML config file schema (crate-private)
// ─────────────────────────────────────────────────────────────────────────────

/// `[factory.defaults]` section — all fields optional.
#[derive(Debug, Default, Deserialize)]
struct FactoryDefaultsToml {
    cli: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

/// One `[[factory.workers]]` entry — all fields optional.
#[derive(Debug, Default, Deserialize)]
struct FactoryWorkerToml {
    name: Option<String>,
    cli: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

/// `[factory]` table.
#[derive(Debug, Default, Deserialize)]
struct FactoryToml {
    defaults: Option<FactoryDefaultsToml>,
    #[serde(default)]
    workers: Vec<FactoryWorkerToml>,
}

/// Minimal wrapper so we can ignore non-`factory` sections.
#[derive(Debug, Default, Deserialize)]
struct ConfigFileToml {
    factory: Option<FactoryToml>,
}

// ─────────────────────────────────────────────────────────────────────────────
// `--worker-spec` JSON schema (crate-private)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WorkerSpecJson {
    name: Option<String>,
    cli: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// All sources fed into the cascade resolver.
///
/// All fields have sensible `Default` values (skip layers whose paths don't
/// exist, no CLI overrides, no JSON overrides).
#[derive(Debug, Default)]
pub struct ConfigSources {
    /// Path to the user config file.
    ///
    /// `None` → use `~/.cas/config.toml` (resolved at call time via
    /// [`dirs::home_dir`]).  Pass a path that does not exist to skip this
    /// layer in tests.
    pub user_config: Option<PathBuf>,

    /// Path to the project config file.
    ///
    /// `None` → skip the project layer entirely.  Callers should pass
    /// `Some(cwd.join(".cas/config.toml"))` when the project root is known.
    pub project_config: Option<PathBuf>,

    /// Global `--worker-cli` override — applied to every slot.
    pub cli_flag: Option<SupervisorCli>,

    /// Global `--worker-model` override — applied to every slot.
    pub model_flag: Option<String>,

    /// Global `--worker-effort` override — applied to every slot.
    pub effort_flag: Option<Effort>,

    /// Raw JSON strings from repeated `--worker-spec` occurrences.
    ///
    /// Each string must deserialise as a JSON object with optional fields
    /// `name`, `cli`, `model`, `effort`.
    pub worker_spec_jsons: Vec<String>,
}

/// Resolve `workers` [`WorkerSpec`] slots from the layered config sources.
///
/// Returns a `Vec<WorkerSpec>` of length `workers`.  Returns an empty vec
/// when `workers == 0`.
///
/// # Errors
///
/// - A config file that exists but cannot be read or parsed produces
///   [`SpecResolverError::ReadConfig`] / [`SpecResolverError::ParseConfig`].
/// - An unparseable `--worker-spec` JSON produces
///   [`SpecResolverError::InvalidWorkerSpec`].
/// - Unknown `cli` or `effort` string values in any layer produce
///   [`SpecResolverError::InvalidCli`] / [`SpecResolverError::InvalidEffort`].
pub fn resolve_specs(
    workers: usize,
    sources: ConfigSources,
) -> Result<Vec<WorkerSpec>, SpecResolverError> {
    if workers == 0 {
        return Ok(vec![]);
    }

    // ── Layer 1: built-in defaults ────────────────────────────────────────
    let mut specs: Vec<WorkerSpec> = (0..workers)
        .map(|_| WorkerSpec::builtin_default())
        .collect();

    // ── Layer 2: user config (~/.cas/config.toml [factory.defaults]) ──────
    let user_path = sources
        .user_config
        .clone()
        .or_else(|| dirs::home_dir().map(|h| h.join(".cas").join("config.toml")));

    if let Some(ref path) = user_path {
        if let Some((defaults, _per_worker)) = load_config_file(path)? {
            if let Some(d) = defaults {
                apply_defaults_to_all(&mut specs, &d)?;
            }
        }
    }

    // ── Layer 3 + 4: project config (.cas/config.toml) ───────────────────
    if let Some(ref path) = sources.project_config {
        if let Some((defaults, per_worker)) = load_config_file(path)? {
            // 3. [factory.defaults]
            if let Some(d) = defaults {
                apply_defaults_to_all(&mut specs, &d)?;
            }
            // 4. [[factory.workers]] — by position
            for (i, wt) in per_worker.iter().enumerate() {
                if let Some(slot) = specs.get_mut(i) {
                    apply_worker_toml(slot, wt)?;
                }
            }
        }
    }

    // ── Layer 5: CLI flags (apply to every slot) ──────────────────────────
    for spec in specs.iter_mut() {
        if let Some(cli) = sources.cli_flag {
            spec.cli = cli;
        }
        if let Some(ref model) = sources.model_flag {
            spec.model = Some(model.clone());
        }
        if let Some(effort) = sources.effort_flag {
            spec.effort = Some(effort);
        }
    }

    // ── Layer 6: --worker-spec JSON overrides ─────────────────────────────
    //
    // Named specs: find an existing slot by name, or claim the next
    // positional slot and assign the name.  Unnamed specs: claim the next
    // positional slot.  A shared cursor tracks sequential slot consumption.
    let mut cursor: usize = 0;

    for json_str in &sources.worker_spec_jsons {
        let parsed: WorkerSpecJson =
            serde_json::from_str(json_str).map_err(|e| {
                SpecResolverError::InvalidWorkerSpec(e.to_string())
            })?;

        let target_idx: Option<usize> = if let Some(ref name) = parsed.name {
            // Prefer an existing named slot; fall back to cursor.
            specs
                .iter()
                .position(|s| s.name.as_deref() == Some(name.as_str()))
                .or_else(|| (cursor < specs.len()).then_some(cursor))
        } else {
            // No name: take the next cursor slot.
            (cursor < specs.len()).then_some(cursor)
        };

        if let Some(i) = target_idx {
            apply_json_spec(&mut specs[i], &parsed)?;
            // Advance cursor only when we consumed a positional (non-name-matched) slot.
            // A name-matched slot is one that already existed before cursor reached it
            // (i.e. i < cursor).  A cursor-consumed slot is i == cursor.
            let name_matched = parsed.name.is_some() && i < cursor;
            if !name_matched && i == cursor {
                cursor += 1;
            }
        }
    }

    Ok(specs)
}

// ─────────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read and parse a TOML config file.
///
/// Returns `Some((defaults_section, per_worker_entries))` when the file exists
/// and parses successfully, or `None` when the file does not exist.
///
/// Avoids the TOCTOU race of `path.exists()` + `read_to_string` by attempting
/// the read directly and treating `NotFound` as an absent file.
fn load_config_file(
    path: &std::path::Path,
) -> Result<Option<(Option<FactoryDefaultsToml>, Vec<FactoryWorkerToml>)>, SpecResolverError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(SpecResolverError::ReadConfig {
                path: path.to_path_buf(),
                source: e,
            })
        }
    };
    let config: ConfigFileToml =
        toml::from_str(&text).map_err(|e| SpecResolverError::ParseConfig {
            path: path.to_path_buf(),
            source: e,
        })?;
    let factory = config.factory.unwrap_or_default();
    Ok(Some((factory.defaults, factory.workers)))
}

/// Apply a `[factory.defaults]` section to every spec in the vec.
fn apply_defaults_to_all(
    specs: &mut [WorkerSpec],
    d: &FactoryDefaultsToml,
) -> Result<(), SpecResolverError> {
    for spec in specs.iter_mut() {
        if let Some(ref s) = d.cli {
            spec.cli = parse_cli(s)?;
        }
        if let Some(ref m) = d.model {
            spec.model = Some(m.clone());
        }
        if let Some(ref s) = d.effort {
            spec.effort = Some(parse_effort(s)?);
        }
    }
    Ok(())
}

/// Apply one `[[factory.workers]]` TOML entry to a single spec.
fn apply_worker_toml(
    spec: &mut WorkerSpec,
    wt: &FactoryWorkerToml,
) -> Result<(), SpecResolverError> {
    if let Some(ref n) = wt.name {
        spec.name = Some(n.clone());
    }
    if let Some(ref s) = wt.cli {
        spec.cli = parse_cli(s)?;
    }
    if let Some(ref m) = wt.model {
        spec.model = Some(m.clone());
    }
    if let Some(ref s) = wt.effort {
        spec.effort = Some(parse_effort(s)?);
    }
    Ok(())
}

/// Apply a parsed `--worker-spec` JSON override to a single spec.
fn apply_json_spec(
    spec: &mut WorkerSpec,
    json: &WorkerSpecJson,
) -> Result<(), SpecResolverError> {
    if let Some(ref n) = json.name {
        spec.name = Some(n.clone());
    }
    if let Some(ref s) = json.cli {
        spec.cli = parse_cli(s)?;
    }
    if let Some(ref m) = json.model {
        spec.model = Some(m.clone());
    }
    if let Some(ref s) = json.effort {
        spec.effort = Some(parse_effort(s)?);
    }
    Ok(())
}

fn parse_cli(s: &str) -> Result<SupervisorCli, SpecResolverError> {
    s.parse::<SupervisorCli>()
        .map_err(|_| SpecResolverError::InvalidCli(s.to_string()))
}

fn parse_effort(s: &str) -> Result<Effort, SpecResolverError> {
    s.parse::<Effort>()
        .map_err(|e| SpecResolverError::InvalidEffort(s.to_string(), e))
}

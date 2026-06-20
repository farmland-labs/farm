//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Public protocol surface between `farm` and the orchestrator
//! that invokes it.
//!
//! Concretely: the on-disk schema for
//! `.farm/ctx/{ctx}/ops/{operation}/context.json`. The orchestrator
//! writes this file before invoking `farm`; `farm` reads it for
//! `farm info` display and forwards a strict subset of the
//! [`Identity`] fields into child Farmfile scripts as `FARM_*`
//! env vars.
//!
//! # Trust zones — file-level separation
//!
//! The orchestrator-to-`farm` handoff carries data of two
//! qualitatively different kinds, and they are kept in
//! *separate files* so the trust boundary is impossible to
//! confuse:
//!
//! ```text
//! .farm/ctx/{ctx}/ops/{op}/
//!   context.json              ← typed, trusted (this crate's schema)
//!   external/
//!     vcs.json                ← webhook-derived data
//!     <source>.json           ← other untrusted sources
//!   log/
//! ```
//!
//! - **`context.json`** — typed [`ContextManifest`], described by
//!   this crate. All fields are orchestrator-generated identifiers
//!   (`pipeline_id`, `pipeline_run_id`, `job_id`, `stage`). Strictly
//!   typed; `#[serde(deny_unknown_fields)]` rejects manifests with
//!   keys this version doesn't know. The only zone eligible for
//!   env-var export, after per-value validation.
//!
//! - **`external/<source>.json`** — opaque JSON files (one per
//!   untrusted source). Conventional sources include `vcs.json`
//!   (webhook payloads, branch names, commit messages, PR titles),
//!   but orchestrators may add more (e.g. third-party API responses,
//!   user-supplied dispatch payloads). **Treated as untrusted.**
//!   Never exported to env vars. Never validated by `farm`. This
//!   crate provides no Rust type for them — they're opaque by
//!   design. Direct file readers in Farmfile scripts are
//!   responsible for their own validation.
//!
//! This split exists because CI tooling generically suffers
//! supply-chain attacks via webhook-derived data (branch names
//! with shell metacharacters, commit messages with injection
//! payloads, etc.). File-level separation makes it *physically
//! impossible* to mistake an `external/*.json` for a typed
//! manifest — a different file, a different parser, a different
//! trust posture. The [`EXTERNAL_SUBDIR`] constant pins the
//! convention name.
//!
//! # Trust-zone defences enforced by this crate
//!
//! 1. **Typed identity fields.** [`Identity`] has fixed
//!    `Option<String>` fields. Non-string JSON values fail to
//!    deserialise. Unknown fields hard-fail via
//!    `deny_unknown_fields`. There is no "free-form" key here.
//! 2. **No external data in the typed schema.** Untrusted bytes
//!    live in `external/*.json` files, never in `context.json`.
//!    This crate provides no API for reading those — by design.
//! 3. **Fixed `FARM_` prefix** ([`ENV_PREFIX`]) on every exported
//!    var, applied centrally — never bypassed.
//! 4. **Byte-content checks** on each exported value: NUL, `\n`,
//!    `\r` are rejected. They break env-var round-trip on POSIX
//!    or confuse shells with a non-default `IFS`.
//! 5. **Length caps**: [`MAX_VALUE_LEN`] per value, and a
//!    manifest-level [`MAX_MANIFEST_BYTES`] cap enforced at
//!    [`ContextManifest::from_json_slice`]. The latter protects
//!    against DoS via gigantic `context.json` files.
//! 6. **No env shadowing**: if a target name (e.g. `FARM_STAGE`)
//!    is already set in the caller's environment, the sanitizer
//!    skips it rather than overwriting. The skip is reported.
//! 7. **Structured reporting**: every skipped value is returned
//!    with a [`SkipReason`], not silently dropped. Callers
//!    SHOULD log these at warn-level so unexpected rejects
//!    surface.
//!
//! # Two channels, two trust levels — what reaches a child process
//!
//! - **Env vars (safe).** Only validated [`Identity`] fields,
//!   each starting with [`ENV_PREFIX`]. Guaranteed: no NULs, no
//!   newlines, byte-capped, won't shadow existing env. Use this
//!   channel whenever you can.
//! - **Direct file read (unsafe).** Child scripts that know where
//!   the operation directory lives (via `FARM_DIR` or similar)
//!   can open `external/*.json` and parse those blobs themselves.
//!   This path bypasses the sanitizer entirely; bytes there may
//!   carry attacker-influenced content (newlines, binary,
//!   shell-special chars). Authors reading those files MUST NOT
//!   pass values to `eval`-like consumers, MUST quote when
//!   substituting into commands, and SHOULD prefer the
//!   `FARM_*` env vars wherever possible.

use serde::{Deserialize, Serialize};

/// Monotonic schema version of [`ContextManifest`].
///
/// Bumped on breaking changes only. Additive changes (new
/// optional fields with `#[serde(default)]`) do not bump.
/// Readers MUST fail when `manifest.schema_version > SCHEMA_VERSION`
/// with a clear "upgrade your reader" message.
pub const SCHEMA_VERSION: u32 = 1;

/// Hard cap on `context.json` byte length, enforced at
/// [`ContextManifest::from_json_slice`]. Protects against DoS
/// from gigantic manifests; 256 KiB is far above any plausible
/// legitimate use (the typed manifest is a few dozen bytes;
/// untrusted/external data does not live here at all — see
/// [`EXTERNAL_SUBDIR`]).
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// Name of the sibling directory (next to `context.json`) where
/// orchestrators place untrusted external data files, one per
/// source.
///
/// ```text
/// .farm/ctx/{ctx}/ops/{op}/
///   context.json         ← this crate's typed schema
///   external/
///     vcs.json           ← conventional source name
///     ...                ← orchestrators may add others
/// ```
///
/// This crate provides no Rust types for files inside the
/// directory; they are opaque by design.
pub const EXTERNAL_SUBDIR: &str = "external";

/// Top-level shape of `.farm/ctx/{ctx}/ops/{operation}/context.json`.
///
/// Strictly typed — there is no opaque `Value` field. Untrusted /
/// external data lives in sibling files under [`EXTERNAL_SUBDIR`].
/// See the crate-level "Trust zones" section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextManifest {
    /// Schema version this manifest was written against. Required.
    /// `0` is not a valid version. A manifest with `schema_version`
    /// absent or set to `0` is rejected by [`Self::from_json_slice`]
    /// as [`ManifestError::MissingVersion`].
    ///
    /// `#[serde(default)]` makes a missing field deserialize as `0`
    /// so both "absent" and "= 0" funnel into the same rejection.
    #[serde(default)]
    pub schema_version: u32,

    /// Orchestrator-controlled identifiers. Trusted zone:
    /// generated by the orchestrator from its own state, not
    /// derived from external inputs. The only zone eligible for
    /// env-var export. See [`Identity`].
    #[serde(default)]
    pub identity: Identity,
}

/// Orchestrator-controlled identifiers. Typed and validated.
///
/// All fields are `Option<String>` to allow standalone
/// orchestrators (running `farm` with no pipeline context) to
/// leave them unset. The sanitizer skips any unset field
/// silently — that's not a security event, just an absent
/// optional value.
///
/// `#[serde(deny_unknown_fields)]` means a manifest that
/// includes a field this version doesn't know about (e.g. a
/// future `cluster_id`) hard-fails to parse rather than silently
/// dropping it. Forward-compat additions therefore require both
/// reader and writer updates and a [`SCHEMA_VERSION`] bump.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// Pipeline definition id (orchestrator-generated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline_id: Option<String>,
    /// Current run id within the pipeline (orchestrator-generated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline_run_id: Option<String>,
    /// Current job id within the run (orchestrator-generated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// Current stage name within the job (orchestrator-controlled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
}

impl Default for ContextManifest {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            identity: Identity::default(),
        }
    }
}

/// Errors from manifest loading.
#[derive(Debug)]
pub enum ManifestError {
    /// Manifest's `schema_version` is from a future version this
    /// reader doesn't know how to handle. Surface as "upgrade
    /// your reader".
    UnsupportedVersion {
        manifest_version: u32,
        reader_version: u32,
    },
    /// Manifest had no `schema_version` field, or it was `0`.
    /// `0` is not a valid version.
    MissingVersion,
    /// Input byte length exceeded [`MAX_MANIFEST_BYTES`].
    /// Reported with the observed length.
    TooLarge { observed: usize, cap: usize },
    /// JSON parse error (includes unknown-field rejections via
    /// `deny_unknown_fields`).
    Parse(serde_json::Error),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::UnsupportedVersion {
                manifest_version,
                reader_version,
            } => write!(
                f,
                "context.json schema_version={} is newer than this reader supports (max={}); upgrade your reader",
                manifest_version, reader_version
            ),
            ManifestError::MissingVersion => write!(
                f,
                "context.json is missing schema_version (or is 0); orchestrator must write an explicit version"
            ),
            ManifestError::TooLarge { observed, cap } => write!(
                f,
                "context.json is {} bytes, exceeds cap of {} bytes",
                observed, cap
            ),
            ManifestError::Parse(e) => write!(f, "context.json parse error: {}", e),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ManifestError::Parse(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for ManifestError {
    fn from(e: serde_json::Error) -> Self {
        ManifestError::Parse(e)
    }
}

impl ContextManifest {
    /// Parse a manifest from JSON bytes, enforcing all load-time
    /// invariants:
    ///
    /// 1. Input length <= [`MAX_MANIFEST_BYTES`] (DoS guard).
    /// 2. JSON parses successfully (no unknown fields anywhere
    ///    `deny_unknown_fields` applies).
    /// 3. `schema_version` is present and non-zero.
    /// 4. `schema_version` <= [`SCHEMA_VERSION`].
    ///
    /// Note: `external` content is NOT validated past JSON syntax.
    /// It is preserved verbatim. Validation of its contents is
    /// the consumer's responsibility (and is generally a bad idea
    /// — prefer the sanitized [`Identity`] env-var path).
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge {
                observed: bytes.len(),
                cap: MAX_MANIFEST_BYTES,
            });
        }
        let manifest: Self = serde_json::from_slice(bytes)?;
        if manifest.schema_version == 0 {
            return Err(ManifestError::MissingVersion);
        }
        if manifest.schema_version > SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedVersion {
                manifest_version: manifest.schema_version,
                reader_version: SCHEMA_VERSION,
            });
        }
        Ok(manifest)
    }

    /// Sanitize this manifest's [`Identity`] into the env-var
    /// exports that should be set on child Farmfile processes.
    /// Thin wrapper around [`sanitize_identity_for_env`].
    pub fn env_exports(
        &self,
        existing_env: impl Fn(&str) -> bool,
    ) -> ExportResult {
        sanitize_identity_for_env(&self.identity, existing_env)
    }
}

// ============================================================================
// Env-var export sanitizer
//
// This is the single code path that turns the typed Identity into
// child-process env vars. The rules are spelled out in the
// crate-level "Trust zones / defences" docs. Tests below lock
// them in — any change here MUST update tests.
// ============================================================================

/// Prefix prepended to every exported env var. Applied centrally;
/// callers MUST NOT bypass.
pub const ENV_PREFIX: &str = "FARM_";

/// Per-value byte cap. Values exceeding this are skipped with
/// [`SkipReason::ValueTooLong`]. Set well below `ARG_MAX` so a
/// handful of oversized values can't exhaust the env block.
pub const MAX_VALUE_LEN: usize = 4096;

/// Defensive ceiling on the number of vars sanitized in a single
/// call. With [`Identity`]'s 4 fields this is unreachable; kept
/// as a guard rail if the struct ever grows.
pub const MAX_EXPORTED_KEYS: usize = 32;

/// One env var prepared for export. `name` is already prefixed
/// (`FARM_*`) and uppercased.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvExport {
    /// Final env var name (always starts with [`ENV_PREFIX`]).
    pub name: String,
    /// Validated string value. Guaranteed: no `\0`, no `\n`, no
    /// `\r`, byte length <= [`MAX_VALUE_LEN`].
    pub value: String,
}

/// Why a candidate value was skipped. Callers SHOULD surface
/// these at warn-level so silent drops don't mask mis-configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// String value exceeded [`MAX_VALUE_LEN`] bytes. Inner value
    /// is the observed byte length.
    ValueTooLong(usize),
    /// Value contained a NUL byte. Env strings are NUL-terminated
    /// on POSIX, so the byte would truncate the value silently.
    ContainsNul,
    /// Value contained `\n` or `\r`. These survive `execve()` but
    /// can confuse shells with a non-default `IFS`.
    ContainsNewline,
    /// An env var with the target name was already set in the
    /// caller's environment. The sanitizer refuses to shadow it.
    AlreadyInEnv,
    /// Defensive: total exports reached [`MAX_EXPORTED_KEYS`]
    /// before this value could be processed.
    LimitReached,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::ValueTooLong(n) => write!(
                f,
                "value is {} bytes (max {})",
                n, MAX_VALUE_LEN
            ),
            SkipReason::ContainsNul => write!(f, "value contains NUL byte"),
            SkipReason::ContainsNewline => write!(f, "value contains newline"),
            SkipReason::AlreadyInEnv => {
                write!(f, "target env var already set, refusing to shadow")
            }
            SkipReason::LimitReached => {
                write!(f, "total exports cap reached ({})", MAX_EXPORTED_KEYS)
            }
        }
    }
}

/// Outcome of [`sanitize_identity_for_env`]: validated exports to
/// apply, plus structured reasons for any skipped values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportResult {
    /// Sanitized env vars. Safe to apply to a child process verbatim.
    pub exports: Vec<EnvExport>,
    /// Skipped candidates with the reason each was dropped.
    /// Tuple is `(identity_field_name, why)`. Logging these at
    /// warn-level is recommended.
    pub skipped: Vec<(&'static str, SkipReason)>,
}

/// Build the sanitized env-var export set from an [`Identity`].
///
/// `existing_env` is a callback returning `true` if a given env
/// var name is already set in the caller's environment. Names
/// returning `true` are skipped, not overwritten. In tests, pass
/// a closure returning `false`. In `farm`, pass
/// `|name| std::env::var_os(name).is_some()`.
///
/// Algorithm:
///
/// 1. For each `Identity` field in declared order, fetch the
///    `Option<String>`. If `None`, skip silently.
/// 2. Reject if value byte length exceeds [`MAX_VALUE_LEN`].
/// 3. Reject if value contains any of `\0`, `\n`, `\r`.
/// 4. Compute `name = ENV_PREFIX + field_name.to_uppercase()`.
/// 5. Reject if `existing_env(&name)` returns `true`.
/// 6. Push the export. Stop if [`MAX_EXPORTED_KEYS`] reached.
pub fn sanitize_identity_for_env(
    identity: &Identity,
    existing_env: impl Fn(&str) -> bool,
) -> ExportResult {
    // (static name, optional value). Order = declared order on Identity.
    let candidates: [(&'static str, &Option<String>); 4] = [
        ("pipeline_id", &identity.pipeline_id),
        ("pipeline_run_id", &identity.pipeline_run_id),
        ("job_id", &identity.job_id),
        ("stage", &identity.stage),
    ];

    let mut out = ExportResult::default();

    for (field_name, opt_value) in candidates {
        if out.exports.len() >= MAX_EXPORTED_KEYS {
            out.skipped.push((field_name, SkipReason::LimitReached));
            continue;
        }
        let Some(s) = opt_value else { continue };

        if s.len() > MAX_VALUE_LEN {
            out.skipped
                .push((field_name, SkipReason::ValueTooLong(s.len())));
            continue;
        }
        if s.contains('\0') {
            out.skipped.push((field_name, SkipReason::ContainsNul));
            continue;
        }
        if s.contains('\n') || s.contains('\r') {
            out.skipped.push((field_name, SkipReason::ContainsNewline));
            continue;
        }

        let name = format!("{}{}", ENV_PREFIX, field_name.to_uppercase());
        if existing_env(&name) {
            out.skipped.push((field_name, SkipReason::AlreadyInEnv));
            continue;
        }

        out.exports.push(EnvExport {
            name,
            value: s.clone(),
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_existing_env(_: &str) -> bool {
        false
    }

    // ---- Schema version & load policy -------------------------------------

    #[test]
    fn default_uses_current_version() {
        let m = ContextManifest::default();
        assert_eq!(m.schema_version, SCHEMA_VERSION);
        assert_eq!(m.identity, Identity::default());
    }

    #[test]
    fn roundtrip_typed_manifest() {
        // No external blob in the typed manifest — that data lives
        // in `external/*.json` sibling files, opaque to this crate.
        let original = ContextManifest {
            schema_version: SCHEMA_VERSION,
            identity: Identity {
                pipeline_id: Some("p123".into()),
                pipeline_run_id: Some("r456".into()),
                job_id: Some("j789".into()),
                stage: Some("build".into()),
            },
        };
        let json = serde_json::to_vec(&original).unwrap();
        let parsed = ContextManifest::from_json_slice(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn missing_identity_defaults_to_empty() {
        // Standalone-orchestrator case: no identity fields set.
        // Should parse and produce an Identity with all-None.
        let json = br#"{"schema_version":1,"identity":{}}"#;
        let parsed = ContextManifest::from_json_slice(json).unwrap();
        assert_eq!(parsed.identity, Identity::default());
    }

    #[test]
    fn missing_schema_version_rejected() {
        let json = br#"{"identity":{}}"#;
        let err = ContextManifest::from_json_slice(json).unwrap_err();
        assert!(matches!(err, ManifestError::MissingVersion));
    }

    #[test]
    fn zero_schema_version_rejected() {
        let json = br#"{"schema_version":0,"identity":{}}"#;
        let err = ContextManifest::from_json_slice(json).unwrap_err();
        assert!(matches!(err, ManifestError::MissingVersion));
    }

    #[test]
    fn future_schema_version_rejected() {
        let json = format!(
            r#"{{"schema_version":{},"identity":{{}}}}"#,
            SCHEMA_VERSION + 1
        );
        let err = ContextManifest::from_json_slice(json.as_bytes()).unwrap_err();
        match err {
            ManifestError::UnsupportedVersion {
                manifest_version,
                reader_version,
            } => {
                assert_eq!(manifest_version, SCHEMA_VERSION + 1);
                assert_eq!(reader_version, SCHEMA_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {:?}", other),
        }
    }

    #[test]
    fn unknown_top_level_field_rejected() {
        // deny_unknown_fields at the manifest level — orchestrator
        // accidentally writes a field this version doesn't know,
        // we hard-fail rather than silently drop.
        let json = br#"{"schema_version":1,"identity":{},"surprise":"hi"}"#;
        let err = ContextManifest::from_json_slice(json).unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    #[test]
    fn unknown_identity_field_rejected() {
        // deny_unknown_fields on Identity — guard against typos and
        // would-be injection via novel keys.
        let json = br#"{
            "schema_version": 1,
            "identity": {"pipeline_id": "p1", "rogue_field": "x"}
        }"#;
        let err = ContextManifest::from_json_slice(json).unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    #[test]
    fn non_string_identity_value_rejected() {
        // Type-pinned at parse time: number-shaped pipeline_id fails.
        let json = br#"{"schema_version":1,"identity":{"pipeline_id":42}}"#;
        let err = ContextManifest::from_json_slice(json).unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    #[test]
    fn too_large_manifest_rejected() {
        // Pad with a long identity value to exceed the cap. The
        // typed manifest has no opaque field to stuff bytes into,
        // so we use a single oversized identity string. Won't be
        // a realistic manifest but exercises the size-cap check
        // before any further parsing happens.
        let big = "x".repeat(MAX_MANIFEST_BYTES);
        let mut bytes = Vec::with_capacity(MAX_MANIFEST_BYTES + 100);
        bytes.extend_from_slice(b"{\"schema_version\":1,\"identity\":{\"pipeline_id\":\"");
        bytes.extend_from_slice(big.as_bytes());
        bytes.extend_from_slice(b"\"}}");
        let err = ContextManifest::from_json_slice(&bytes).unwrap_err();
        match err {
            ManifestError::TooLarge { observed, cap } => {
                assert!(observed > cap);
                assert_eq!(cap, MAX_MANIFEST_BYTES);
            }
            other => panic!("expected TooLarge, got {:?}", other),
        }
    }

    #[test]
    fn manifest_rejects_external_field() {
        // External data does NOT live in context.json (it lives in
        // `external/*.json` sibling files). Any orchestrator that
        // tries to embed it here should hard-fail via the
        // `deny_unknown_fields` enforcement.
        let json = br#"{"schema_version":1,"identity":{},"external":{"x":1}}"#;
        let err = ContextManifest::from_json_slice(json).unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    // ---- Sanitizer: identity → env vars -----------------------------------

    #[test]
    fn happy_path_all_identity_fields_exported_and_prefixed() {
        let identity = Identity {
            pipeline_id: Some("p123".into()),
            pipeline_run_id: Some("r456".into()),
            job_id: Some("j789".into()),
            stage: Some("build".into()),
        };
        let result = sanitize_identity_for_env(&identity, no_existing_env);
        assert!(result.skipped.is_empty(), "{:?}", result.skipped);
        assert_eq!(result.exports.len(), 4);
        assert_eq!(result.exports[0].name, "FARM_PIPELINE_ID");
        assert_eq!(result.exports[1].name, "FARM_PIPELINE_RUN_ID");
        assert_eq!(result.exports[2].name, "FARM_JOB_ID");
        assert_eq!(result.exports[3].name, "FARM_STAGE");
        for ex in &result.exports {
            assert!(ex.name.starts_with(ENV_PREFIX));
        }
    }

    #[test]
    fn unset_identity_fields_skipped_silently() {
        // A partially-set Identity is legit — not all
        // orchestrations have a pipeline (e.g. ad-hoc `farm` runs).
        let identity = Identity {
            pipeline_id: None,
            pipeline_run_id: None,
            job_id: None,
            stage: Some("build".into()),
        };
        let result = sanitize_identity_for_env(&identity, no_existing_env);
        assert_eq!(result.exports.len(), 1);
        assert_eq!(result.exports[0].name, "FARM_STAGE");
        assert!(result.skipped.is_empty());
    }

    #[test]
    fn nul_byte_rejected() {
        let identity = Identity {
            pipeline_id: Some("p123\0LD_PRELOAD=/evil.so".into()),
            stage: Some("ok".into()),
            ..Identity::default()
        };
        let result = sanitize_identity_for_env(&identity, no_existing_env);
        assert_eq!(result.exports.len(), 1);
        assert_eq!(result.exports[0].name, "FARM_STAGE");
        assert_eq!(result.skipped[0], ("pipeline_id", SkipReason::ContainsNul));
    }

    #[test]
    fn newline_rejected() {
        let identity = Identity {
            pipeline_id: Some("p\nrm -rf /".into()),
            ..Identity::default()
        };
        let result = sanitize_identity_for_env(&identity, no_existing_env);
        assert!(result.exports.is_empty());
        assert_eq!(result.skipped[0].1, SkipReason::ContainsNewline);
    }

    #[test]
    fn carriage_return_rejected() {
        let identity = Identity {
            pipeline_id: Some("p\rsneaky".into()),
            ..Identity::default()
        };
        let result = sanitize_identity_for_env(&identity, no_existing_env);
        assert!(result.exports.is_empty());
        assert_eq!(result.skipped[0].1, SkipReason::ContainsNewline);
    }

    #[test]
    fn too_long_value_rejected() {
        let big = "x".repeat(MAX_VALUE_LEN + 1);
        let identity = Identity {
            pipeline_id: Some(big),
            stage: Some("ok".into()),
            ..Identity::default()
        };
        let result = sanitize_identity_for_env(&identity, no_existing_env);
        assert_eq!(result.exports.len(), 1);
        match result.skipped[0].1 {
            SkipReason::ValueTooLong(n) => assert_eq!(n, MAX_VALUE_LEN + 1),
            ref other => panic!("expected ValueTooLong, got {:?}", other),
        }
    }

    #[test]
    fn value_at_exact_cap_accepted() {
        let exactly = "x".repeat(MAX_VALUE_LEN);
        let identity = Identity {
            pipeline_id: Some(exactly),
            ..Identity::default()
        };
        let result = sanitize_identity_for_env(&identity, no_existing_env);
        assert_eq!(result.exports.len(), 1);
        assert!(result.skipped.is_empty());
    }

    #[test]
    fn already_in_env_skipped_not_overwritten() {
        let identity = Identity {
            pipeline_id: Some("from-manifest".into()),
            stage: Some("build".into()),
            ..Identity::default()
        };
        let result = sanitize_identity_for_env(&identity, |name| {
            name == "FARM_PIPELINE_ID"
        });
        assert_eq!(result.exports.len(), 1);
        assert_eq!(result.exports[0].name, "FARM_STAGE");
        assert_eq!(result.skipped[0], ("pipeline_id", SkipReason::AlreadyInEnv));
    }

    #[test]
    fn manifest_env_exports_convenience() {
        let m = ContextManifest {
            schema_version: SCHEMA_VERSION,
            identity: Identity {
                stage: Some("deploy".into()),
                ..Identity::default()
            },
        };
        let result = m.env_exports(no_existing_env);
        assert_eq!(result.exports.len(), 1);
        assert_eq!(result.exports[0].name, "FARM_STAGE");
        assert_eq!(result.exports[0].value, "deploy");
    }
}

//! `proptest` property suite for `Config` TOML load / serialize.
//!
//! Config is loaded from an on-disk TOML file the user (or a migration) may have
//! hand-edited, so the loader is an UNTRUSTED-INPUT boundary: it must never panic
//! on garbage, must round-trip its own serialized output, and must accept partial
//! configs by filling in defaults (`#[serde(default)]` on `Config`).

use c0pl4nd_core::config::Config;
use proptest::prelude::*;
use std::path::Path;

fn p() -> &'static Path {
    Path::new("config.toml")
}

/// The serialize→parse round-trip: `Config::default().to_toml()` must parse back
/// to an equal `Config`. This pins that every field round-trips through TOML
/// (a field that fails to serialize or deserialize breaks settings persistence).
#[test]
fn default_config_toml_round_trips() {
    let cfg = Config::default();
    let toml_text = cfg.to_toml().expect("default config serializes");
    let parsed = Config::from_toml(&toml_text, p()).expect("default config re-parses");
    assert_eq!(cfg, parsed, "Config did not round-trip through TOML");
}

proptest! {
    /// ARBITRARY BYTES as TOML: an arbitrary byte vector interpreted as a config
    /// file must never panic the loader. It either parses (yielding a valid
    /// config) or returns a typed `ConfigError` — never a crash. This is the
    /// untrusted-input totality guarantee.
    #[test]
    fn arbitrary_bytes_never_panic_the_loader(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(src) = std::str::from_utf8(&bytes) {
            // The result is intentionally ignored — the property is "does not panic".
            let _ = Config::from_toml(src, p());
        }
    }

    /// ARBITRARY KEY/VALUE TOML: well-formed TOML with unknown or partial keys
    /// must never panic — unknown keys are ignored, missing keys fall back to
    /// defaults. A valid parse always yields a config that PASSES validation
    /// (because `from_toml` runs `validate()` internally and only returns Ok on
    /// success).
    #[test]
    fn partial_toml_yields_validated_or_typed_error(
        keys in prop::collection::vec("[a-z_]{1,12}", 0..6),
        vals in prop::collection::vec(0i64..1000, 0..6),
    ) {
        let mut src = String::new();
        for (k, v) in keys.iter().zip(vals.iter()) {
            src.push_str(&format!("{k} = {v}\n"));
        }
        // Either a valid config (already validated by from_toml) or a typed error.
        if let Ok(cfg) = Config::from_toml(&src, p()) {
            // A successfully-loaded config must itself re-validate.
            prop_assert!(cfg.validate().is_ok());
        }
    }

    /// SCROLLBACK / OPACITY FUZZ over real config keys: injecting arbitrary
    /// values for the numeric knobs either parses-and-validates or returns a
    /// typed error — never panics. Exercises the `validate()` range checks
    /// (opacity ∈ [0,1], font.size > 0, etc.).
    #[test]
    fn numeric_knob_fuzz_never_panics(
        scrollback in 0u64..1_000_000,
        opacity in -2.0f64..3.0,
    ) {
        let src = format!(
            "scrollback_lines = {scrollback}\nopacity = {opacity}\n"
        );
        let _ = Config::from_toml(&src, p());
    }
}

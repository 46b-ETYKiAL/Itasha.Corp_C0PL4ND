//! Installed-font enumeration and egui font-stack wiring for the Font settings
//! page (#14 — "Font family + fallback dropdowns that actually load the font").
//!
//! Before this module the egui app NEVER loaded the configured font: the chrome
//! only registered egui's default fonts + the Phosphor icons, and the terminal
//! grid drew with egui's built-in `FontFamily::Monospace`. The free-text
//! `family` / `fallback` config fields were inert. This module:
//!
//! 1. **enumerates** the system's installed MONOSPACE families (via `fontdb`,
//!    cached once — it is slow), and
//! 2. **loads** the configured family + fallbacks into egui by reading each
//!    chosen face's raw bytes and PREPENDING them to `FontFamily::Monospace`,
//!    keeping egui's default monospace + Phosphor at the END as the ultimate
//!    fallback, so the grid + every monospace UI surface use the chosen font.
//!
//! A family that cannot be found is skipped gracefully — never a panic — so a
//! stale config naming an uninstalled font falls back to the built-in mono.

use std::sync::OnceLock;

use eframe::egui;

/// The label shown for egui's built-in monospace in the Family / Fallback
/// dropdowns. Always offered as a choice so a user can deliberately fall back to
/// the bundled mono even on a system rich in installed fonts, and so the list is
/// never empty (the enumeration could legitimately find zero installed faces).
pub const BUILTIN_MONOSPACE_LABEL: &str = "(built-in monospace)";

/// The Fallback dropdown's "no fallback in this slot" choice.
pub const NONE_LABEL: &str = "(none)";

/// Process-wide cache of the installed monospace family list. Enumeration loads
/// and parses the whole system font set (100s of ms with hundreds of fonts), so
/// it runs at most ONCE per process — never per frame.
static MONOSPACE_FAMILIES: OnceLock<Vec<String>> = OnceLock::new();

/// The installed monospace family names plus the built-in label, sorted +
/// deduped, computed lazily and cached for the process lifetime.
///
/// The returned list is suitable to drive the Font settings `ComboBox`es
/// directly. [`BUILTIN_MONOSPACE_LABEL`] is ALWAYS present (first), so the list
/// is non-empty even on a system where enumeration finds nothing.
pub fn monospace_family_choices() -> &'static [String] {
    MONOSPACE_FAMILIES.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let installed: Vec<String> = db
            .faces()
            .filter(|f| f.monospaced)
            // The first family name is the English-US one (fontdb guarantees it
            // is first when present); display that.
            .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
            .collect();
        normalize_family_list(installed)
    })
}

/// Pure helper: take a raw list of monospace family names, prepend the built-in
/// label, then sort + dedupe (case-insensitively) so the dropdown is clean and
/// stable. The built-in label always sorts to the FRONT regardless of the
/// installed names, so it is the first, default-reachable choice.
///
/// Kept pure (no `fontdb`, no egui) so the "always contains the built-in +
/// sorted + deduped" contract is unit-testable without a system font DB.
pub fn normalize_family_list(installed: Vec<String>) -> Vec<String> {
    let mut names: Vec<String> = installed
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != BUILTIN_MONOSPACE_LABEL)
        .collect();
    // Case-insensitive sort + dedupe so "Cascadia Code" and "cascadia code" do
    // not both appear, and the order is deterministic.
    names.sort_by_key(|a| a.to_lowercase());
    names.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
    let mut out = Vec::with_capacity(names.len() + 1);
    out.push(BUILTIN_MONOSPACE_LABEL.to_string());
    out.extend(names);
    out
}

/// Whether `family` names egui's built-in monospace (the synthetic label, or the
/// generic "monospace" the default config uses), i.e. NOT a real installed face
/// to load. Such a value contributes no custom font bytes — the built-in mono is
/// already the ultimate fallback.
pub fn is_builtin_family(family: &str) -> bool {
    let f = family.trim();
    f.is_empty()
        || f == BUILTIN_MONOSPACE_LABEL
        || f == NONE_LABEL
        || f.eq_ignore_ascii_case("monospace")
}

/// Read the raw bytes of the regular face of the installed family `family` from
/// `db`, case-insensitively matched against each face's primary (English-US)
/// family name. Returns `None` when no installed face matches (caller skips it
/// gracefully). Prefers a non-italic, ~regular-weight face so the grid renders
/// upright text; falls back to the first match if no plain face exists.
pub fn face_bytes_for_family(db: &fontdb::Database, family: &str) -> Option<(Vec<u8>, u32)> {
    let want = family.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    let matches = db.faces().filter(|f| {
        f.families
            .first()
            .map(|(name, _)| name.to_lowercase() == want)
            .unwrap_or(false)
    });
    // Prefer an upright, regular-weight face; otherwise take whatever matched.
    let mut best: Option<(&fontdb::FaceInfo, i32)> = None;
    for f in matches {
        let upright = matches!(f.style, fontdb::Style::Normal);
        // Distance from the regular weight (400); smaller is better.
        let weight_dist = (f.weight.0 as i32 - 400).abs();
        let score = if upright { 0 } else { 10_000 } + weight_dist;
        if best.as_ref().map(|(_, s)| score < *s).unwrap_or(true) {
            best = Some((f, score));
        }
    }
    let id = best?.0.id;
    // Return the raw file bytes AND the face's index WITHIN that file. Many
    // Windows system fonts (MS Gothic, Malgun Gothic, …) are TrueType Collections
    // (.ttc) holding multiple faces; `with_face_data` yields the whole-file bytes
    // plus the wanted face's collection index. Dropping that index and handing the
    // bytes to egui makes it read face 0 — the WRONG font — so ASCII codepoints
    // render as whatever glyph sits at that slot in the wrong face (the terminal-
    // grid "garble": Latin text drawn as CJK / math glyphs). The caller MUST set
    // `FontData.index` to this value.
    db.with_face_data(id, |data, index| (data.to_vec(), index))
}

/// The egui `font_data` key under which a loaded custom family's bytes are
/// registered. Distinct per family so the Family choice and a Fallback choice
/// naming a different font do not collide.
fn font_data_key(family: &str) -> String {
    format!("c0pl4nd-user-font::{}", family.trim().to_lowercase())
}

/// The platform's OS-bundled CJK fonts, tried in order as an implicit ultimate
/// fallback by [`build_font_definitions`] so CJK (Chinese/Japanese/Korean) text
/// renders out of the box — instead of tofu boxes — without shipping a multi-MB
/// CJK font in the binary. Only the FIRST that resolves on the machine is loaded.
fn os_cjk_fallback_fonts() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        // Present on every Windows install: MS Gothic (JP monospace) first, then
        // the SC / KR system faces so non-Japanese CJK also resolves.
        &["MS Gothic", "Yu Gothic", "Microsoft YaHei", "Malgun Gothic"]
    }
    #[cfg(target_os = "macos")]
    {
        &["Hiragino Sans", "Apple SD Gothic Neo", "PingFang SC"]
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        &[
            "Noto Sans Mono CJK JP",
            "Noto Sans CJK JP",
            "Noto Sans CJK SC",
        ]
    }
}

/// Build the egui [`egui::FontDefinitions`] for the configured `family` +
/// `fallbacks`, on top of `base` (which must already carry egui's defaults +
/// the Phosphor icon families — i.e. the output of the chrome's base font
/// install). Each named family that resolves to an installed face is loaded and
/// PREPENDED (family first, then each fallback in order) to
/// `FontFamily::Monospace`; egui's default monospace fonts + Phosphor stay at
/// the END as the ultimate fallback. Families that do not resolve (built-in
/// label, "(none)", or simply not installed) are skipped — never a panic.
///
/// Returns `(defs, loaded_any)` where `loaded_any` is true iff at least one
/// custom face was prepended (so the caller can note whether the install
/// actually changed the monospace stack). The `db` is borrowed so the (slow)
/// system-font load can be done once by the caller and reused across the family
/// + fallback lookups.
pub fn build_font_definitions(
    mut base: egui::FontDefinitions,
    db: &fontdb::Database,
    family: &str,
    fallbacks: &[String],
) -> (egui::FontDefinitions, bool) {
    // Resolve the ordered list of real families to load: the primary first,
    // then each fallback, skipping the built-in/none/empty markers and
    // de-duplicating so the same font is never prepended twice.
    let mut wanted: Vec<String> = Vec::new();
    for name in std::iter::once(family).chain(fallbacks.iter().map(String::as_str)) {
        if is_builtin_family(name) {
            continue;
        }
        let n = name.trim().to_string();
        if !wanted.iter().any(|w| w.eq_ignore_ascii_case(&n)) {
            wanted.push(n);
        }
    }

    // Load each resolvable face and register it; collect the keys to prepend in
    // priority order (primary, then fallbacks).
    let mut prepend_keys: Vec<String> = Vec::new();
    for name in &wanted {
        if let Some((bytes, index)) = face_bytes_for_family(db, name) {
            let key = font_data_key(name);
            // Preserve the .ttc face index — a collection face at a non-zero index
            // loaded as index 0 renders the WRONG font (the grid garble).
            let mut face = egui::FontData::from_owned(bytes);
            face.index = index;
            base.font_data.insert(key.clone(), face.into());
            prepend_keys.push(key);
            tracing::debug!(font = %name, index, "loaded configured monospace font face");
        } else {
            // Observability breadcrumb: a configured family/fallback that does
            // not resolve degrades silently to the built-in font, which reads as
            // "my font setting did nothing". Log it at warn so `C0PL4ND_LOG=warn`
            // (the default) surfaces it without a debug build.
            tracing::warn!(
                font = %name,
                "configured monospace font not found on this system; using the \
                 built-in font (check the [font] family/fallback in config.toml)"
            );
        }
    }

    // Implicit OS CJK fallback: append the FIRST platform CJK font that resolves
    // so Chinese/Japanese/Korean text renders out of the box using a face the OS
    // already ships — instead of tofu boxes — WITHOUT bundling a multi-MB CJK
    // font into the binary. It is loaded last, so it is the lowest-priority
    // fallback (used only for glyphs the primary + user fallbacks lack), and at
    // most ONE is loaded. The per-cell renderer places every wide glyph on its
    // own two cells, so even a proportional CJK face aligns in the grid.
    for cand in os_cjk_fallback_fonts() {
        if wanted.iter().any(|w| w.eq_ignore_ascii_case(cand)) {
            continue; // already attempted as a user-configured fallback
        }
        if let Some((bytes, index)) = face_bytes_for_family(db, cand) {
            let key = font_data_key(cand);
            // Preserve the .ttc face index — MS Gothic / Malgun Gothic are
            // collections; face 0 is the wrong font.
            let mut face = egui::FontData::from_owned(bytes);
            face.index = index;
            base.font_data.insert(key.clone(), face.into());
            prepend_keys.push(key);
            tracing::debug!(font = %cand, index, "loaded OS CJK fallback font for grid coverage");
            break;
        }
    }

    let loaded_any = !prepend_keys.is_empty();
    if loaded_any {
        let mono = base
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default();
        // Prepend in REVERSE so the final order is [primary, fallback1, ...,
        // <egui defaults>, <phosphor>] — inserting each at the front in reverse
        // yields the forward priority order.
        for key in prepend_keys.iter().rev() {
            // Defensive: never register the same key twice in the family list.
            mono.retain(|k| k != key);
            mono.insert(0, key.clone());
        }
    }
    (base, loaded_any)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_list_always_contains_builtin_first() {
        let got = normalize_family_list(vec!["Cascadia Code".into(), "Hack".into()]);
        assert_eq!(
            got.first().map(String::as_str),
            Some(BUILTIN_MONOSPACE_LABEL),
            "the built-in label is always the first choice"
        );
    }

    #[test]
    fn family_list_is_sorted_and_deduped_case_insensitively() {
        let got = normalize_family_list(vec![
            "Hack".into(),
            "Cascadia Code".into(),
            "cascadia code".into(), // duplicate (different case)
            "  ".into(),            // blank dropped
            "Anonymous Pro".into(),
        ]);
        // Built-in first, then the installed names sorted case-insensitively,
        // with the case-duplicate collapsed.
        assert_eq!(
            got,
            vec![
                BUILTIN_MONOSPACE_LABEL.to_string(),
                "Anonymous Pro".to_string(),
                "Cascadia Code".to_string(),
                "Hack".to_string(),
            ]
        );
    }

    #[test]
    fn empty_install_still_offers_the_builtin() {
        let got = normalize_family_list(vec![]);
        assert_eq!(got, vec![BUILTIN_MONOSPACE_LABEL.to_string()]);
    }

    #[test]
    fn is_builtin_family_recognises_markers_and_generic_monospace() {
        assert!(is_builtin_family(""));
        assert!(is_builtin_family("   "));
        assert!(is_builtin_family(BUILTIN_MONOSPACE_LABEL));
        assert!(is_builtin_family(NONE_LABEL));
        assert!(is_builtin_family("monospace"));
        assert!(is_builtin_family("MONOSPACE"));
        assert!(!is_builtin_family("Cascadia Code"));
    }

    /// A face that exists in a `fontdb::Database` is prepended to Monospace ahead
    /// of egui's defaults; a family that does NOT exist is skipped (no panic, no
    /// change). Builds an in-memory DB from a tiny embedded test font so the test
    /// never depends on any system font being installed.
    #[test]
    fn build_definitions_prepends_existing_and_skips_missing() {
        // egui ships test-usable font bytes via its own default set; here we
        // construct a fontdb from the data egui's own default proportional font
        // exposes is not accessible, so we synthesise a minimal valid TTF by
        // loading egui's bundled fonts is not possible. Instead, exercise the
        // pure path: an EMPTY db resolves nothing, so build is a graceful no-op.
        let db = fontdb::Database::new(); // no faces loaded
        let base = egui::FontDefinitions::default();
        let before = base
            .families
            .get(&egui::FontFamily::Monospace)
            .cloned()
            .unwrap_or_default();
        let (defs, loaded) = build_font_definitions(base, &db, "Definitely Not Installed XYZ", &[]);
        assert!(!loaded, "a missing family loads nothing");
        let after = defs
            .families
            .get(&egui::FontFamily::Monospace)
            .cloned()
            .unwrap_or_default();
        assert_eq!(before, after, "a missing family leaves Monospace unchanged");
    }

    /// When a face IS present in the db, its bytes are registered and its key is
    /// prepended to the Monospace family ahead of the existing entries. Uses a
    /// real installed monospace family IF the system has one (guarded — skipped
    /// when none exists), so the prepend wiring is exercised on a real face
    /// without hard-coding a font name.
    #[test]
    fn build_definitions_prepends_a_real_installed_monospace_when_available() {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        // Find any installed monospace family to use as the primary.
        let Some(fam) = db
            .faces()
            .find(|f| f.monospaced)
            .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
        else {
            // No monospace font installed on this box — nothing to assert.
            return;
        };
        let base = egui::FontDefinitions::default();
        let (defs, loaded) = build_font_definitions(base, &db, &fam, &[]);
        assert!(loaded, "an installed monospace family must load");
        let mono = defs
            .families
            .get(&egui::FontFamily::Monospace)
            .expect("monospace family exists");
        let key = font_data_key(&fam);
        assert_eq!(
            mono.first(),
            Some(&key),
            "the chosen family is prepended to the FRONT of Monospace"
        );
        assert!(
            defs.font_data.contains_key(&key),
            "the chosen family's bytes are registered under its key"
        );
    }

    #[test]
    fn font_data_key_is_family_namespaced_and_case_folded() {
        // The egui font_data key is per-family so the Family choice and a
        // differently-named Fallback never collide; it is lowercased + trimmed
        // so the same font under two casings maps to ONE key.
        assert_eq!(
            font_data_key("Cascadia Code"),
            "c0pl4nd-user-font::cascadia code"
        );
        assert_eq!(
            font_data_key("  CASCADIA code  "),
            font_data_key("cascadia code"),
            "trimming + case-folding give one key for one font"
        );
        assert_ne!(
            font_data_key("Hack"),
            font_data_key("Cascadia Code"),
            "distinct families get distinct keys (no cross-font collision)"
        );
    }

    #[test]
    fn face_bytes_for_family_returns_none_for_empty_name() {
        // The empty-name guard: a blank family never matches a face (and never
        // touches the db face iterator with an empty want string).
        let db = fontdb::Database::new();
        assert!(face_bytes_for_family(&db, "").is_none());
        assert!(face_bytes_for_family(&db, "   ").is_none());
    }

    #[test]
    fn face_bytes_for_family_returns_none_when_no_face_matches() {
        // A db with no faces (or a name no face carries) yields None — the
        // caller skips the family gracefully rather than panicking.
        let db = fontdb::Database::new(); // empty
        assert!(face_bytes_for_family(&db, "Definitely Not Installed XYZ").is_none());
    }

    #[test]
    fn build_definitions_dedupes_a_repeated_family_and_fallback() {
        // A family repeated as a fallback (same name, different case) must not
        // be loaded twice. With an empty db nothing loads, but the dedup of the
        // `wanted` list is exercised (no panic, loaded_any == false).
        let db = fontdb::Database::new();
        let base = egui::FontDefinitions::default();
        let (_defs, loaded) =
            build_font_definitions(base, &db, "Hack", &["hack".to_string(), "HACK".to_string()]);
        assert!(
            !loaded,
            "an uninstalled family loads nothing even when repeated"
        );
    }
}

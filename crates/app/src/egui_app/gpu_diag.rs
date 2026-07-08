//! GPU / surface transparency diagnostics + transparency-aware adapter selection.
//!
//! # The bug this fixes
//!
//! On a hybrid-GPU (Optimus) laptop — NVIDIA discrete + Intel integrated, Windows 11
//! — a translucent C0PL4ND window rendered OPAQUE BLACK, while the sibling app
//! SCR1B3 (identical eframe 0.34 / egui-wgpu 0.34.3 / wgpu 29 stack) is see-through
//! on the SAME machine. The `gpu-diag.log` instrumentation below produced the
//! smoking gun:
//!
//! * **Intel Iris Xe (integrated)** — `alpha_modes = [Opaque, Inherit]`. This iGPU
//!   physically DRIVES THE DISPLAY, so DWM composites its output directly; this is
//!   the adapter that can actually show the desktop through the window.
//! * **NVIDIA RTX 3080 Ti (discrete)** — `alpha_modes = [Opaque, PreMultiplied]`.
//!   Transparent-capable on paper, BUT on an Optimus laptop the discrete GPU renders
//!   OFF-SCREEN and the copy to the Intel-driven display is OPAQUE, so its
//!   transparency never reaches the screen.
//!
//! The first fix wrongly preferred the adapter advertising `PreMultiplied` (the
//! discrete GPU) → still black. The REDIRECT (this module): for a translucent
//! window, prefer the INTEGRATED / display-driving adapter — exactly what SCR1B3
//! gets via `PowerPreference::LowPower`. The discrete GPU is never chosen for a
//! see-through window even though it looks transparent-capable, because the Optimus
//! display path opaques it.
//!
//! # What this module does
//!
//! 1. **Instrumentation** — [`log_line`] appends to `<config_dir>/gpu-diag.log`
//!    (the release binary is `windows_subsystem = "windows"`, so stderr/tracing is
//!    lost; a real file is the only way the user can hand back the evidence). Every
//!    candidate adapter, its surface `alpha_modes`, the mode egui-wgpu WILL
//!    configure ([`egui_wgpu_configured_mode`]), and the final pick are logged.
//! 2. **Deterministic adapter selection** — [`choose_display_driving_adapter`]
//!    prefers the integrated / display-driving GPU (with a tie-break toward the
//!    richest transparent-composite capability), matching SCR1B3's `LowPower`
//!    intent. Pure + unit-tested here; the wgpu-touching glue lives in
//!    `egui_main.rs`.

use std::path::PathBuf;

use eframe::wgpu::{CompositeAlphaMode, DeviceType};

/// The on-disk diagnostics log path: `<config_dir>/gpu-diag.log`, next to
/// `config.toml`. `None` when the per-user config dir cannot be resolved.
pub(crate) fn diag_log_path() -> Option<PathBuf> {
    c0pl4nd_core::Config::config_dir().map(|d| d.join("gpu-diag.log"))
}

/// Append one line (with a seconds-since-epoch prefix) to the GPU diagnostics log.
/// Best-effort: a missing dir or an I/O error is swallowed — diagnostics must never
/// break launch. Creates the parent dir if needed.
pub(crate) fn log_line(msg: &str) {
    let Some(path) = diag_log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write as _;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

/// Truncate the diagnostics log and write a fresh session header, so each launch's
/// data starts clean (the file does not grow unboundedly across launches). Called
/// once at the very start of GPU init. Best-effort.
pub(crate) fn begin_session(header: &str) {
    let Some(path) = diag_log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("=== C0PL4ND gpu-diag :: {header} ===\n"));
}

/// Human-readable name for a wgpu [`CompositeAlphaMode`] (stable across wgpu
/// versions where the enum variants may otherwise `Debug`-print differently).
pub(crate) fn alpha_mode_name(m: CompositeAlphaMode) -> &'static str {
    match m {
        CompositeAlphaMode::Auto => "Auto",
        CompositeAlphaMode::Opaque => "Opaque",
        CompositeAlphaMode::PreMultiplied => "PreMultiplied",
        CompositeAlphaMode::PostMultiplied => "PostMultiplied",
        CompositeAlphaMode::Inherit => "Inherit",
    }
}

/// Human-readable name for a wgpu [`DeviceType`].
pub(crate) fn device_type_name(t: DeviceType) -> &'static str {
    match t {
        DeviceType::Other => "Other",
        DeviceType::IntegratedGpu => "IntegratedGpu",
        DeviceType::DiscreteGpu => "DiscreteGpu",
        DeviceType::VirtualGpu => "VirtualGpu",
        DeviceType::Cpu => "Cpu",
    }
}

/// The [`CompositeAlphaMode`] egui-wgpu 0.34.3 will CONFIGURE for a transparent
/// window given a surface's supported `alpha_modes` — reproduced here so the log
/// records what the swapchain actually ends up with (egui-wgpu itself only logs
/// this at `warn`, which the release GUI build discards).
///
/// egui-wgpu picks `PreMultiplied > PostMultiplied > Auto`; wgpu-core then resolves
/// `Auto` via the fallback list `[Opaque, Inherit]` (first supported wins). So a
/// surface that offers only `[Opaque, Inherit]` ends up **Opaque** (no transparency)
/// — which is exactly why the Intel Vulkan surface was black and why the fix is to
/// land on a surface/adapter path that composites, not merely one that lists a
/// transparent mode.
pub(crate) fn egui_wgpu_configured_mode(modes: &[CompositeAlphaMode]) -> CompositeAlphaMode {
    if modes.contains(&CompositeAlphaMode::PreMultiplied) {
        CompositeAlphaMode::PreMultiplied
    } else if modes.contains(&CompositeAlphaMode::PostMultiplied) {
        CompositeAlphaMode::PostMultiplied
    } else if modes.contains(&CompositeAlphaMode::Opaque) {
        // egui-wgpu passes `Auto`; wgpu-core's Auto fallback is `[Opaque, Inherit]`,
        // Opaque first.
        CompositeAlphaMode::Opaque
    } else if modes.contains(&CompositeAlphaMode::Inherit) {
        CompositeAlphaMode::Inherit
    } else {
        CompositeAlphaMode::Auto
    }
}

/// Whether a set of supported alpha modes includes a *transparent* one that
/// egui-wgpu will actually configure (`PreMultiplied`/`PostMultiplied`).
pub(crate) fn supports_transparency(modes: &[CompositeAlphaMode]) -> bool {
    modes.contains(&CompositeAlphaMode::PreMultiplied)
        || modes.contains(&CompositeAlphaMode::PostMultiplied)
}

/// Minimal per-adapter metadata — the pure inputs to
/// [`choose_display_driving_adapter`], free of wgpu handles so the selection policy
/// is unit-testable without a GPU.
#[derive(Debug, Clone)]
pub(crate) struct AdapterMeta {
    pub device_type: DeviceType,
    pub premultiplied: bool,
    pub postmultiplied: bool,
    pub inherit: bool,
}

/// Selection score for one adapter when a SEE-THROUGH window is requested.
///
/// The dominant term is the DISPLAY-DRIVING preference: on an Optimus laptop only
/// the integrated GPU (which owns the display outputs) can present a composited
/// see-through window; the discrete GPU renders off-screen and its result is copied
/// back OPAQUE, so it must NOT be chosen for transparency even though it advertises
/// `PreMultiplied`. Integrated is therefore weighted far above the transparent-mode
/// bonus, so a transparent-capable DISCRETE adapter never outranks the integrated
/// one. The smaller transparent-capability term only tie-breaks among adapters of
/// the same device class (e.g. the same iGPU exposed on two backends). Pure.
fn transparency_score(a: &AdapterMeta) -> i32 {
    let device_term = match a.device_type {
        // The display-driving class — the only one that composites on Optimus.
        DeviceType::IntegratedGpu => 1000,
        // Unknown / virtual: better than the off-screen discrete GPU, worse than iGPU.
        DeviceType::Other | DeviceType::VirtualGpu => 500,
        // Discrete: last resort (off-screen → opaque copy on a hybrid laptop). Still
        // beats a CPU adapter so a single-dGPU desktop keeps working.
        DeviceType::DiscreteGpu => 100,
        DeviceType::Cpu => 0,
    };
    // Tie-break toward the richest transparent-composite capability so, among
    // adapters of the SAME class, we prefer one egui-wgpu can make see-through.
    let alpha_term = if a.premultiplied {
        30
    } else if a.postmultiplied {
        20
    } else if a.inherit {
        10
    } else {
        0
    };
    device_term + alpha_term
}

/// Choose the adapter index for a see-through window: the highest
/// [`transparency_score`] (integrated / display-driving GPU first, then richest
/// transparent capability), stable on ties (first adapter wins). Returns `None`
/// only for an empty list.
pub(crate) fn choose_display_driving_adapter(adapters: &[AdapterMeta]) -> Option<usize> {
    adapters
        .iter()
        .enumerate()
        // `max_by_key` keeps the LAST max on ties; encode a descending index so the
        // FIRST adapter wins a true tie.
        .max_by_key(|(i, a)| (transparency_score(a), usize::MAX - *i))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(t: DeviceType, pre: bool, post: bool, inherit: bool) -> AdapterMeta {
        AdapterMeta {
            device_type: t,
            premultiplied: pre,
            postmultiplied: post,
            inherit,
        }
    }

    #[test]
    fn prefers_integrated_display_driver_over_transparent_capable_discrete() {
        // The exact Optimus laptop case: discrete advertises PreMultiplied but is
        // opaqued by the off-screen copy; integrated only has [Opaque, Inherit] yet
        // is the one that actually composites. Integrated MUST win.
        let adapters = [
            meta(DeviceType::DiscreteGpu, true, false, false), // NVIDIA — PreMultiplied
            meta(DeviceType::IntegratedGpu, false, false, true), // Intel — Inherit only
        ];
        assert_eq!(choose_display_driving_adapter(&adapters), Some(1));
    }

    #[test]
    fn among_same_class_prefers_richest_transparent_capability() {
        // Same iGPU exposed on two backends: prefer the one egui-wgpu can configure
        // transparent (PreMultiplied) over the Inherit-only one.
        let adapters = [
            meta(DeviceType::IntegratedGpu, false, false, true), // Inherit only
            meta(DeviceType::IntegratedGpu, true, false, false), // PreMultiplied
        ];
        assert_eq!(choose_display_driving_adapter(&adapters), Some(1));
        // PostMultiplied beats Inherit but loses to PreMultiplied.
        let adapters = [
            meta(DeviceType::IntegratedGpu, false, true, false), // Post
            meta(DeviceType::IntegratedGpu, false, false, true), // Inherit
        ];
        assert_eq!(choose_display_driving_adapter(&adapters), Some(0));
    }

    #[test]
    fn falls_back_to_discrete_when_no_integrated_exists() {
        // A single-dGPU desktop: the discrete GPU is the only option and must be
        // chosen (it is not off-screen there, so it composites fine).
        let adapters = [meta(DeviceType::DiscreteGpu, true, false, false)];
        assert_eq!(choose_display_driving_adapter(&adapters), Some(0));
    }

    #[test]
    fn stable_first_on_full_tie() {
        let adapters = [
            meta(DeviceType::IntegratedGpu, true, false, false),
            meta(DeviceType::IntegratedGpu, true, false, false),
        ];
        assert_eq!(choose_display_driving_adapter(&adapters), Some(0));
    }

    #[test]
    fn none_for_empty_list() {
        assert_eq!(choose_display_driving_adapter(&[]), None);
    }

    #[test]
    fn egui_wgpu_configured_mode_matches_the_real_precedence() {
        // Intel Vulkan surface from the bug report → Opaque (the black window).
        assert_eq!(
            egui_wgpu_configured_mode(&[CompositeAlphaMode::Opaque, CompositeAlphaMode::Inherit]),
            CompositeAlphaMode::Opaque,
        );
        // A PreMultiplied-capable surface → transparent.
        assert_eq!(
            egui_wgpu_configured_mode(&[
                CompositeAlphaMode::Opaque,
                CompositeAlphaMode::PreMultiplied
            ]),
            CompositeAlphaMode::PreMultiplied,
        );
        // Pre beats Post.
        assert_eq!(
            egui_wgpu_configured_mode(&[
                CompositeAlphaMode::PostMultiplied,
                CompositeAlphaMode::PreMultiplied
            ]),
            CompositeAlphaMode::PreMultiplied,
        );
    }

    #[test]
    fn supports_transparency_detects_configurable_modes_only() {
        assert!(supports_transparency(&[CompositeAlphaMode::PreMultiplied]));
        assert!(supports_transparency(&[CompositeAlphaMode::PostMultiplied]));
        // Inherit is NOT configurable by egui-wgpu, so it does not count as
        // "supported" here (it resolves to Opaque via Auto).
        assert!(!supports_transparency(&[
            CompositeAlphaMode::Opaque,
            CompositeAlphaMode::Inherit,
        ]));
        assert!(!supports_transparency(&[]));
    }
}

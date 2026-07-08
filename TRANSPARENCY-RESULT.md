# C0PL4ND window transparency — investigation, fix (attempt 2 / redirected), and test instructions

Branch: `fix/transparency-compositing`
Binary to test: `../c0pl4nd-transparency/target/release/c0pl4nd.exe` (built fresh — the main thread must LAUNCH it; transparency is hardware-verifiable only).

## TL;DR

The window rendered **opaque black** in every translucent mode on the hybrid-GPU
(Optimus) laptop. The `gpu-diag.log` instrumentation added in attempt 1 gave the
answer, and it showed attempt 1 picked the WRONG GPU. This attempt (2) **redirects**
the fix to prefer the **integrated / display-driving GPU** and to stop forcing the
Vulkan backend — replicating the sibling app **SCR1B3**, which is see-through on the
same machine. Test the fresh binary.

## The smoking gun (from attempt 1's `gpu-diag.log`)

| Adapter | `alpha_modes` | Reality |
|---|---|---|
| **Intel Iris Xe** (integrated) | `[Opaque, Inherit]` | **Drives the display** — DWM composites its output directly. The only GPU that can actually show the desktop through the window. |
| **NVIDIA RTX 3080 Ti** (discrete) | `[Opaque, PreMultiplied]` | Transparent *on paper*, but on Optimus it renders **off-screen** and the copy to the Intel-driven display is **opaque**, so its transparency never reaches the screen. |

Attempt 1 preferred the adapter advertising `PreMultiplied` → it chose **NVIDIA** →
still black (the Optimus display path opaqued it).

## Exact SCR1B3-vs-C0PL4ND differences (SCR1B3 = the working reference)

Both apps use the **identical** stack: `eframe 0.34.3`, `egui-wgpu 0.34.3`,
`wgpu 29.0.3`. SCR1B3 (`crates/scribe-app/src/main.rs` ~150-185) does only this for
the see-through path, and nothing else:

| Knob | SCR1B3 (see-through) | C0PL4ND before this fix | Fixed to |
|---|---|---|---|
| GPU backend | **eframe default** (not forced) | **forced `Backends::VULKAN`** | default (unforced) when translucent |
| `power_preference` | `LowPower` (integrated) | `HighPerformance` (discrete) | `LowPower` when translucent |
| `desired_maximum_frame_latency` | `1` | `3` | `1` when translucent (keeps `3` opaque) |
| adapter selector | none | none (attempt 1 added a wrong one) | selector that prefers the **integrated** GPU |
| `with_transparent(true)` | yes when translucent | yes | unchanged |
| clear-color / panel fill | alpha-folded | already alpha-folded (verified) | unchanged (was never the bug) |

**Forcing Vulkan was the core bug.** It pinned the integrated/display-driving GPU to
its *Vulkan* surface, whose `alpha_modes` are `[Opaque, Inherit]` (no
`PreMultiplied`/`PostMultiplied`). egui-wgpu 0.34.3 only configures
`PreMultiplied > PostMultiplied > Auto`; with neither present it passes `Auto`, and
wgpu-core resolves `Auto` via the fallback list `[Opaque, Inherit]` — **Opaque wins**
(it is always present) → solid black. SCR1B3 never forces Vulkan, so wgpu reaches the
surface/adapter path that actually composites on the display-driving iGPU. (Verified
by reading `egui-wgpu-0.34.3/src/winit.rs:247-270`, `wgpu-core-29.0.3
/src/device/resource.rs:5013-5045`, and `wgpu-hal .../dx12/adapter.rs:1297` where a
plain-HWND DX12 surface is `[Opaque]`-only.)

C0PL4ND's painting layer was **not** the bug: `window_clear_color` and the
`CentralPanel` fill both already fold the opacity/alpha correctly (confirmed in
`egui_app/mod.rs:3295` and `:4402-4411`).

## What this attempt changes (all in `crates/app/src/egui_main.rs` + `egui_app/gpu_diag.rs`)

1. **`prefer_backend_on_windows`** — translucent window: **do NOT force Vulkan**; use
   eframe's default backends (SCR1B3-parity). Opaque window: still force Vulkan
   (dodges the NVIDIA DX12 glyph-garble). An explicit `graphics_backend` /
   `WGPU_BACKEND` still wins in both modes.
2. **`apply_gpu_preference`** — default `PowerPreference::LowPower` when translucent
   (integrated GPU), `HighPerformance` when opaque. `graphics_gpu` config +
   `WGPU_POWER_PREF` still win.
3. **`desired_maximum_frame_latency`** — `1` when translucent (SCR1B3), `3` opaque.
4. **`install_transparency_adapter_selector`** (REDIRECTED) — an eframe
   `native_adapter_selector` that prefers the **integrated / display-driving** GPU
   and never the Optimus-opaqued discrete GPU, via
   `gpu_diag::choose_display_driving_adapter` (integrated class weighted far above
   the transparent-mode bonus; discrete only as a single-GPU-desktop fallback).
   This overrides wgpu's `power_preference` hint with a hard device-class rule.
5. **`gpu_diag.rs`** — instrumentation kept and extended: each candidate adapter's
   `alpha_modes` **and the mode egui-wgpu will actually configure**
   (`egui_wgpu_configured_mode`) are logged, plus the final CHOSEN adapter + its
   configured alpha mode.
6. **Tint ON/OFF toggle** (unchanged from attempt 1) — new `Config.tint_enabled`
   (default `true`), a "Enable tint wash" checkbox in Settings → Appearance, and
   `paint_background_tint` now no-ops when it is off (independent of the strength
   slider).

## Hypothesis for this attempt

On an Optimus laptop only the **integrated GPU physically drives the display**, so
DWM composites its transparent (layered) window directly; the discrete GPU renders
off-screen and its result is copied back opaque. Selecting the integrated GPU +
using eframe's default (unforced) backends is exactly SCR1B3's configuration, which
is see-through on this machine. If this attempt is still black, the new
`gpu-diag.log` now records the **chosen adapter + its configured alpha mode**, which
will say definitively whether we landed on the integrated GPU and what alpha mode the
swapchain got — the next lever.

## Diagnostics log

Path: `<config_dir>/gpu-diag.log` (next to `config.toml`;
`%APPDATA%\com.itashacorp.c0pl4nd\...` on Windows). Truncated + rewritten each launch
(only when a translucent mode is on). It lists every adapter (name, device type,
backend, driver, `alpha_modes`, the mode egui-wgpu would configure), the CHOSEN
adapter + its configured alpha mode, and a `RenderState bound:` cross-check line from
app startup (effective translucency, window mode, opacity, clear-color alpha).

## Verification done here

- `cargo build --release --bin c0pl4nd` — OK (fresh binary at `target/release/c0pl4nd.exe`).
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo fmt` — clean.
- Unit tests: `gpu_diag` selection (7), power-preference resolution, tint/clear-color,
  and the `egui_settings` integration test (`the_tint_strength_slider_changes_the_live_config`) — all green.

## NOT done (by instruction)

No PR, no release. Transparency is hardware-verifiable only — the **main thread must
launch `target/release/c0pl4nd.exe`** so the user can confirm see-through and hand
back `gpu-diag.log` if it is still opaque.

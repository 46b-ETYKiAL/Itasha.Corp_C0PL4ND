# c0pl4nd Structural Audit

Audit of the Rust terminal emulator at
`.s4f3-data/pubrepo-work/c0pl4nd-audit`. Stack: winit 0.30 + wgpu +
glyphon + vte + portable-pty (PTY behind `c0pl4nd-core::pty` / the
`Session` API).

## Verification method

This environment intermittently garbles/empties tool output. Every fact
below was confirmed by **two independent reads** (full Read + targeted
offset Read, or Read cross-checked against Grep with line numbers). All
six requested files were ultimately read in full and dual-confirmed.
Crate names are `c0pl4nd-core` / `c0pl4nd-renderer` / `c0pl4nd-app`;
the product/window name comes from `c0pl4nd_core::PRODUCT_NAME`.

Corrections to common assumptions baked into the task prompt are called
out inline (the prompt's guessed constant values, struct names, and the
"backplate pipeline doesn't exist yet" premise are all wrong — see §3/§4).

---

## 1. Config schema — `crates/core/src/config/mod.rs` (307 lines)

The config is a **directory module** (`config/mod.rs`), not a flat file.
Zero-config by design (module doc lines 1–3).

### `ConfigError` (10–23)
`NotFound(PathBuf)` · `Io { path, source }` · `Parse { path, message }`
· `Invalid(String)` — `thiserror`-derived, never panics.

### Top-level `Config` (167–185) — 11 fields

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,            // 171  theme file-stem to load
    pub font: FontConfig,         // 172
    pub scrollback_lines: usize,  // 173
    pub opacity: f32,             // 175  0.0..=1.0
    pub cursor: CursorConfig,     // 176
    pub window: WindowConfig,     // 177
    pub effects: EffectsConfig,   // 178
    pub keybindings: Keybindings, // 179
    pub update: UpdateConfig,     // 180
    pub startup_panel: bool,      // 182  neofetch-style splash on launch
    pub shell: Option<String>,    // 184  None = platform default shell
}
```

> The prompt's expected sub-struct names (ThemeConfig / ColorConfig /
> ScrollbackConfig / ShellConfig / KeybindConfig / BehaviorConfig) **do
> not exist.** `theme` is a bare `String` (a theme file-stem resolved by
> `theme/mod.rs`); `scrollback_lines` a bare `usize`; `shell` a bare
> `Option<String>`. Behaviour-type flags live in `EffectsConfig` +
> `startup_panel` + `opacity` + `update`.

### Sub-structs (exact fields, dual-verified)

| Struct | Lines | Fields |
|---|---|---|
| `FontConfig` | 27–34 | `family: String`, `size: f32`, `line_height: f32` (px cell advance), `fallback: Vec<String>` (ordered fallback families, CJK etc.) |
| `UpdateConfig` | 52–57 | `check_on_launch: bool`, `channel: String` |
| `Keybindings` | 71–83 | `copy`, `paste`, `new_tab`, `close_tab`, `next_tab`, `split_right`, `split_down`, `search`, `command_palette`, `increase_font`, `decrease_font` — all `String` |
| `CursorStyle` (enum) | 105–111 | `Block` · `Bar` · `Underline` (`#[serde(rename_all="snake_case")]`) |
| `CursorConfig` | 113–118 | `style: CursorStyle`, `blink: bool` |
| `WindowConfig` | 129–135 | `cols: u16`, `rows: u16`, `padding: u16` |
| `EffectsConfig` | 147–155 | `crt_scanlines: bool`, `chromatic_aberration: f32` |

> **WindowConfig (129–135) — direct answer to the explicit question:**
> has **only** `cols`, `rows`, `padding` (all `u16`). **NO `position`,
> NO `maximized`, NO remembered geometry, NO pixel width/height, NO
> decorations field.** Window size is expressed in terminal cells, not
> pixels. Persisting window geometry / maximized state requires **new
> fields here** plus a config writer (which does not exist — see below).

### Defaults (dual-verified)

- `FontConfig` (36–46): family **"Monaspace Neon"**, size 14.0,
  line_height 20.0, fallback `["Noto Sans JP", "monospace"]`.
- `UpdateConfig` (59–66): check_on_launch **false** (local-first; no
  network on launch), channel "stable".
- `Keybindings` (85–103): copy `mod+shift+c`, paste `mod+shift+v`,
  new_tab `mod+shift+t`, close_tab `mod+shift+w`, next_tab `mod+shift+]`,
  split_right `mod+shift+d`, split_down `mod+shift+e`, search
  `mod+shift+f`, command_palette `mod+shift+p`, increase_font
  `mod+plus`, decrease_font `mod+minus`. ("mod" = Ctrl+Shift on
  Win/Linux, Cmd on macOS — per the doc comment.)
- `CursorConfig` (120–127): `Block`, blink true.
- `WindowConfig` (137–145): cols 80, rows 24, padding 8.
- `EffectsConfig` (157–164): crt_scanlines false, chromatic_aberration 0.0.
- `Config` (187–203): theme **"itasha-void"**, scrollback_lines 10_000,
  opacity 1.0, startup_panel true, shell None.

### Load / parse / validate / path (impl 205–266)

```rust
impl Config {
    pub fn from_toml(src: &str, path: &Path) -> Result<Config, ConfigError>  // 207
        // toml::from_str -> ConfigError::Parse on failure; then self.validate()
    pub fn validate(&self) -> Result<(), ConfigError>                        // 217
        // theme non-empty; opacity 0.0..=1.0; font.size > 0; cols/rows != 0
    pub fn default_path() -> Option<PathBuf>                                  // 239
        // windows: %APPDATA%\c0pl4nd\config.toml                 (241-245)
        // unix:    $XDG_CONFIG_HOME or $HOME/.config -> /c0pl4nd/config.toml (246-252)
    pub fn load_from(path: &Path) -> Result<Config, ConfigError>             // 256
        // read_to_string -> from_toml; NotFound -> Ok(Config::default()) (zero-config); other Io -> Err
}
```

- **Default path resolver:** `Config::default_path()` (239) —
  `%APPDATA%\c0pl4nd\config.toml` (Windows) /
  `$XDG_CONFIG_HOME`(or `$HOME/.config`)`/c0pl4nd/config.toml` (Unix).
- **Load fn:** `Config::load_from(path)` (256) + `Config::from_toml`
  (207). There is **no** zero-arg `Config::load()`; callers combine
  `default_path()` + `load_from()`.
- **SAVE / WRITE fn: DOES NOT EXIST.** No `save`/`write`/`to_string`/
  `toml::to_string` in this module. Every struct derives `Serialize`, so
  a writer is trivial to add (`toml::to_string_pretty(self)` + atomic
  `fs::write`). **Any persisted-settings / persisted-geometry feature
  must implement the writer first.** Single biggest config gap.

### Live-reload wiring

- **There is no file watcher in the app.** The user-facing config entry
  point is **"Open Config File"** — `App::open_config_file()`
  (`window.rs:444–462`): resolves `Config::default_path()`, writes a
  commented starter TOML if absent, and opens it in the OS default
  editor via `open_path()` (`window.rs:73–92`, `cmd /C start` on
  Windows / `open` on macOS / `xdg-open` on Linux).
- The doc comment at `window.rs:440–443` *claims* "The config is
  live-reloaded on save" — but **no `notify` watcher, no re-`load_from`
  call, and no reload path exists** in `window.rs` or anywhere
  dual-read. The live-reload claim is currently **doc-only / aspirational**.
  A live-reload task must add the watcher + re-load + buffer rebuild from
  scratch. (`crates/app/src/update.rs` exists but is the GitHub-release
  self-updater, not a config watcher.)

### Tests (268–306): `defaults_are_sane_and_valid`,
`partial_toml_fills_defaults` (proves `#[serde(default)]` partial-fill),
`invalid_opacity_is_rejected`, `malformed_toml_is_error_not_panic`,
`missing_file_yields_defaults`.

---

## 2. Window creation — `crates/app/src/window.rs` (2747 lines)

Frameless by design (module doc 1–7): "C0PL4ND draws its own brand title
bar … instead of the OS default."

### `Gpu` struct (121–145)

```rust
struct Gpu {
    window: Arc<Window>,                                   // 122
    device: wgpu::Device,                                  // 123
    queue: wgpu::Queue,                                    // 124
    surface: wgpu::Surface<'static>,                       // 125
    surface_config: wgpu::SurfaceConfiguration,            // 126
    font_system: FontSystem,                               // 127  glyphon
    swash_cache: SwashCache,                               // 128  glyphon
    atlas: TextAtlas,                                      // 129  glyphon
    viewport: Viewport,                                    // 130  glyphon
    text_renderer: TextRenderer,                           // 131  glyphon::TextRenderer
    metrics: Metrics,                                      // 132
    leaf_buffers: HashMap<LeafId, Buffer>,                 // 135  one grid buffer per visible leaf
    tabbar_buffers: HashMap<LeafId, Buffer>,               // 138  one nested-tab-strip buffer per cell w/ strip
    chrome_buffer: Buffer,                                 // 139  titlebar wordmark+title+button glyphs (single buffer)
    palette_buffer: Buffer,                                // 140
    splash_buffer: Buffer,                                 // 141  neofetch startup splash
    image_renderer: crate::image_render::ImageRenderer,    // 142  textured-quad pipeline
    chrome_renderer: ChromeRenderer,                       // 143  SOLID-COLOR quad pipeline
    gpu_name: String,                                      // 144
}
```

`Gpu::new(window, font_size)` (2481+) is the constructor; the glyphon
stack + both quad pipelines (`ImageRenderer::new` 2546,
`ChromeRenderer::new` 2547) are built there. `retain_leaf_buffers` (149)
drops leaf/tabbar buffers for closed leaves.

### `App` struct (155–200)

```rust
struct App {
    config: Config,                                        // 156
    theme: Theme,                                          // 157
    tabs: Vec<Tab>,                                        // 159  window tabs; each holds a split tree
    active: usize,                                         // 160
    gpu: Option<Gpu>,                                      // 161
    next_poll: Instant,                                    // 162
    cursor: (f64, f64),                                    // 163
    chrome_cursor: winit::window::CursorIcon,              // 166
    last_titlebar_click: Option<Instant>,                 // 169  double-click-maximize clock
    hover_leaf: Option<LeafId>,                            // 172
    modifiers: ModifiersState,                             // 173
    search_mode: bool,                                     // 174
    search_query: String,                                  // 175
    search_matches: Vec<c0pl4nd_core::search::SearchMatch>,// 176
    search_idx: usize,                                     // 177
    palette_mode: bool,                                    // 178
    palette_query: String,                                 // 179
    palette_idx: usize,                                    // 180
    splash: Option<String>,                                // 183
    pending_resize: Option<(u32, u32)>,                    // 188  debounced PTY resize
    last_pty_resize: Instant,                              // 190
    drag: DragState,                                       // 193  Ctrl+Shift pane drag-rearrange
    reduced_motion: bool,                                  // 196  honours C0PL4ND_REDUCED_MOTION env
    workspace_prompt: Option<WorkspacePrompt>,             // 199  Save/Restore-layout modal
}
```

Supporting types: `Tab { layout: Layout, cells: HashMap<LeafId, Cell> }`
(298–301); `Cell { group: TabGroup, sessions: Vec<Session> }` (249–252);
`WorkspacePrompt::{ Save{name}, Restore{names,idx} }` (206–211). The
split/tab tree lives in `c0pl4nd_core::layout` (`Layout`, `LeafId`,
`TabGroup`, `Preset`, `Axis`, `Direction`, `Rect as LRect`).

### `WindowAttributes` builder — `resumed()` (1663–1701)

```rust
fn resumed(&mut self, event_loop: &ActiveEventLoop) {
    if self.gpu.is_some() { return; }                                   // 1664
    let cols = self.config.window.cols;                                 // 1667
    let rows = self.config.window.rows;                                 // 1668
    let width  = (cols as f32 * CELL_W) as u32 + 16;                    // 1669
    let height = (rows as f32 * LINE_HEIGHT) as u32 + 16 + TITLEBAR_H as u32; // 1670
    let attrs = Window::default_attributes()                           // 1672
        .with_title(c0pl4nd_core::PRODUCT_NAME)                         // 1673
        .with_decorations(false)              // ALWAYS frameless        // 1674
        .with_resizable(true)                                          // 1675
        .with_inner_size(winit::dpi::LogicalSize::new(width as f64, height as f64)); // 1676
    let window = Arc::new(event_loop.create_window(attrs).expect("create window")); // 1677
    let gpu = match pollster::block_on(Gpu::new(window.clone(), self.config.font.size)) { ... }; // 1679-1686
    self.gpu = Some(gpu);                                               // 1688
    if self.tabs.is_empty() { self.spawn_tab(); self.restore_default_workspace_on_startup(); } // 1689-1697
    if let Some(g) = &self.gpu { g.window.request_redraw(); }           // 1698-1700
}
```

- Builder calls present: `with_title(PRODUCT_NAME)`,
  `with_decorations(false)` (**hard-coded false** — WindowConfig has no
  decorations field), `with_resizable(true)`,
  `with_inner_size(LogicalSize)` sized from cols×rows + 16px padding +
  titlebar.
- **ABSENT:** `with_position`, `with_maximized`, `with_transparent`.
  **A geometry-restore task adds `with_position`/`with_maximized`
  between line 1676 and 1677** (plus new WindowConfig fields + a config
  writer from §1, and a save-on-`Resized`/`Moved` path).

### `Gpu::new` GPU init (2481–2520+)

`wgpu::Instance::default()` (2483) → `create_surface(window.clone())`
(2484) → `request_adapter { power_preference: LowPower,
compatible_surface, force_fallback_adapter: false }` (2485–2491) →
`request_device { label "c0pl4nd-device", ..default }` (2492–2497) →
`surface.get_capabilities` (2499) → pick first **sRGB** format
(2500–2505) → `SurfaceConfiguration { usage: RENDER_ATTACHMENT, format,
width/height, present_mode: Mailbox if supported else Fifo (2513–2517),
alpha_mode: caps.alpha_modes[0], view_formats: [],
desired_maximum_frame_latency: 2 }` (2506–2520). Surface format is
threaded into both `ImageRenderer::new` and `ChromeRenderer::new`.

### raw-window-handle / HWND / win32

- **No `raw_window_handle`, `HWND`, `windows` crate, or `win32` usage**
  anywhere. wgpu consumes the handle internally via
  `instance.create_surface(window.clone())` (2484); **no native HWND is
  exposed to app code.**
- The only OS-native code is `open_path()` (73–92): `cmd /C start ""`
  on Windows (with `CREATE_NO_WINDOW = 0x0800_0000` via
  `std::os::windows::process::CommandExt::creation_flags`), `open` on
  macOS, `xdg-open` on Linux — for opening the config file.
- Frameless window controls use **winit's own APIs**, not the raw
  handle: `window.set_minimized(true)` (1807),
  `window.set_maximized(!is_maximized())` (1809–1810/1814–1815),
  `window.drag_window()` (1817), `window.drag_resize_window(dir)` (1787).
  Any DWM-blur / custom-snap feature needing the HWND must add
  `raw-window-handle` and pull it from `Gpu.window`.

---

## 3. Render pass + the solid-color quad pipeline

### Main render — `App::render()` (2001–2477) — THE load-bearing function

There is **one** render pass (`label: "text"`, 2437). Build phase first,
then a single `begin_render_pass`. Sequence:

1. Resolve colors: `fg_color()` (1591), `accent_color()` (1596, from
   `theme.cursor`), `bg_color()` (1581, sRGB→linear `wgpu::Color`),
   `signal_red = rgb(255,0,64)` (2005). `to_rgba` closure converts
   glyphon Color → `[f32;4]` (2006).
2. `content = self.content_rect()` (2016); `cells =
   active_tab.layout.cascade(content)` (2017–2020) → `Vec<(LeafId,
   LRect)>`; `focused` leaf (2021–2024).
3. **Per-leaf snapshot (2031–2038):** `leaf_spans(leaf, fg)` (1317) →
   per-color `Vec<(String, GColor)>` spans (CLEARS the leaf's damage
   flag); `collect_leaf_image_quads(leaf, cell)` (1271) → inline-image
   `ImageQuad`s.
4. Build nested-tab strip text (2048–2065) via `cell_tabbar_text`;
   palette overlay text (2073–2100); splash text (2102); drag overlay
   `ColorRect`s via `drag_overlay_quads(accent)` (2105); `border =
   BORDER_PX if cells>1 else 0` (2108).
5. **Borrow gpu mut (2110).** Fill each leaf's glyphon `Buffer`
   (`set_rich_text` per-color spans, 2120–2148); tab-strip buffers
   (2151–2167); the **chrome buffer** (2169–2217, see §4); palette
   (2219–2229) and splash (2231–2246) buffers.
6. Acquire frame (2248), create view (2256), `viewport.update` (2260).
7. Build `Vec<TextArea>` (2270–2368): titlebar chrome first (left:6,
   top:6, bounds 0..TITLEBAR_H, 2272–2285), then one per visible leaf
   grid (origin via `leaf_text_origin`, bounds via `leaf_text_bounds`,
   2290–2310), then tab strips (2313–2333), then splash (2334–2351) and
   palette (2352–2368) overlays.
8. `text_renderer.prepare(device, queue, &mut font_system, &mut atlas,
   &viewport, areas, &mut swash_cache)` (2369–2381).
9. **Pane chrome quads (2383–2397):** `chrome_quads(&cells, focused,
   to_rgba(accent), [0.30,0.30,0.34,1.0], [bg…], content)` (2386–2394)
   → `chrome_renderer.prepare(device, w, h, &chrome)` (2395–2397). Drag
   overlay prepared separately (2399–2401).
10. **Image quads grouped per leaf + per-leaf scissor (2403–2429):**
    each leaf's images get `image_renderer.prepare(...)` and a
    `leaf_scissor(cell, border, w, h)` rect, collected into
    `prepared_image_groups`.
11. **THE RENDER PASS (2436–2473):** one `begin_render_pass`, single
    color attachment on `view`, `LoadOp::Clear(bg)`, `StoreOp::Store`,
    no depth/stencil. **Draw order (painters'):**
    ```rust
    gpu.chrome_renderer.draw(&mut pass, &prepared_chrome);   // 2454  1. PANE chrome quads (gutter+borders)
    gpu.text_renderer.render(&atlas,&viewport,&mut pass);     // 2455-2457  2. ALL text (grids+titlebar+strips+overlays)
    for (scissor, prepared) in &prepared_image_groups {       // 2460  3. inline images, set_scissor_rect per leaf
        pass.set_scissor_rect(sx,sy,sw,sh); gpu.image_renderer.draw(&mut pass, prepared);
    }
    if !prepared_overlay.is_empty() {                         // 2469  4. drag overlay LAST, scissor reset to full
        pass.set_scissor_rect(0,0,w,h); gpu.chrome_renderer.draw(&mut pass, &prepared_overlay);
    }
    ```
12. `queue.submit` (2474), `frame.present()` (2475), `atlas.trim()`
    (2476).

> **Key facts for the chrome-backplate task:**
> - The app **ALREADY HAS a solid-color quad pipeline** —
>   `ChromeRenderer` in `pane_render.rs` (§ below) — with a clean
>   `ColorRect` input and a `prepare`/`draw` API. **No new pipeline is
>   needed for backplates.**
> - BUT `ChromeRenderer` is currently used **only for PANE chrome
>   (split gutters + per-leaf borders, 2454)** and the **drag overlay
>   (2471)**. It is **NOT** currently used for the titlebar at all.
> - The titlebar buttons today are **pure glyphs in text** (§4), drawn
>   in step 2 (2455) with **zero backplate quads behind them.**
> - To add caption-button backplates: build `ColorRect`s at the §4
>   button pixel rects and feed them to `chrome_renderer` **before** the
>   text draw. Because chrome quads (2454) precede text (2455) in the
>   pass, a quad pushed there renders *behind* the button glyphs
>   automatically. Either (a) push them into a new prepared set drawn at
>   step 1, or (b) extend `chrome_quads` / add a sibling
>   `titlebar_quads()` in `pane_render.rs`. NOTE: the existing
>   `chrome_quads` early-returns empty for a single pane (2386 →
>   `cells.len() <= 1`), so titlebar backplates must NOT go through that
>   function unchanged — add a separate quad set so they always render.

### Text rendering — glyphon directly (renderer crate is a stub)

`crates/renderer/src/lib.rs` (28 lines) only defines
`FramePolicy { OnDamage (default), Continuous }` + one test. **All real
text rendering is glyphon's `TextRenderer` used inline in `window.rs`**
(`gpu.text_renderer.prepare(...)` 2369, `.render(...)` 2455). The
per-leaf/chrome/palette/splash TextArea bounds & origins are assembled
at 2270–2368; the layout helpers (`leaf_text_origin`,
`leaf_text_bounds`, `leaf_scissor`, `cell_tabbar_text`) live in
`pane_render.rs`.

### Image pipeline — `crates/app/src/image_render.rs` (244 lines)

Textured screen-space quads (inline sixel images). Secondary reference
for quads; for *solid* backplates use `ChromeRenderer` instead.

```rust
pub struct ImageQuad { pub rgba: Vec<u8>, pub width: u32, pub height: u32, pub x: f32, pub y: f32 } // 11-17
pub struct Prepared { bind_group: BindGroup, vbuf: Buffer, _texture: Texture }                       // 20-24
pub struct ImageRenderer { pipeline: RenderPipeline, sampler: Sampler, layout: BindGroupLayout }     // 26-30
```

- **`ImageRenderer::new(device, format)` (33–102):** WGSL `SHADER`
  module (34–37); bind group layout (38–58) = binding0 `Texture{Float
  filterable, D2}` + binding1 `Sampler(Filtering)`, both FRAGMENT;
  pipeline layout (59–63); render pipeline (64–92): vertex
  `entry_point "vs"`, `VertexBufferLayout { array_stride: 16,
  attributes: vertex_attr_array![0=>Float32x2, 1=>Float32x2] }`;
  fragment `entry_point "fs"`, target `ColorTargetState { format, blend:
  Some(BlendState::ALPHA_BLENDING), write_mask: ColorWrites::ALL }`;
  `primitive: default` (triangle-list), `depth_stencil: None`,
  `multiview_mask: None`, `cache: None`. Sampler default (93–96).
- **`prepare(device, queue, surface_w, surface_h, quads) ->
  Vec<Prepared>`** (105–118) filters zero-size, maps `prepare_one`.
- **`prepare_one(...)` (120–200):** `Rgba8UnormSrgb` texture (128–141),
  `write_texture` (142–160), view + bind_group (161–175), then the
  **pixel-rect → NDC conversion (177–189)**:
  ```rust
  let x0 = q.x / sw * 2.0 - 1.0;                       // 178
  let x1 = (q.x + q.width as f32) / sw * 2.0 - 1.0;    // 179
  let y0 = 1.0 - q.y / sh * 2.0;                       // 180
  let y1 = 1.0 - (q.y + q.height as f32) / sh * 2.0;   // 181
  // 6 verts [x,y,u,v] = 2 triangles                    // 182-189
  ```
  `create_buffer_init(BufferUsages::VERTEX)` (190–194).
- **`draw(pass, prepared)` (203–213):** `set_pipeline` → per quad
  `set_bind_group(0,..)` + `set_vertex_buffer(0,..)` + `draw(0..6, 0..1)`.
- **`bytemuck_cast` (217–221):** in-house `slice::from_raw_parts` (no
  bytemuck dep). WGSL `SHADER` (223–244): `vs` passes NDC position
  through; `fs` returns `textureSample(tex, samp, uv)`.

### Solid-color quad pipeline — `crates/app/src/pane_render.rs` (455 lines) ★ THE backplate model

Module doc (1–18): per-leaf render geometry + the `ChromeRenderer`
"tiny solid-colour quad pipeline that draws the inter-pane gutters and a
per-leaf border."

```rust
pub const BORDER_PX: i32 = 1;                                   // 25
#[derive(Debug, Clone, Copy)]
pub struct ColorRect { pub x: i32, pub y: i32, pub w: i32, pub h: i32, pub rgba: [f32; 4] }  // 30-41
impl ColorRect { pub fn new(x,y,w,h, rgba: [f32;4]) -> Self }   // 43-49  straight-alpha sRGB, pixel coords
pub struct ChromeRenderer { pipeline: wgpu::RenderPipeline }    // 182-184
pub struct PreparedQuad { vbuf: wgpu::Buffer }                  // 188-190
```

- **`ChromeRenderer::new(device, format)` (192–246):** WGSL `SHADER`
  module (196–199); pipeline layout with **NO bind groups**
  (`bind_group_layouts: &[]`, 200–204 — color is per-vertex, no
  uniforms/textures); render pipeline (205–245): vertex `entry_point
  "vs"`, `VertexBufferLayout { array_stride: 6*4=24,
  attributes: [Float32x2 @loc0 offset 0 (pos), Float32x4 @loc1 offset 8
  (rgba)] }` (212–227); fragment `entry_point "fs"`, `ColorTargetState
  { format, blend: ALPHA_BLENDING, write_mask: ALL }` (229–238);
  `primitive: default`, `depth_stencil: None`, `multiview_mask: None`,
  `cache: None`.
- **`prepare(&self, device, surface_w: f32, surface_h: f32, quads:
  &[ColorRect]) -> Vec<PreparedQuad>` (251–276):** filters w>0/h>0/
  surface>0, builds a mapped vertex buffer per quad via `quad_verts`,
  writes bytes through `get_mapped_range_mut`. **Note: signature takes
  `device` only — NO `queue` (unlike `ImageRenderer::prepare`).**
- **`draw(&self, pass, prepared)` (279–289):** `set_pipeline` → per quad
  `set_vertex_buffer(0,..)` + `draw(0..6, 0..1)` (no bind groups).
- **`quad_verts(q, sw, sh)` (292–300):** pixel→NDC
  `(px/sw*2-1, 1-py/sh*2)`; 6 verts of `[x,y,r,g,b,a]` (2 triangles).
- **`verts_bytes` (303–307):** in-house `slice::from_raw_parts`. WGSL
  `SHADER` (309–327): `vs` passes pos through + forwards color; `fs`
  returns `in.color`. **This is the exact pipeline a colored backplate
  quad uses — feed it `ColorRect`s.**

Other `pane_render.rs` exports (all dual-verified, all `pub`):
- `leaf_text_bounds(cell, border) -> TextBounds` (54–65) — grid text
  clip, inset by border.
- `leaf_text_origin(cell, border, left_pad, top_pad) -> (f32,f32)`
  (71–76).
- `leaf_scissor(cell, border, surface_w, surface_h) -> (u32,u32,u32,u32)`
  (81–94) — wgpu scissor rect for per-cell images.
- `chrome_quads(cells, focused, accent_rgba, border_rgba, gutter_rgba,
  surface) -> Vec<ColorRect>` (103–133) — **PANE** chrome: returns empty
  for ≤1 cell (112–114), else a full-surface gutter fill + a
  `border_ring` (136–148, 4 edge rects) per leaf, accent on focused.
  **NOT the titlebar.**
- `cell_tabbar_text(tab_count, active, max_cols) -> String` (155–174) —
  `" 1 [2] 3 "` style nested-tab strip.
- 12 unit tests (329–454) covering all of the above.

---

## 4. Chrome / titlebar — `crates/app/src/window.rs`

### Constants (35–60)

| Const | Value | Meaning |
|---|---|---|
| `LINE_HEIGHT` | `20.0` | cell line height (px) |
| `CELL_W` | `9.0` | cell width (px) |
| `CELL_TABBAR_H` | `= LINE_HEIGHT` (20.0) | nested-tab strip height |
| `CELL_STRIP_MIN_H` | `CELL_TABBAR_H + 3*LINE_HEIGHT` (= 80) | min cell height to show strip |
| `TITLEBAR_H` | `30.0` | custom titlebar height (px) |
| `BUTTON_CELLS` | `5.0` | cells per caption button → 5×9 = **45 px** |
| `BUTTONS_CELLS` | `BUTTON_CELLS * 3.0` (15.0) | 3-button cluster → 15×9 = **135 px** |
| `BTN_RIGHT_MARGIN` | `8.0` | gap from close button to right edge |
| `RESIZE_BORDER` | `8.0` (f64) | invisible frameless resize band thickness |
| `CHROME_LEFT` | `6.0` | left inset of chrome text |

> The prompt's guessed values (BUTTON_CELLS=3, CELL_W=8, TITLEBAR_H=28)
> are wrong. Actual: BUTTON_CELLS=**5**, CELL_W=**9**, TITLEBAR_H=**30**,
> LINE_HEIGHT=**20**.

### `TitlebarHit` enum (112–119)

`None`, `Drag`, `Minimize`, `Maximize`, `Close` (5 variants). There are
**no** resize variants in this enum — frameless resize is a separate
`hit_resize_edge()` (1632–1659) returning `winit::window::ResizeDirection`.

### `hit_titlebar(&self, x: f64, y: f64) -> TitlebarHit` (1602–1620)

```rust
let width = gpu.surface_config.width as f64;                 // 1603-1607
if y > TITLEBAR_H as f64 { return None; }                    // 1608-1610
let buttons_left = Self::buttons_left_px(width as f32) as f64; // 1611
if x < buttons_left { return Drag; }                         // 1612-1613
match ((x - buttons_left) / (BUTTON_CELLS as f64 * CELL_W as f64)) as i32 { // 1615
    0 => Minimize, 1 => Maximize, _ => Close,                // 1616-1618
}
```

### `buttons_left_px(width: f32) -> f32` (1625–1627) — ★ shared geometry

```rust
width - BUTTONS_CELLS * CELL_W - BTN_RIGHT_MARGIN   // = width - 135 - 8 = width - 143
```

This is **the** shared anchor: `hit_titlebar` (1611) AND the chrome text
layout in `render()` (`buttons_left = Self::buttons_left_px(width)`,
2171) both call it, so click zones and rendered glyphs align by
construction.

**Exact pixel geometry of each caption button (window width `w`):**

| Region | x-range (px) | y-range (px) | width |
|---|---|---|---|
| Drag (wordmark/title) | `0 .. w-143` | `0 .. 30` | — |
| Minimize | `w-143 .. w-98` | `0 .. 30` | 45 |
| Maximize | `w-98 .. w-53` | `0 .. 30` | 45 |
| Close | `w-53 .. w-8` | `0 .. 30` | 45 |
| Right margin | `w-8 .. w` | — | 8 |

(`buttons_left = w-143`; button N at `buttons_left + N*45`,
`N ∈ {0,1,2}`.) **A per-button backplate `ColorRect` uses exactly
these rects**, e.g. close-button backplate =
`ColorRect::new((w-53) as i32, 0, 45, TITLEBAR_H as i32, color)`, fed
to `chrome_renderer` before the text draw (§3).

### Where the chrome text + button glyphs are built — `render()` (2169–2217)

The titlebar is a **single glyphon `Buffer`** (`gpu.chrome_buffer`)
filled with rich-text spans. Exact code (2169–2203):

```rust
let buttons_left = Self::buttons_left_px(width);                 // 2171
let pad_cols = (((buttons_left - CHROME_LEFT) / CELL_W).round()).max(0.0) as usize; // 2172
let title = /* search-mode | "{PRODUCT}  [n/m]" multi-tab | " {PRODUCT} " */ ; // 2173-2189
let mut chrome: String = title.chars().take(pad_cols).collect(); // 2192  pad/truncate
while chrome.chars().count() < pad_cols { chrome.push(' '); }    // 2193-2195
let chrome_spans = [                                             // 2198-2203
    (chrome, accent),
    ("  \u{2014}  ".to_string(), fg),         // minimize  — (U+2014, 5 cells)
    ("  \u{25a1}  ".to_string(), fg),         // maximize  □ (U+25A1, 5 cells)
    ("  \u{2715}  ".to_string(), signal_red), // close     ✕ (U+2715, 5 cells, red)
];
gpu.chrome_buffer.set_rich_text(&mut gpu.font_system,
    chrome_spans.iter().map(...), &Attrs...color(fg), Shaping::Advanced, None); // 2204-2215
gpu.chrome_buffer.shape_until_scroll(&mut gpu.font_system, false);             // 2216-2217
```

- The title is left-padded to exactly `pad_cols` columns so the three
  5-cell button glyphs land on their `BUTTON_CELLS`-wide hit zones
  (comment 2190–2197). Glyphs: `—` minimize, `□` maximize, `✕` close
  (close colored `signal_red` = rgb(255,0,64)).
- The chrome buffer is rendered as the **first TextArea** (2272–2285,
  `left:6.0, top:6.0, bounds 0..TITLEBAR_H`) in the text-render step.
- **There are currently NO backplate quads behind the buttons** — only
  these glyphs. This is exactly the gap the backplate task fills.

### Titlebar interaction — `window_event()` (1703–1822)

- `CursorMoved` (1706): drag state machine (1710–1730), then frameless
  resize/button cursor feedback (1736–1759) — `hit_resize_edge` →
  Ew/Ns/Nwse/Nesw resize cursors; else `hit_titlebar` → `Pointer` on a
  button else `Default`. Hover-leaf tracking (1760–1767).
- `MouseInput Pressed Left` (1769–1823): Ctrl+Shift+press arms a pane
  drag (1776–1783); else `hit_resize_edge` → `drag_resize_window(dir)`
  (1785–1790); else `hit_titlebar` (1791) →
  `Close => event_loop.exit()` (1806),
  `Minimize => window.set_minimized(true)` (1807),
  `Maximize => window.set_maximized(!is_maximized())` (1808–1811),
  `Drag => double-click toggles maximize else window.drag_window()`
  (1812–1819).

---

## 5. Command palette — `crates/app/src/window.rs` (the settings-overlay model)

### `PALETTE_ACTIONS: &[&str]` (216–244) — 26 plain display strings

```
"New Tab" "Close Tab" "Next Tab" "Previous Tab"
"New Cell Tab" "Next Cell Tab" "Previous Cell Tab"
"Split Right" "Split Down" "Focus Next Pane" "Zoom Pane"
"Equalize Panes" "Auto Arrange"
"Layout: 1" "Layout: 1x2" "Layout: 2x1" "Layout: 1+2"
"Layout: 2x2" "Layout: 1+3" "Layout: 2x3"
"Save Layout As…" "Restore Layout"
"Search" "Scroll To Bottom"
"Settings" "Open Config File" "Quit"
```

> Actions are **plain strings**, not (label, id) tuples. Both
> `"Settings"` and `"Open Config File"` dispatch to
> `open_config_file()` (434) — **there is no in-app settings panel
> yet**; the doc at 440–443 calls the config file "the discoverable
> Settings entry point until the in-app panel lands." This is precisely
> the gap a settings-overlay task fills.

### Toggle / state / navigation

- State on `App`: `palette_mode: bool`, `palette_query: String`,
  `palette_idx: usize` (178–180).
- **Open:** `enter_palette()` (359–363) clears query+idx, sets
  `palette_mode=true`. Wired to **Ctrl+Shift+P** via
  `handle_tab_combo` `Some('p') => self.enter_palette()` (1474–1477).
- **Fuzzy filter:** `palette_filtered()` (365–367) →
  `c0pl4nd_core::fuzzy::filter_sorted(PALETTE_ACTIONS, &palette_query)`
  (live fuzzy search; reusable by a settings overlay).
- **Key handling:** `handle_palette_key(key, event_loop)` (369–400):
  Escape closes (371–373); ArrowDown/Up move `palette_idx` mod filtered
  len (374–381); Enter takes `palette_filtered()[palette_idx]`, closes,
  `execute_palette_action(action, event_loop)` (382–388); Backspace pops
  query (389–392); `Key::Character` appends to query, resets idx
  (393–396). Routed FIRST in keyboard handling when `palette_mode`
  (1906–1909).

### Dispatch — `execute_palette_action(&mut self, action: &str, event_loop)` (402–438)

`match action { ... }` mapping each display string to a method:
`"New Tab"=>spawn_tab`, `"Close Tab"=>close_active_tab`,
`"Split Right"=>split_active(Axis::Horizontal)`,
`"Split Down"=>split_active(Axis::Vertical)`,
`"Layout: 2x2"=>apply_preset(Preset::Grid2x2)` (etc., 417–423),
`"Save Layout As…"=>open_save_prompt`,
`"Restore Layout"=>open_restore_prompt`, `"Search"=>enter_search`,
`"Scroll To Bottom"=>…scroll_to_bottom` (427–433),
`"Settings" | "Open Config File"=>open_config_file` (434),
`"Quit"=>event_loop.exit()` (435), `_=>{}` (436).

### Overlay rendering

The palette overlay text is built in `render()` (2073–2100): a header
`"▒ command palette  › {query}"` plus one line per filtered action
(`▸ ` marker on the selected row, action padded to 26 + `action_hint`
shortcut). It's pushed into `gpu.palette_buffer` (2219–2229) and added
as a centered TextArea (`left: w*0.25, top: TITLEBAR_H+40`, 2352–2368)
only when `palette_mode`. `action_hint(action)` (97–108) maps actions
to shortcut hint strings.

> **Settings-overlay recipe:** the closest existing template is the
> `WorkspacePrompt` modal (enum 206–211; `open_save_prompt`/
> `open_restore_prompt` 1136–1153; `handle_workspace_prompt_key`
> 1157–1196), which is a typed-input + arrow-select + Enter-applies
> overlay routed FIRST in keyboard handling (1902–1905). To build a
> settings overlay: add `settings_mode` + index/edit state to `App`; a
> `gpu.settings_buffer` glyphon Buffer; a text-build + centered TextArea
> in `render()` (mirroring the palette block at 2073/2219/2352); a
> `handle_settings_key` branch routed before the PTY (mirror 1902–1909);
> an apply step that mutates `App.config` and calls the **new
> `Config` writer** (§1). Add a `ColorRect` backplate behind it via
> `chrome_renderer` for a solid panel (§3).

---

## 6. Feature / keybind inventory

### Keybinds — keyboard dispatch in `window_event` (1893–1957)

Modal overlays capture keys FIRST (workspace prompt 1902, palette 1906,
search 1910). Then chord families:

**Ctrl+`,`** (no shift, 1923–1929) → `open_config_file` (Settings).

**Ctrl-only (no shift/alt) — `handle_cell_tab_combo` (1559–1579), routed 1931:**

| Key | Action |
|---|---|
| `PageDown` | `next_cell_tab` |
| `PageUp` | `prev_cell_tab` |
| `t` | `spawn_cell_tab` (new tab in focused cell) |

**Ctrl+Shift (or Cmd+Shift) — `handle_tab_combo` (1451–1534), routed 1935–1939:**

| Key | Action |
|---|---|
| `t` | `spawn_tab` |
| `w` | `close_active_tab` |
| `]` | `next_tab` |
| `[` | `prev_tab` |
| `f` | `enter_search` |
| `p` | `enter_palette` |
| `d` | `split_active(Horizontal)` (split right) |
| `e` | `split_active(Vertical)` (split down) |
| `o` | `focus_next_pane` |
| `=` / `+` | `equalize` |
| `\` | `auto_balance` |
| Tab | `next_tab` |
| Arrow L/R/U/D | `focus_dir(Direction::…)` (directional pane focus) |
| Enter | `toggle_zoom` |

**Alt-only — `handle_alt_combo` (1539–1554), routed 1943–1948:**
`Alt+Arrow` = `swap_dir` (swap focused pane), `Alt+Shift+Arrow` =
`resize_focused`.

Plain keys → `key_to_bytes(logical_key, text)` → `session.write_input`
when no modal/chord consumes them (1949–1956); `scroll_to_bottom` first
(1952). `MouseWheel` scrolls the view via `scroll_up_view`/
`scroll_down_view` (1856–1878).

### Terminal features — `crates/core/src/term.rs` (648 lines)

VT/ANSI interpreter on `vte::{Parser, Perform}`. `Perform` is
implemented on `struct Screen` (43–66). State: `grid`, `row`, `col`,
`pen: Pen{fg, bg, flags}`, `history: VecDeque<Vec<Cell>>`,
`max_scrollback`, `view_offset`, `title`, `cwd: Option<String>`,
`prompt_marks: Vec<usize>` (OSC 133), `hyperlinks: Vec<String>` (OSC 8),
`images: Vec<TerminalImage>` (Sixel), `sixel_accum: Option<Vec<u8>>`.
The public wrapper `Terminal` (337–474) exposes `advance(bytes)`,
`grid`, `title`, `cwd`, `prompt_marks`, `hyperlinks`, `images`,
`scrollback_len`, `view_offset`, `scroll_up_view`/`scroll_down_view`/
`set_view_offset`/`scroll_to_bottom`, `display_rows`, `all_lines`,
`resize`.

**SUPPORTED (dual-verified):**

| Feature | Where | Notes |
|---|---|---|
| Print + line-wrap | `print()` 181–197 | wraps at cols, `newline()` (87–99) scrolls into history |
| C0 controls | `execute()` 199–214 | LF, CR, **HT** (8-col tab stops, 203–208), BS (0x08) |
| CSI cursor pos | `csi_dispatch` 225–240 | H / f (1-based → clamped 0-based) |
| CSI cursor move | 241–244 | A/B/C/D with clamping |
| CSI erase | 245–263 | J mode 2 = clear screen + home (only mode 2); K = erase line cursor→end |
| **SGR** | `sgr()` 101–164 | reset(0), bold(1), italic(3), underline(4), **inverse/reverse(7)**, **strikeout(9)**, un-set (22/23/24/27/29), 16-color fg/bg (30–37/40–47), bright (90–97/100–107), default fg/bg (39/49), **256-color (38/48;5;n)** + **truecolor (38/48;2;r;g;b)** (131–159) |
| **OSC 0 / 2** (title) | `osc_dispatch` 274–278 | sets `title` |
| **OSC 7** (cwd) | 279–283 | sets `cwd` (real, captures the URI) |
| **OSC 8** (hyperlink) | 284–291 | pushes URI to `hyperlinks` (the `id=` param is NOT parsed/stored) |
| **OSC 133** (semantic prompt) | 292–303 | `;A` or `;B` record absolute prompt line in `prompt_marks` (jump-to-prompt). **Explicit security note (294–295): NEVER routes any report back into the PTY — iTerm2 CVE-2024-38395/38396 class — capture only.** |
| **Sixel graphics** | `hook` 308–313 / `put` 315–322 / `unhook` 324–334 | DCS final byte `q` accumulates payload (capped **8 MiB**, 318) → `image::decode_sixel` → `TerminalImage { image, line, col }` anchored to absolute line; rendered by `ImageRenderer` |
| Scrollback | `newline` 87–99 + history VecDeque | capped at `max_scrollback` (default `DEFAULT_SCROLLBACK = 10_000`, 14) |
| Scrollback search | in `App` (§5) | `c0pl4nd_core::search::find` over `Terminal::all_lines()` (458–467) |
| Adversarial-input safety | test 610–647 + `fuzz/fuzz_targets/vt_parser.rs` | fuzz corpus: bare CSI, param overflow, truncated truecolor, OSC 52/1337, DCS, invalid UTF-8, etc. |

**ABSENT / incomplete (dual-verified by exhaustive read of `osc_dispatch`,
`csi_dispatch`, and the `Screen`/`Perform` impl):**

| Feature | Status |
|---|---|
| **Kitty graphics protocol** | **ABSENT** — only Sixel DCS; no APC / `_G` handling |
| **OSC 52 (clipboard)** | **ABSENT** — `osc_dispatch` (274–305) handles only 0/2/7/8/133; no 52 case. (The fuzz corpus *feeds* `\x1b]52;c;` as a hostile input at term.rs:619, but there is no handler — it's a no-op robustness seed, not a feature.) |
| **OSC 4 (palette set/query)** | **ABSENT** — not in `osc_dispatch` |
| **OSC 8 hyperlink id** | Incomplete — only the URI is stored; `id=` ignored |
| **DEC private modes (`?h`/`?l`)** | **ABSENT** — `csi_dispatch` (216–266) has no `set_mode`/`reset_mode`; the `Screen` struct has **no** `alt_screen`, `bracketed_paste`, `mouse_report`, `cursor_visible`, or `application_cursor` fields. So: **no alternate screen (1049), no bracketed paste (2004), no mouse reporting (1000/1006), no focus reporting (1004), no DECCKM**. |
| **DECSCUSR cursor shape (`CSI Ps SP q`)** | **ABSENT** in `term.rs` — no `set_cursor_shape`. (Cursor *style* is a config field `CursorConfig.style`, but the runtime escape is not parsed.) |
| **Mouse-report encoding** | **ABSENT** — no mode tracking and no event→escape encoder anywhere |
| **REVERSE rendering** | SGR `inverse` flag is parsed (7/27) into `Pen.flags`, but `leaf_spans` (window.rs:1317–1353) resolves **only fg** per cell — it does not swap fg/bg for inverse, and does not render bg color at all. So reverse-video is captured but not displayed. |
| **Copy/paste (clipboard)** | The palette has `Copy`-class chords (Ctrl+Shift+C/V via config), but the actual clipboard handlers are not in the dual-read window.rs regions; `key_to_bytes`/`write_input` is the PTY path. No `arboard`/clipboard crate seen. (Worth a focused re-check if clipboard is in scope.) |

---

## Cross-cutting facts

- **Crate graph:** `c0pl4nd-core` (config, term, grid, image, layout +
  layout/{action,geometry,mod,nav,ops,tree}, layout_persist, search,
  session, theme, pty, plugin, fetch, fuzzy) ← `c0pl4nd-renderer`
  (FramePolicy stub only) ← `c0pl4nd-app` (window / image_render /
  pane_render / drag / update / screenshot / main; bin `c0pl4nd`). Plus
  `fuzz/` (vt_parser target) and `crates/core/benches/throughput.rs`.
- **Frameless always** (`with_decorations(false)`, window.rs:1674) →
  the `TITLEBAR_H=30` custom chrome is always the active title bar.
- **Single render pass** (`label:"text"`), painters' order: pane chrome
  quads → glyphon text → per-leaf-scissored images → drag overlay quads.
  Both quad pipelines (`ChromeRenderer` solid, `ImageRenderer` textured)
  already exist and are wired in.
- **Theme** loaded by name (`load_theme(&config.theme)` → fallback
  `Theme::builtin_void`, window.rs:326); `Theme` exposes hex strings
  `.foreground` / `.background` / `.cursor` (used as accent) parsed by
  `c0pl4nd_core::theme::parse_hex`. (`theme/mod.rs` not audited beyond
  these call sites.)
- **Frame scheduling:** render-on-damage. `about_to_wait` (1963–1997)
  polls at ~60Hz for grid damage, redraws only when damaged; resize PTY
  debounced at ~30Hz via `pending_resize`.

## Key gaps the four tasks will hit

1. **No `Config` save/writer** (config/mod.rs) — add
   `toml::to_string_pretty` + atomic `fs::write` before any persisted
   settings or window geometry.
2. **Solid-color quad pipeline ALREADY EXISTS** —
   `pane_render::ChromeRenderer` + `ColorRect`
   (`prepare(device, w, h, &[ColorRect])` → `draw(pass, &prepared)`,
   per-vertex color, no bind groups). For caption-button backplates:
   build `ColorRect`s at the §4 button rects (`w-143`/`w-98`/`w-53`,
   45px wide, 0..30 tall), prepare them, and draw them in the render
   pass **before** `text_renderer.render` (between 2454 and 2455), or as
   an extra `chrome_renderer.draw` at step 1. Do NOT route them through
   `chrome_quads()` (it early-returns empty for a single pane).
3. **WindowConfig lacks position/maximized** (config/mod.rs:129–135) —
   add fields; restore via `with_position`/`with_maximized` in
   `resumed()` between 1676 and 1677; save on `Resized`/a new `Moved`
   handler. Needs the §1 writer.
4. **Live-reload is doc-claimed but NOT implemented** — only manual
   "Open Config File". A watcher (`notify` is not currently a dep) +
   re-`load_from` + buffer rebuild must be built.
5. **No in-app settings panel** — "Settings"/"Open Config File" both
   just open the file. Clone the `WorkspacePrompt` modal pattern
   (window.rs:206–211, 1136–1196) for an in-app settings overlay.
6. **Terminal feature gaps:** no DEC private modes at all (so no alt
   screen / bracketed paste / mouse reporting), no Kitty graphics, no
   OSC 52, no DECSCUSR escape, no inverse-video rendering (parsed not
   shown), no bg-color rendering in `leaf_spans`.

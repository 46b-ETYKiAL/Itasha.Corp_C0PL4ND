//! One-shot `--diagnostics` / `--doctor` report (finding F9-1).
//!
//! `--version` prints only the product + semver. This module adds a plain-text
//! environment + config dump that exits 0 without opening a window — the kind of
//! report a user pastes into a bug report. It is dependency-light: it reads the
//! process env + the persisted config + a handful of compile-time facts.
//!
//! The report body is built by the pure [`build_report`] function so it is
//! unit-testable without touching the real environment; [`run`] is the thin
//! shell that collects live inputs, prints, and returns the exit code.

use std::path::PathBuf;

/// Whether `--diagnostics` (or its `--doctor` alias) was requested on the CLI.
pub fn requested(args: &[String]) -> bool {
    args.iter().any(|a| a == "--diagnostics" || a == "--doctor")
}

/// The status of loading + validating the persisted config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigStatus {
    /// No config path could be resolved (no `%APPDATA%` / `$HOME`).
    NoPath,
    /// A config path is known but no file exists there (zero-config default).
    Absent,
    /// The file exists and parsed + validated cleanly.
    Loaded,
    /// The file exists but failed to parse/validate; carries the error text.
    Invalid(String),
}

/// All inputs the report renders. Collected from the live process by
/// [`collect`]; constructed directly by tests.
#[derive(Debug, Clone)]
pub struct Diagnostics {
    /// `c0pl4nd_core::version()`.
    pub version: String,
    /// `std::env::consts::OS`.
    pub os: String,
    /// `std::env::consts::ARCH`.
    pub arch: String,
    /// How the wgpu backend is chosen (the `WGPU_BACKEND` override, else the
    /// transparency-driven Windows default, else the platform default).
    pub wgpu_backend: String,
    /// `TERM` as the process sees it (the child shell inherits this).
    pub term: Option<String>,
    /// `COLORTERM` as the process sees it.
    pub colorterm: Option<String>,
    /// `TERM_PROGRAM` as the process sees it.
    pub term_program: Option<String>,
    /// Resolved config-file path (if any).
    pub config_path: Option<PathBuf>,
    /// Load/validate status of the config file.
    pub config_status: ConfigStatus,
    /// Reduced-motion state (the `C0PL4ND_REDUCED_MOTION` env convention).
    pub reduced_motion: bool,
    /// Whether IME / composed-text handling is compiled in (the egui app always
    /// routes `Event::Text`, so this is `true` for the egui binary).
    pub ime_compiled_in: bool,
    /// Directory crash logs are written to (the panic hook target).
    pub crash_log_dir: Option<PathBuf>,
}

/// Collect diagnostics from the live process. Reads env + persisted config; no
/// window or GPU device is created.
pub fn collect(ime_compiled_in: bool, crash_log_dir: Option<PathBuf>) -> Diagnostics {
    let config_path = c0pl4nd_core::Config::default_path();
    let config_status = config_status(config_path.as_deref());

    Diagnostics {
        version: c0pl4nd_core::version().to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        wgpu_backend: wgpu_backend_choice(),
        term: std::env::var("TERM").ok(),
        colorterm: std::env::var("COLORTERM").ok(),
        term_program: std::env::var("TERM_PROGRAM").ok(),
        config_path,
        config_status,
        reduced_motion: reduced_motion_enabled(),
        ime_compiled_in,
        crash_log_dir,
    }
}

/// Determine the config load/validate status for a (maybe) path.
fn config_status(path: Option<&std::path::Path>) -> ConfigStatus {
    let Some(path) = path else {
        return ConfigStatus::NoPath;
    };
    if !path.exists() {
        return ConfigStatus::Absent;
    }
    match std::fs::read_to_string(path) {
        Ok(src) => match c0pl4nd_core::Config::from_toml(&src, path) {
            Ok(_) => ConfigStatus::Loaded,
            Err(e) => ConfigStatus::Invalid(e.to_string()),
        },
        Err(e) => ConfigStatus::Invalid(e.to_string()),
    }
}

/// Describe how the wgpu backend is selected, matching `egui_main`'s logic: a
/// `WGPU_BACKEND` env override always wins; otherwise Windows picks Vulkan when
/// transparency is enabled and DX12 otherwise; other platforms use the wgpu
/// default. This is descriptive text, not a device handle (no GPU init).
fn wgpu_backend_choice() -> String {
    if let Ok(forced) = std::env::var("WGPU_BACKEND") {
        if !forced.is_empty() {
            return format!("{forced} (forced via WGPU_BACKEND)");
        }
    }
    if cfg!(target_os = "windows") {
        "DX12 default, Vulkan when window transparency is enabled (override with WGPU_BACKEND)"
            .to_string()
    } else {
        "wgpu platform default (override with WGPU_BACKEND)".to_string()
    }
}

/// Reduced-motion state — the `C0PL4ND_REDUCED_MOTION` env override OR the OS
/// accessibility setting (F2-2). Delegates to the single source of truth the
/// renderer also uses, so `--diagnostics` reports exactly what the CRT effect
/// honours.
fn reduced_motion_enabled() -> bool {
    c0pl4nd_core::reduced_motion::reduced_motion()
}

/// Build the plain-text diagnostics report. Pure: deterministic given its input.
pub fn build_report(d: &Diagnostics) -> String {
    let opt = |v: &Option<String>| v.clone().unwrap_or_else(|| "(unset)".to_string());
    let path = |p: &Option<PathBuf>| {
        p.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(unresolved)".to_string())
    };
    let config_status = match &d.config_status {
        ConfigStatus::NoPath => "no path resolved".to_string(),
        ConfigStatus::Absent => "absent (using built-in defaults)".to_string(),
        ConfigStatus::Loaded => "loaded + validated OK".to_string(),
        ConfigStatus::Invalid(_) => {
            "INVALID — settings file could not be parsed (see logs for details)".to_string()
        }
    };

    format!(
        "{product} diagnostics\n\
         version:        {version}\n\
         os/arch:        {os} / {arch}\n\
         wgpu backend:   {backend}\n\
         TERM:           {term}\n\
         COLORTERM:      {colorterm}\n\
         TERM_PROGRAM:   {term_program}\n\
         config path:    {config_path}\n\
         config status:  {config_status}\n\
         reduced motion: {reduced_motion} (C0PL4ND_REDUCED_MOTION)\n\
         IME handling:   {ime}\n\
         crash log dir:  {crash_dir}\n",
        product = c0pl4nd_core::PRODUCT_NAME,
        version = d.version,
        os = d.os,
        arch = d.arch,
        backend = d.wgpu_backend,
        term = opt(&d.term),
        colorterm = opt(&d.colorterm),
        term_program = opt(&d.term_program),
        config_path = path(&d.config_path),
        config_status = config_status,
        reduced_motion = d.reduced_motion,
        ime = if d.ime_compiled_in {
            "compiled in"
        } else {
            "not compiled in"
        },
        crash_dir = path(&d.crash_log_dir),
    )
}

/// Print the diagnostics report to stdout and return the process exit code (0).
/// `ime_compiled_in` is passed by the caller because it is a per-binary fact
/// (the egui app routes IME text; the legacy binary path differs).
pub fn run(ime_compiled_in: bool, crash_log_dir: Option<PathBuf>) -> i32 {
    let d = collect(ime_compiled_in, crash_log_dir);
    print!("{}", build_report(&d));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Diagnostics {
        Diagnostics {
            version: "9.9.9".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            wgpu_backend: "wgpu platform default".to_string(),
            term: Some("xterm-256color".to_string()),
            colorterm: Some("truecolor".to_string()),
            term_program: None,
            config_path: Some(PathBuf::from("/home/u/.config/c0pl4nd/config.toml")),
            config_status: ConfigStatus::Loaded,
            reduced_motion: true,
            ime_compiled_in: true,
            crash_log_dir: Some(PathBuf::from("/home/u/.config/c0pl4nd/crashes")),
        }
    }

    #[test]
    fn report_includes_all_key_fields() {
        let report = build_report(&sample());
        assert!(report.contains("C0PL4ND diagnostics"), "{report}");
        assert!(report.contains("9.9.9"), "version: {report}");
        assert!(report.contains("linux / x86_64"), "os/arch: {report}");
        assert!(report.contains("wgpu backend:"), "backend: {report}");
        assert!(report.contains("xterm-256color"), "TERM: {report}");
        assert!(report.contains("truecolor"), "COLORTERM: {report}");
        assert!(
            report.contains("config/c0pl4nd/config.toml"),
            "path: {report}"
        );
        assert!(report.contains("loaded + validated OK"), "status: {report}");
        assert!(report.contains("reduced motion: true"), "motion: {report}");
        assert!(
            report.contains("IME handling:   compiled in"),
            "ime: {report}"
        );
        assert!(report.contains("crashes"), "crash dir: {report}");
    }

    #[test]
    fn unset_env_renders_placeholder() {
        let mut d = sample();
        d.term_program = None;
        d.term = None;
        let report = build_report(&d);
        // The TERM_PROGRAM line must show the explicit placeholder.
        assert!(report.contains("TERM_PROGRAM:   (unset)"), "{report}");
        assert!(report.contains("TERM:           (unset)"), "{report}");
    }

    #[test]
    fn invalid_config_status_is_surfaced() {
        let mut d = sample();
        d.config_status = ConfigStatus::Invalid("bad opacity 2.0".to_string());
        let report = build_report(&d);
        assert!(
            report.contains("INVALID — settings file could not be parsed"),
            "{report}"
        );
        // The raw parse detail must NOT leak into the doctor output (it can carry
        // a path); it stays in the logs only.
        assert!(!report.contains("bad opacity 2.0"), "{report}");
    }

    #[test]
    fn requested_matches_both_flags() {
        assert!(requested(&["--diagnostics".to_string()]));
        assert!(requested(&["--doctor".to_string()]));
        assert!(requested(&["c0pl4nd".to_string(), "--doctor".to_string()]));
        assert!(!requested(&["--version".to_string()]));
        assert!(!requested(&[]));
    }

    #[test]
    fn run_returns_zero() {
        // Smoke: `run` prints and returns the success code without panicking.
        assert_eq!(run(true, Some(PathBuf::from("/tmp/x"))), 0);
    }

    // ---- config_status branch coverage (pure given a path) ------------------

    #[test]
    fn config_status_no_path_when_path_is_none() {
        assert_eq!(config_status(None), ConfigStatus::NoPath);
    }

    #[test]
    fn config_status_absent_when_file_missing() {
        // A path that does not exist on disk → Absent (zero-config default).
        let missing =
            std::env::temp_dir().join(format!("c0pl4nd-diag-absent-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        assert_eq!(config_status(Some(&missing)), ConfigStatus::Absent);
    }

    #[test]
    fn config_status_invalid_when_toml_is_malformed() {
        // A present-but-unparseable file → Invalid (carries the error text).
        let dir = std::env::temp_dir().join(format!("c0pl4nd-diag-invalid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("config.toml");
        // Not valid TOML at all.
        std::fs::write(&path, b"this is = = not valid toml [[[").expect("write");
        match config_status(Some(&path)) {
            ConfigStatus::Invalid(_) => {}
            other => panic!("expected Invalid for malformed TOML, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_status_loaded_when_toml_is_valid_default() {
        // An EMPTY config file is valid (every field has a default) → Loaded.
        let dir = std::env::temp_dir().join(format!("c0pl4nd-diag-loaded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("config.toml");
        std::fs::write(&path, b"").expect("write");
        assert_eq!(
            config_status(Some(&path)),
            ConfigStatus::Loaded,
            "an empty (all-default) config is valid → Loaded"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- wgpu_backend_choice env-driven branches ----------------------------

    use std::sync::Mutex;
    static WGPU_ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Scoped guard for the `WGPU_BACKEND` env var that restores it on drop.
    struct WgpuEnvGuard {
        prev: Option<String>,
    }
    impl WgpuEnvGuard {
        fn set(val: &str) -> Self {
            let prev = std::env::var("WGPU_BACKEND").ok();
            std::env::set_var("WGPU_BACKEND", val);
            Self { prev }
        }
        fn unset() -> Self {
            let prev = std::env::var("WGPU_BACKEND").ok();
            std::env::remove_var("WGPU_BACKEND");
            Self { prev }
        }
    }
    impl Drop for WgpuEnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("WGPU_BACKEND", v),
                None => std::env::remove_var("WGPU_BACKEND"),
            }
        }
    }

    #[test]
    fn wgpu_backend_choice_reports_forced_override() {
        let _lock = WGPU_ENV_LOCK.lock().unwrap();
        let _g = WgpuEnvGuard::set("vulkan");
        let choice = wgpu_backend_choice();
        assert!(
            choice.contains("vulkan") && choice.contains("forced via WGPU_BACKEND"),
            "a set WGPU_BACKEND must be reported as the forced backend: {choice}"
        );
    }

    #[test]
    fn wgpu_backend_choice_empty_override_falls_through_to_default() {
        let _lock = WGPU_ENV_LOCK.lock().unwrap();
        // An EMPTY WGPU_BACKEND is treated as unset → the platform default text,
        // NEVER the "forced" wording (a mutant that drops the is_empty guard is
        // caught here).
        let _g = WgpuEnvGuard::set("");
        let choice = wgpu_backend_choice();
        assert!(
            !choice.contains("forced via WGPU_BACKEND"),
            "an empty WGPU_BACKEND must not be reported as forced: {choice}"
        );
        assert!(
            choice.contains("override with WGPU_BACKEND"),
            "the default text invites the override: {choice}"
        );
    }

    #[test]
    fn wgpu_backend_choice_default_text_matches_platform() {
        let _lock = WGPU_ENV_LOCK.lock().unwrap();
        let _g = WgpuEnvGuard::unset();
        let choice = wgpu_backend_choice();
        assert!(!choice.contains("forced"), "unset → not forced: {choice}");
        if cfg!(target_os = "windows") {
            assert!(
                choice.contains("DX12") && choice.contains("Vulkan"),
                "windows default names DX12 + Vulkan-on-transparency: {choice}"
            );
        } else {
            assert!(
                choice.contains("wgpu platform default"),
                "non-windows default names the wgpu platform default: {choice}"
            );
        }
    }

    #[test]
    fn collect_produces_a_renderable_report_without_a_window() {
        // `collect` reads only env + config (no GPU/window). The report it
        // produces must render and name the live version + os/arch. This proves
        // the live-collection seam, not just the pure formatter.
        let d = collect(true, Some(PathBuf::from("/tmp/crashes")));
        assert_eq!(d.version, c0pl4nd_core::version());
        assert_eq!(d.os, std::env::consts::OS);
        assert_eq!(d.arch, std::env::consts::ARCH);
        let report = build_report(&d);
        assert!(report.contains("C0PL4ND diagnostics"));
        assert!(report.contains(std::env::consts::OS));
    }
}

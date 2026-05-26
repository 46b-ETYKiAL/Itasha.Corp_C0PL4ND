//! Startup fetch panel — a neofetch/fastfetch-style splash.
//!
//! Gathers local system info ([`SystemInfo`]) and renders an Itasha.Corp ASCII
//! logo on the left with a column of stats on the right ([`render_panel`]).
//! Privacy: reads only local system facts and returns text — it never touches
//! the network, the PTY input, or anything identifying beyond host/user that
//! the user already sees in their own prompt. Gated by config `[startup_panel]`.

/// Structured, display-only system facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    pub os: String,
    pub kernel: String,
    pub host: String,
    pub uptime: String,
    pub shell: String,
    pub terminal: String,
    pub cpu: String,
    pub memory: String,
    pub gpu: String,
}

fn fmt_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

fn fmt_mem(used: u64, total: u64) -> String {
    let gib = |b: u64| b as f64 / 1_073_741_824.0;
    if total == 0 {
        return "n/a".into();
    }
    format!("{:.1} / {:.1} GiB", gib(used), gib(total))
}

impl SystemInfo {
    /// Collect facts from the local system. `gpu` is supplied by the caller
    /// (the renderer already created a wgpu adapter — no extra probe needed).
    pub fn gather(gpu: Option<&str>) -> SystemInfo {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.refresh_cpu_all();

        let cpu = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());

        let shell = std::env::var("SHELL")
            .or_else(|_| std::env::var("COMSPEC"))
            .ok()
            .and_then(|p| {
                std::path::Path::new(&p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unknown".into());

        SystemInfo {
            os: sysinfo::System::long_os_version().unwrap_or_else(|| "unknown".into()),
            kernel: sysinfo::System::kernel_version().unwrap_or_else(|| "unknown".into()),
            host: sysinfo::System::host_name().unwrap_or_else(|| "unknown".into()),
            uptime: fmt_uptime(sysinfo::System::uptime()),
            shell,
            terminal: format!("{} {}", crate::PRODUCT_NAME, crate::version()),
            cpu,
            memory: fmt_mem(sys.used_memory(), sys.total_memory()),
            gpu: gpu.unwrap_or("unknown").to_string(),
        }
    }

    /// A fixed instance for tests / previews (no system reads).
    pub fn demo() -> SystemInfo {
        SystemInfo {
            os: "Windows 11 Pro".into(),
            kernel: "10.0.26200".into(),
            host: "wired".into(),
            uptime: "3h 14m".into(),
            shell: "pwsh".into(),
            terminal: "C0PL4ND 0.1.0".into(),
            cpu: "Operator CPU".into(),
            memory: "9.4 / 32.0 GiB".into(),
            gpu: "Signal GPU".into(),
        }
    }
}

/// The Itasha.Corp / C0PL4ND ASCII logo (left column of the panel).
pub const LOGO: &str = r#"   ___ ___ ___ _    _ _  _ ___
  / __/ _ \  _ \ |  | | || |   \
 | (_| (_) |  _/ |__| |__  | |) |
  \___\___/|_| |____|_| |_|___/
  >_ the operator's shell
     into the wired"#;

/// Render the fetch panel as PLAIN text: the logo on the left, `label : value`
/// stats on the right, one line each.
///
/// Plain (no ANSI) by design. C0PL4ND draws this as an app-rendered startup
/// overlay and applies the theme colours itself, because on Windows the shell's
/// ConPTY repaint clears anything written into the terminal grid before the
/// shell starts. `core` stays UI-agnostic: this owns layout, the renderer owns
/// colour.
pub fn render_panel(info: &SystemInfo) -> String {
    let rows = [
        ("os", &info.os),
        ("kernel", &info.kernel),
        ("host", &info.host),
        ("uptime", &info.uptime),
        ("shell", &info.shell),
        ("term", &info.terminal),
        ("cpu", &info.cpu),
        ("memory", &info.memory),
        ("gpu", &info.gpu),
    ];

    let logo_lines: Vec<&str> = LOGO.lines().collect();
    let logo_w = logo_lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    let n = logo_lines.len().max(rows.len());

    let mut out = String::new();
    for i in 0..n {
        let logo = logo_lines.get(i).copied().unwrap_or("");
        out.push_str(logo);
        // Pad the logo column so the stats line up in a second column.
        for _ in logo.chars().count()..(logo_w + 4) {
            out.push(' ');
        }
        if let Some((label, value)) = rows.get(i) {
            out.push_str(&format!("{label:>7} : {value}"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_formats() {
        assert_eq!(fmt_uptime(0), "0m");
        assert_eq!(fmt_uptime(3 * 3600 + 14 * 60), "3h 14m");
        assert_eq!(fmt_uptime(2 * 86400 + 3600), "2d 1h 0m");
    }

    #[test]
    fn mem_formats() {
        assert_eq!(fmt_mem(0, 0), "n/a");
        assert!(fmt_mem(1_073_741_824, 2_147_483_648).contains("1.0 / 2.0 GiB"));
    }

    #[test]
    fn panel_contains_logo_and_stats() {
        let p = render_panel(&SystemInfo::demo());
        assert!(p.contains("the operator's shell"));
        assert!(p.contains("os"));
        assert!(p.contains("Windows 11 Pro"));
        // Plain text by design: the renderer colours the overlay, so the panel
        // itself must carry NO ANSI escapes (they'd render as literal glyphs).
        assert!(!p.contains('\x1b'), "panel must be plain text, no ANSI");
    }

    #[test]
    fn gather_does_not_panic() {
        let info = SystemInfo::gather(Some("Test GPU"));
        assert_eq!(info.gpu, "Test GPU");
        assert!(!info.terminal.is_empty());
    }
}

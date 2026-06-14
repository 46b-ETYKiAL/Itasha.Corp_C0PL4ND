//! Extension/plugin system (declarative layer).
//!
//! C0PL4ND extensions live in their own folder under the user plugins
//! directory, each carrying an `extension.toml` manifest. The declarative
//! layer lets a plugin contribute **themes** and **keybinding overrides** —
//! which is what the large majority of real-world "extensions" are — with a
//! capability block that is **default-deny** for anything dangerous.
//!
//! The capability model is forward-compatible with a sandboxed code layer: the
//! manifest declares an `api_version` and a `[capabilities]` table, and the
//! loader verifies both. A declarative plugin can never execute code, write to
//! the PTY, touch the filesystem outside its folder, or open the network — by
//! construction it only contributes data.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Host extension-API version. Plugins declare the `api_version` they target;
/// the loader accepts a plugin when its major version matches the host.
pub const PLUGIN_API_VERSION: &str = "0.1.0";

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("plugin manifest not found: {0}")]
    NoManifest(PathBuf),
    #[error("plugin manifest parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("plugin {id} targets incompatible api_version {got} (host {host})")]
    ApiMismatch {
        id: String,
        got: String,
        host: String,
    },
    #[error("plugin {id} declares a contributed path that escapes its folder: {path}")]
    PathEscape { id: String, path: String },
}

/// Capability grants. Dangerous capabilities default to `false` (deny). The
/// declarative layer never honours the dangerous ones at runtime; they exist so
/// a manifest is forward-compatible and so the loader can warn on over-ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Capabilities {
    pub pty_write: bool,
    pub filesystem: bool,
    pub network: bool,
    pub process_spawn: bool,
}

impl Capabilities {
    /// True if the manifest requests any capability the declarative layer
    /// cannot grant (informational — the loader records a warning).
    pub fn requests_dangerous(&self) -> bool {
        self.pty_write || self.filesystem || self.network || self.process_spawn
    }
}

/// What a plugin contributes (declarative).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Contributes {
    /// Theme files (relative to the plugin folder).
    pub themes: Vec<String>,
    /// Optional keybinding-override file (relative to the plugin folder).
    pub keybindings: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    pub api_version: String,
}

/// A parsed `extension.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub extension: ExtensionMeta,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub contributes: Contributes,
}

impl ExtensionManifest {
    pub fn from_toml(src: &str, path: &Path) -> Result<ExtensionManifest, PluginError> {
        toml::from_str(src).map_err(|e| PluginError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }
}

/// A discovered, validated plugin and the folder it lives in.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: ExtensionManifest,
    pub dir: PathBuf,
    /// True when the manifest over-asked for capabilities the layer can't grant.
    pub over_asked: bool,
}

impl Plugin {
    /// Absolute paths of the theme files this plugin contributes.
    pub fn theme_paths(&self) -> Vec<PathBuf> {
        self.manifest
            .contributes
            .themes
            .iter()
            .map(|t| self.dir.join(t))
            .collect()
    }

    /// Absolute path of the keybinding override file, if any.
    pub fn keybinding_path(&self) -> Option<PathBuf> {
        self.manifest
            .contributes
            .keybindings
            .as_ref()
            .map(|k| self.dir.join(k))
    }
}

/// Major version component of a `major.minor.patch` string.
fn major(v: &str) -> u64 {
    v.split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Verify a contributed relative path stays inside the plugin folder.
fn is_contained(rel: &str) -> bool {
    // Reject `..` traversal and absolute paths. Also reject any `:` so a Windows
    // DRIVE-RELATIVE path (`C:theme.toml`) — which `Path::is_absolute()` reports
    // as NON-absolute, yet `dir.join("C:theme.toml")` resolves against the C:
    // drive's current directory rather than the plugin folder — cannot escape the
    // plugin dir. (UNC `\\srv\share` and rooted `\x` forms are already caught by
    // `is_absolute()`.) Defense-in-depth: the plugin layer is declarative-only.
    !rel.contains("..") && !rel.contains(':') && !Path::new(rel).is_absolute()
}

/// Load and validate a single plugin folder (containing `extension.toml`).
pub fn load_plugin(dir: &Path) -> Result<Plugin, PluginError> {
    let manifest_path = dir.join("extension.toml");
    let src = std::fs::read_to_string(&manifest_path)
        .map_err(|_| PluginError::NoManifest(manifest_path.clone()))?;
    let manifest = ExtensionManifest::from_toml(&src, &manifest_path)?;

    if major(&manifest.extension.api_version) != major(PLUGIN_API_VERSION) {
        return Err(PluginError::ApiMismatch {
            id: manifest.extension.id.clone(),
            got: manifest.extension.api_version.clone(),
            host: PLUGIN_API_VERSION.to_string(),
        });
    }

    for rel in &manifest.contributes.themes {
        if !is_contained(rel) {
            return Err(PluginError::PathEscape {
                id: manifest.extension.id.clone(),
                path: rel.clone(),
            });
        }
    }
    if let Some(k) = &manifest.contributes.keybindings {
        if !is_contained(k) {
            return Err(PluginError::PathEscape {
                id: manifest.extension.id.clone(),
                path: k.clone(),
            });
        }
    }

    let over_asked = manifest.capabilities.requests_dangerous();
    Ok(Plugin {
        manifest,
        dir: dir.to_path_buf(),
        over_asked,
    })
}

/// Discover every plugin under `plugins_dir`. Folders without a manifest, or
/// with an invalid one, are skipped (with the error returned per-folder).
pub fn discover(plugins_dir: &Path) -> Vec<Result<Plugin, PluginError>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(plugins_dir) {
        Ok(e) => e,
        Err(_) => return out, // no plugins dir → no plugins (not an error)
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("extension.toml").exists() {
            out.push(load_plugin(&path));
        }
    }
    out
}

/// The default per-user plugins directory.
pub fn default_plugins_dir() -> Option<PathBuf> {
    crate::config::Config::default_path().and_then(|p| p.parent().map(|d| d.join("plugins")))
}

/// A loaded set of valid plugins with merged contributions.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    pub plugins: Vec<Plugin>,
}

impl PluginRegistry {
    pub fn load(plugins_dir: &Path) -> PluginRegistry {
        let plugins = discover(plugins_dir)
            .into_iter()
            .filter_map(|r| r.ok())
            .collect();
        PluginRegistry { plugins }
    }

    /// All theme files contributed by all valid plugins.
    pub fn contributed_theme_paths(&self) -> Vec<PathBuf> {
        self.plugins.iter().flat_map(|p| p.theme_paths()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, content: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn capabilities_default_to_deny() {
        let c = Capabilities::default();
        assert!(!c.requests_dangerous());
    }

    #[test]
    fn is_contained_rejects_escapes_including_windows_drive_relative() {
        // Plain relative paths inside the plugin folder are allowed.
        assert!(is_contained("themes/neon.toml"));
        assert!(is_contained("keybindings.toml"));
        // Traversal + absolute forms are rejected.
        assert!(!is_contained("../evil.toml"));
        assert!(!is_contained("a/../../evil.toml"));
        // Windows drive-relative `C:theme.toml` is NON-absolute per
        // `Path::is_absolute()` yet escapes via the drive's cwd — rejected by the
        // `:` guard. (Also covers drive-absolute `C:\...` and UNC `\\srv\share`.)
        assert!(!is_contained("C:theme.toml"));
        assert!(!is_contained(r"C:\Windows\evil.toml"));
        assert!(!is_contained(r"\\server\share\evil.toml"));
    }

    #[test]
    fn loads_a_declarative_theme_plugin() {
        let tmp = std::env::temp_dir().join(format!("c0pl4nd-plug-{}", std::process::id()));
        let dir = tmp.join("neon-pack");
        write(
            &dir,
            "extension.toml",
            r#"
[extension]
id = "neon-pack"
name = "Neon Pack"
version = "0.1.0"
api_version = "0.1.0"
[contributes]
themes = ["themes/neon.toml"]
"#,
        );
        write(&dir.join("themes"), "neon.toml", "name=\"neon\"\n");
        let plugin = load_plugin(&dir).expect("load plugin");
        assert_eq!(plugin.manifest.extension.id, "neon-pack");
        assert_eq!(plugin.theme_paths().len(), 1);
        assert!(!plugin.over_asked);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rejects_incompatible_api_version() {
        let tmp = std::env::temp_dir().join(format!("c0pl4nd-plug-api-{}", std::process::id()));
        let dir = tmp.join("future");
        write(
            &dir,
            "extension.toml",
            r#"
[extension]
id = "future"
name = "Future"
version = "1.0.0"
api_version = "9.0.0"
"#,
        );
        assert!(matches!(
            load_plugin(&dir),
            Err(PluginError::ApiMismatch { .. })
        ));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rejects_path_escape() {
        let tmp = std::env::temp_dir().join(format!("c0pl4nd-plug-esc-{}", std::process::id()));
        let dir = tmp.join("evil");
        write(
            &dir,
            "extension.toml",
            r#"
[extension]
id = "evil"
name = "Evil"
version = "0.1.0"
api_version = "0.1.0"
[contributes]
themes = ["../../../../etc/passwd"]
"#,
        );
        assert!(matches!(
            load_plugin(&dir),
            Err(PluginError::PathEscape { .. })
        ));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn over_ask_is_flagged_not_fatal() {
        let tmp = std::env::temp_dir().join(format!("c0pl4nd-plug-cap-{}", std::process::id()));
        let dir = tmp.join("greedy");
        write(
            &dir,
            "extension.toml",
            r#"
[extension]
id = "greedy"
name = "Greedy"
version = "0.1.0"
api_version = "0.1.0"
[capabilities]
network = true
"#,
        );
        let plugin = load_plugin(&dir).expect("load");
        assert!(
            plugin.over_asked,
            "network grant must be flagged as over-ask"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_skips_non_plugin_folders() {
        let tmp = std::env::temp_dir().join(format!("c0pl4nd-disc-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("not-a-plugin")).unwrap();
        let found = discover(&tmp);
        assert!(found.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

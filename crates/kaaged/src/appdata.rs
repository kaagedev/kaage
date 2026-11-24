use kaage_proto::{AppDataDirInfo, AppDataSnapshot};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
};

pub const DEFAULT_APPDATA_DISPLAY: &str = "~/appdata";

static ACTIVE_APPDATA_DIR: OnceLock<RwLock<PathBuf>> = OnceLock::new();

fn active_dir_lock() -> &'static RwLock<PathBuf> {
    ACTIVE_APPDATA_DIR.get_or_init(|| {
        let cfg = default_config_path();
        let (dir, _) = resolve_initial_appdata_dir(&cfg);
        RwLock::new(dir)
    })
}

/// Returns the currently active `KAAGE_APPDATA` base directory (default: `~/appdata`).
pub fn active_appdata_dir() -> PathBuf {
    active_dir_lock()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Returns kaage's own appdata directory (`$KAAGE_APPDATA/kaage`), used for vitals, trash, and workspaces.
pub fn kaage_app_dir() -> PathBuf {
    active_appdata_dir().join("kaage")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedAppDataConfig {
    appdata_dir: String,
}

pub struct AppDataManager {
    inner: RwLock<AppDataInner>,
    config_file: PathBuf,
}

struct AppDataInner {
    appdata_dir: PathBuf,
    from_env: bool,
}

impl AppDataManager {
    pub fn new() -> Arc<Self> {
        let config_file = default_config_path();
        let (appdata_dir, from_env) = resolve_initial_appdata_dir(&config_file);
        if let Err(err) = fs::create_dir_all(&appdata_dir) {
            tracing::warn!(
                "Failed to ensure KAAGE_APPDATA dir {}: {err}",
                appdata_dir.display()
            );
        }
        let _ = fs::create_dir_all(appdata_dir.join("kaage"));
        *active_dir_lock().write().unwrap_or_else(|e| e.into_inner()) = appdata_dir.clone();
        Arc::new(Self {
            inner: RwLock::new(AppDataInner {
                appdata_dir,
                from_env,
            }),
            config_file,
        })
    }

    #[cfg(test)]
    pub fn new_for_test(appdata_dir: PathBuf, config_file: PathBuf) -> Arc<Self> {
        let _ = fs::create_dir_all(&appdata_dir);
        Arc::new(Self {
            inner: RwLock::new(AppDataInner {
                appdata_dir,
                from_env: false,
            }),
            config_file,
        })
    }

    pub fn current_dir(&self) -> PathBuf {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .appdata_dir
            .clone()
    }

    pub fn set_dir(&self, raw_path: &str) -> Result<AppDataSnapshot, String> {
        let trimmed = raw_path.trim();
        if trimmed.is_empty() {
            return Err("KAAGE_APPDATA path cannot be empty".into());
        }
        let expanded = expand_user_path(trimmed);
        fs::create_dir_all(&expanded)
            .map_err(|e| format!("Failed to create {}: {e}", expanded.display()))?;

        {
            let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
            guard.appdata_dir = expanded.clone();
            guard.from_env = false;
        }
        let _ = fs::create_dir_all(expanded.join("kaage"));
        *active_dir_lock().write().unwrap_or_else(|e| e.into_inner()) = expanded.clone();

        if let Some(parent) = self.config_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let persisted = PersistedAppDataConfig {
            appdata_dir: expanded.display().to_string(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&persisted) {
            let _ = fs::write(&self.config_file, json);
        }

        Ok(self.snapshot())
    }

    pub fn create_app_dir(&self, app_name: &str) -> Result<PathBuf, String> {
        validate_app_name(app_name)?;
        let target = self.current_dir().join(app_name.trim());
        fs::create_dir_all(&target)
            .map_err(|e| format!("Failed to create {}: {e}", target.display()))?;
        Ok(target)
    }

    pub fn snapshot(&self) -> AppDataSnapshot {
        let (dir, from_env) = {
            let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
            (guard.appdata_dir.clone(), guard.from_env)
        };
        let exists = dir.is_dir();
        let mut apps = Vec::new();

        if exists && let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let app_name = entry.file_name().to_string_lossy().into_owned();
                if app_name.starts_with('.') {
                    continue;
                }
                let (size_bytes, file_count) = dir_stats(&path, 0);
                apps.push(AppDataDirInfo {
                    app_name,
                    path: path.display().to_string(),
                    size_bytes,
                    file_count,
                });
            }
        }
        apps.sort_by(|a, b| a.app_name.cmp(&b.app_name));

        AppDataSnapshot {
            appdata_dir: dir.display().to_string(),
            exists,
            from_env,
            apps,
        }
    }
}

pub fn validate_app_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("App name cannot be empty".into());
    }
    if trimmed.starts_with('.') || trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err("App name must be a simple folder name without paths or leading dots".into());
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("App name may only contain letters, digits, '.', '-', and '_'".into());
    }
    Ok(())
}

fn default_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg).join("kaage").join("appdata.json");
    }
    expand_user_path("~/.config/kaage/appdata.json")
}

fn resolve_initial_appdata_dir(config_file: &Path) -> (PathBuf, bool) {
    if let Ok(val) = std::env::var("KAAGE_APPDATA")
        && !val.trim().is_empty()
    {
        return (expand_user_path(val.trim()), true);
    }
    if let Ok(raw) = fs::read_to_string(config_file)
        && let Ok(cfg) = serde_json::from_str::<PersistedAppDataConfig>(&raw)
        && !cfg.appdata_dir.trim().is_empty()
    {
        return (expand_user_path(cfg.appdata_dir.trim()), false);
    }
    (expand_user_path(DEFAULT_APPDATA_DISPLAY), false)
}

pub fn expand_user_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed == "~" {
        return home_dir();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    if trimmed.is_empty() {
        return PathBuf::from("/");
    }
    PathBuf::from(trimmed)
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn dir_stats(dir: &Path, depth: usize) -> (u64, u64) {
    if depth > 6 {
        return (0, 0);
    }
    let mut size = 0u64;
    let mut files = 0u64;
    let Ok(entries) = fs::read_dir(dir) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_file() {
            size = size.saturating_add(meta.len());
            files = files.saturating_add(1);
        } else if meta.is_dir() && !meta.file_type().is_symlink() {
            let (sub_size, sub_files) = dir_stats(&entry.path(), depth + 1);
            size = size.saturating_add(sub_size);
            files = files.saturating_add(sub_files);
        }
    }
    (size, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_default_appdata_dir_to_home_appdata() {
        let expanded = expand_user_path("~/appdata/");
        assert!(expanded.ends_with("appdata"));
        assert!(!expanded.display().to_string().contains('~'));
    }

    #[test]
    fn validates_app_names_and_rejects_traversal() {
        assert!(validate_app_name("notes").is_ok());
        assert!(validate_app_name("my-app_01.data").is_ok());
        assert!(validate_app_name("").is_err());
        assert!(validate_app_name("../escape").is_err());
        assert!(validate_app_name("a/b").is_err());
        assert!(validate_app_name(".hidden").is_err());
        assert!(validate_app_name("bad name!").is_err());
    }

    #[test]
    fn creates_and_discovers_app_subdirectories() {
        let base = std::env::temp_dir().join(format!("kaage_appdata_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let cfg = base.join("_cfg.json");
        let root = base.join("appdata");
        let mgr = AppDataManager::new_for_test(root.clone(), cfg);

        let created = mgr.create_app_dir("notes").expect("create notes");
        fs::write(created.join("hello.txt"), b"hello world").unwrap();

        let snap = mgr.snapshot();
        assert!(snap.exists);
        assert_eq!(snap.apps.len(), 1);
        assert_eq!(snap.apps[0].app_name, "notes");
        assert_eq!(snap.apps[0].file_count, 1);
        assert_eq!(snap.apps[0].size_bytes, 11);

        let _ = fs::remove_dir_all(&base);
    }
}

use kaage_proto::{NixosInfo, SystemdUnitInfo, UnitScope};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

/// Raw tuple returned by `org.freedesktop.systemd1.Manager.ListUnits`.
#[derive(Debug, Clone)]
pub struct LoadedUnitRecord {
    pub name: String,
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
}

/// Raw tuple returned by `org.freedesktop.systemd1.Manager.ListUnitFiles`.
#[derive(Debug, Clone)]
pub struct UnitFileRecord {
    pub path: String,
    pub state: String,
}

/// Detect whether the host is NixOS along with its version, active system store path, and generation.
pub fn detect_nixos_info() -> NixosInfo {
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let current_system = fs::read_link("/run/current-system")
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let generation = fs::read_link("/nix/var/nix/profiles/system")
        .ok()
        .and_then(|p| parse_generation_from_link(&p.to_string_lossy()));

    build_nixos_info(&os_release, current_system, generation)
}

pub fn build_nixos_info(
    os_release: &str,
    current_system: Option<String>,
    generation: Option<u64>,
) -> NixosInfo {
    let fields = parse_os_release(os_release);
    let id = fields.get("ID").map(String::as_str).unwrap_or("");
    let is_nixos = id == "nixos" || current_system.is_some();
    let pretty_name = fields
        .get("PRETTY_NAME")
        .or_else(|| fields.get("NAME"))
        .cloned()
        .unwrap_or_else(|| {
            if is_nixos {
                "NixOS".into()
            } else {
                "Linux".into()
            }
        });
    let version = fields
        .get("VERSION")
        .or_else(|| fields.get("VERSION_ID"))
        .cloned()
        .unwrap_or_default();

    NixosInfo {
        is_nixos,
        pretty_name,
        version,
        current_system,
        generation,
    }
}

pub fn parse_os_release(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, val)) = trimmed.split_once('=') else {
            continue;
        };
        let val = val.trim();
        let unquoted = val
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| val.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(val);
        map.insert(key.trim().to_string(), unquoted.to_string());
    }
    map
}

/// Parse generation number from a NixOS profile symlink like `system-42-link` or `/nix/var/nix/profiles/system-42-link`.
pub fn parse_generation_from_link(link: &str) -> Option<u64> {
    let base = Path::new(link).file_name()?.to_str()?;
    let rest = base.strip_prefix("system-")?.strip_suffix("-link")?;
    rest.parse::<u64>().ok()
}

/// Normalize a user-supplied unit name so `kaaged` -> `kaaged.service`,
/// while preserving explicit suffixes like `kaaged.service` or `fstrim.timer`.
pub fn normalize_service_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains('.') {
        trimmed.to_string()
    } else {
        format!("{trimmed}.service")
    }
}

/// Returns true if a systemd `UnitFileState` indicates the service is enabled.
pub fn is_unit_file_enabled(unit_file_state: &str) -> bool {
    matches!(unit_file_state, "enabled" | "enabled-runtime")
}

/// Returns true if a systemd service is actively running.
pub fn is_unit_running(active_state: &str, sub_state: &str) -> bool {
    active_state == "active" && sub_state == "running"
}

/// Resolve a unit file path (following symlinks) and return it if it lives in `/nix/store/`.
pub fn resolve_nix_store_path(unit_path: &str) -> Option<String> {
    if unit_path.is_empty() {
        return None;
    }
    if unit_path.starts_with("/nix/store/") {
        return Some(unit_path.to_string());
    }
    let resolved: PathBuf = fs::canonicalize(unit_path).ok()?;
    let resolved_str = resolved.to_string_lossy().into_owned();
    if resolved_str.starts_with("/nix/store/") {
        Some(resolved_str)
    } else {
        None
    }
}

/// Merge loaded systemd units (`ListUnits`) with installed unit files (`ListUnitFiles`)
/// so kaage reports enabled/disabled/static/linked/masked states as well as unloaded services,
/// while filtering out NixOS `not-found` phantom units and template `@.service` definitions.
pub fn merge_units_and_files(
    loaded: Vec<LoadedUnitRecord>,
    unit_files: Vec<UnitFileRecord>,
    scope: UnitScope,
    resolve_store: &dyn Fn(&str) -> Option<String>,
) -> Vec<SystemdUnitInfo> {
    let mut by_name: HashMap<String, SystemdUnitInfo> = HashMap::new();

    for u in loaded {
        if !u.name.ends_with(".service") || u.name.ends_with("@.service") {
            continue;
        }
        if u.load_state == "not-found" {
            continue;
        }
        let running = is_unit_running(&u.active_state, &u.sub_state);
        by_name.insert(
            u.name.clone(),
            SystemdUnitInfo {
                name: u.name,
                description: u.description,
                load_state: u.load_state,
                active_state: u.active_state,
                sub_state: u.sub_state,
                unit_file_state: String::new(),
                enabled: false,
                running,
                nixos_managed: false,
                scope,
            },
        );
    }

    for f in unit_files {
        let Some(name) = Path::new(&f.path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if !name.ends_with(".service") || name.ends_with("@.service") {
            continue;
        }

        let nixos_managed = resolve_store(&f.path).is_some();
        let enabled = is_unit_file_enabled(&f.state);

        if let Some(existing) = by_name.get_mut(&name) {
            if existing.unit_file_state.is_empty() || existing.unit_file_state == "alias" {
                existing.unit_file_state = f.state;
                existing.enabled = enabled;
            }
            if nixos_managed {
                existing.nixos_managed = true;
            }
        } else {
            if f.state == "alias" {
                continue;
            }
            by_name.insert(
                name.clone(),
                SystemdUnitInfo {
                    name,
                    description: String::new(),
                    load_state: "unloaded".into(),
                    active_state: "inactive".into(),
                    sub_state: "dead".into(),
                    unit_file_state: f.state,
                    enabled,
                    running: false,
                    nixos_managed,
                    scope,
                },
            );
        }
    }

    let mut result: Vec<SystemdUnitInfo> = by_name.into_values().collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nixos_os_release_and_generation() {
        let os_release = r#"
ID=nixos
NAME=NixOS
PRETTY_NAME="NixOS 26.11 (Zokor)"
VERSION="26.11 (Zokor)"
VERSION_ID="26.11"
"#;
        let generation = parse_generation_from_link("/nix/var/nix/profiles/system-42-link");
        assert_eq!(generation, Some(42));

        let info = build_nixos_info(
            os_release,
            Some("/nix/store/abc123-nixos-system-aihole-26.11".into()),
            generation,
        );
        assert!(info.is_nixos);
        assert_eq!(info.pretty_name, "NixOS 26.11 (Zokor)");
        assert_eq!(info.version, "26.11 (Zokor)");
        assert_eq!(info.generation, Some(42));
    }

    #[test]
    fn normalizes_service_names() {
        assert_eq!(normalize_service_name("kaaged"), "kaaged.service");
        assert_eq!(normalize_service_name("kaaged.service"), "kaaged.service");
        assert_eq!(normalize_service_name("fstrim.timer"), "fstrim.timer");
        assert_eq!(normalize_service_name("  acedop  "), "acedop.service");
    }

    #[test]
    fn merges_loaded_units_and_unit_files_for_nixos() {
        let loaded = vec![
            LoadedUnitRecord {
                name: "kaaged.service".into(),
                description: "kaage daemon".into(),
                load_state: "loaded".into(),
                active_state: "active".into(),
                sub_state: "running".into(),
            },
            LoadedUnitRecord {
                name: "auditd.service".into(),
                description: "auditd.service".into(),
                load_state: "not-found".into(),
                active_state: "inactive".into(),
                sub_state: "dead".into(),
            },
        ];

        let files = vec![
            UnitFileRecord {
                path: "/etc/systemd/system/kaaged.service".into(),
                state: "enabled".into(),
            },
            UnitFileRecord {
                path: "/etc/systemd/system/cups.service".into(),
                state: "linked".into(),
            },
            UnitFileRecord {
                path: "/etc/systemd/system/autovt@.service".into(),
                state: "alias".into(),
            },
            UnitFileRecord {
                path: "/etc/systemd/system/dbus-org.bluez.service".into(),
                state: "alias".into(),
            },
            UnitFileRecord {
                path: "/etc/systemd/system/custom-disabled.service".into(),
                state: "disabled".into(),
            },
        ];

        let merged = merge_units_and_files(loaded, files, UnitScope::System, &|p| {
            if p.starts_with("/etc/systemd/system/") {
                Some(format!("/nix/store/fake-{p}"))
            } else {
                None
            }
        });

        assert_eq!(merged.len(), 3);

        let cups = merged.iter().find(|u| u.name == "cups.service").unwrap();
        assert_eq!(cups.unit_file_state, "linked");
        assert!(!cups.enabled);
        assert!(!cups.running);
        assert!(cups.nixos_managed);

        let disabled = merged
            .iter()
            .find(|u| u.name == "custom-disabled.service")
            .unwrap();
        assert_eq!(disabled.unit_file_state, "disabled");
        assert!(!disabled.enabled);
        assert!(!disabled.running);

        let kaaged = merged.iter().find(|u| u.name == "kaaged.service").unwrap();
        assert_eq!(kaaged.unit_file_state, "enabled");
        assert!(kaaged.enabled);
        assert!(kaaged.running);
        assert!(kaaged.nixos_managed);
    }
}

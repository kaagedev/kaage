use axum::{
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use kaage_proto::{
    DirListing, FileEntry, FileKind, FileOp, GitDirStatus, PreviewContent, WorkspaceDef,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::UNIX_EPOCH,
};
use tokio_util::io::ReaderStream;

pub struct UndoRecord {
    pub description: String,
    /// Directory to refresh after undo
    pub refresh_dir: Option<PathBuf>,
    /// Pairs of (current_location, original_location) to move back
    pub moves: Vec<(PathBuf, PathBuf)>,
    /// Paths created by Copy/Mkdir to remove on undo
    pub created: Vec<PathBuf>,
}

pub fn classify_kind(path: &Path, is_dir: bool) -> FileKind {
    if is_dir {
        return FileKind::Directory;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" => FileKind::Markdown,
        "pdf" | "doc" | "docx" | "txt" | "rtf" | "org" | "csv" | "xlsx" => FileKind::Document,
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "heic" => FileKind::Image,
        "mp4" | "mov" | "mkv" | "webm" | "avi" => FileKind::Video,
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => FileKind::Audio,
        "zip" | "tar" | "gz" | "zst" | "xz" | "7z" | "rar" => FileKind::Archive,
        "safetensors" | "gguf" | "ckpt" | "pt" | "pth" | "onnx" | "bin" => FileKind::ModelWeights,
        "rs" | "py" | "ts" | "tsx" | "js" | "nix" | "toml" | "yaml" | "yml" | "json" | "sh"
        | "c" | "cpp" | "h" | "go" | "zig" => FileKind::Code,
        "deb" | "rpm" | "appimage" | "pkg" | "dmg" => FileKind::Installer,
        _ => FileKind::Other,
    }
}

pub fn expand_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if (trimmed == "~" || trimmed.starts_with("~/"))
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(trimmed.replacen('~', &home, 1));
    }
    PathBuf::from(trimmed)
}

/// Maps canonical file paths -> systemd unit name by inspecting `/proc/<pid>/cgroup` & `/proc/<pid>/fd`.
fn scan_systemd_open_files() -> HashMap<PathBuf, String> {
    let mut map = HashMap::new();
    let Ok(proc_dir) = fs::read_dir("/proc") else {
        return map;
    };
    for pid_entry in proc_dir.flatten() {
        let fname = pid_entry.file_name();
        let pid_str = fname.to_string_lossy();
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let cgroup_path = pid_entry.path().join("cgroup");
        let Ok(cgroup) = fs::read_to_string(&cgroup_path) else {
            continue;
        };
        let Some(unit) = cgroup
            .split('/')
            .find(|seg| seg.trim().ends_with(".service"))
            .map(|s| s.trim().to_string())
        else {
            continue;
        };

        if let Ok(fds) = fs::read_dir(pid_entry.path().join("fd")) {
            for fd in fds.flatten() {
                if let Ok(target) = fs::read_link(fd.path())
                    && target.is_absolute()
                    && !target.starts_with("/dev")
                    && !target.starts_with("/proc")
                {
                    map.insert(target, unit.clone());
                }
            }
        }
    }
    map
}

fn detect_git_status(dir: &Path) -> Option<GitDirStatus> {
    let out = Command::new("git")
        .args(["status", "--porcelain=v1", "-b"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let branch_line = lines.next()?;
    let branch = branch_line
        .strip_prefix("## ")
        .unwrap_or("HEAD")
        .split("...")
        .next()
        .unwrap_or("HEAD")
        .to_string();
    let dirty_count = lines.filter(|l| !l.trim().is_empty()).count();
    Some(GitDirStatus {
        branch,
        dirty_count,
    })
}

fn build_file_entry(entry_path: &Path, open_units: &HashMap<PathBuf, String>) -> FileEntry {
    let name = entry_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| entry_path.display().to_string());
    let sym_meta = fs::symlink_metadata(entry_path);
    let is_symlink = sym_meta
        .as_ref()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    let symlink_target = if is_symlink {
        fs::read_link(entry_path)
            .ok()
            .map(|p| p.display().to_string())
    } else {
        None
    };

    let meta_res = fs::metadata(entry_path);
    let symlink_broken = is_symlink && meta_res.is_err();
    let (is_dir, size, modified_ts, mode) = match meta_res.or(sym_meta) {
        Ok(m) => {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            (m.is_dir(), m.size(), mtime, m.permissions().mode())
        }
        Err(_) => (false, 0, 0, 0),
    };

    let held_by_unit = open_units.get(entry_path).cloned();

    FileEntry {
        name,
        path: entry_path.display().to_string(),
        kind: classify_kind(entry_path, is_dir),
        size,
        modified_ts,
        mode,
        is_symlink,
        symlink_target,
        symlink_broken,
        held_by_unit,
    }
}

pub fn read_dir_listing(pane_id: u64, raw_path: &str) -> Result<DirListing, String> {
    let path = expand_path(raw_path);
    let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let read_dir =
        fs::read_dir(&canonical).map_err(|e| format!("{}: {e}", canonical.display()))?;

    let open_units = scan_systemd_open_files();
    let mut entries = Vec::new();

    for item in read_dir.flatten() {
        entries.push(build_file_entry(&item.path(), &open_units));
    }

    entries.sort_by(|a, b| {
        let a_dir = a.kind == FileKind::Directory;
        let b_dir = b.kind == FileKind::Directory;
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut disk_free_bytes = 0;
    let mut disk_total_bytes = 0;
    let mut best_mount_len = 0;
    for d in &disks {
        let mp = d.mount_point();
        if canonical.starts_with(mp) && mp.as_os_str().len() >= best_mount_len {
            best_mount_len = mp.as_os_str().len();
            disk_free_bytes = d.available_space();
            disk_total_bytes = d.total_space();
        }
    }

    Ok(DirListing {
        pane_id,
        path: canonical.display().to_string(),
        git: detect_git_status(&canonical),
        disk_free_bytes,
        disk_total_bytes,
        entries,
    })
}

pub fn execute_file_op(op: FileOp) -> Result<(String, Option<PathBuf>, UndoRecord), String> {
    match op {
        FileOp::Mkdir { parent, name } => {
            let parent_path = expand_path(&parent);
            let target = parent_path.join(&name);
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            Ok((
                format!("Created {}", target.display()),
                Some(parent_path.clone()),
                UndoRecord {
                    description: format!("Undo mkdir {name}"),
                    refresh_dir: Some(parent_path),
                    moves: vec![],
                    created: vec![target],
                },
            ))
        }
        FileOp::Rename { path, new_name } => {
            let src = expand_path(&path);
            let parent = src.parent().ok_or("No parent dir")?.to_path_buf();
            let dst = parent.join(&new_name);
            fs::rename(&src, &dst).map_err(|e| e.to_string())?;
            Ok((
                format!("Renamed to {new_name}"),
                Some(parent.clone()),
                UndoRecord {
                    description: format!("Undo rename {new_name}"),
                    refresh_dir: Some(parent),
                    moves: vec![(dst, src)],
                    created: vec![],
                },
            ))
        }
        FileOp::Move { sources, dest_dir } => {
            let dst_base = expand_path(&dest_dir);
            let mut moves = Vec::new();
            for s in &sources {
                let src = expand_path(s);
                let fname = src.file_name().ok_or("Invalid source path")?;
                let dst = dst_base.join(fname);
                fs::rename(&src, &dst).map_err(|e| e.to_string())?;
                moves.push((dst, src));
            }
            Ok((
                format!("Moved {} item(s)", moves.len()),
                Some(dst_base.clone()),
                UndoRecord {
                    description: "Undo move".into(),
                    refresh_dir: Some(dst_base),
                    moves,
                    created: vec![],
                },
            ))
        }
        FileOp::Copy { sources, dest_dir } => {
            let dst_base = expand_path(&dest_dir);
            let mut created = Vec::new();
            for s in &sources {
                let src = expand_path(s);
                let fname = src.file_name().ok_or("Invalid source path")?;
                let dst = dst_base.join(fname);
                if src.is_dir() {
                    copy_dir_recursive(&src, &dst).map_err(|e| e.to_string())?;
                } else {
                    fs::copy(&src, &dst).map_err(|e| e.to_string())?;
                }
                created.push(dst);
            }
            Ok((
                format!("Copied {} item(s)", created.len()),
                Some(dst_base.clone()),
                UndoRecord {
                    description: "Undo copy".into(),
                    refresh_dir: Some(dst_base),
                    moves: vec![],
                    created,
                },
            ))
        }
        FileOp::Trash { paths } => {
            let trash_dir = expand_path("~/.local/share/kaage/trash");
            fs::create_dir_all(&trash_dir).map_err(|e| e.to_string())?;
            let ts = std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let mut moves = Vec::new();
            let mut refresh_dir = None;
            for p in &paths {
                let src = expand_path(p);
                if refresh_dir.is_none() {
                    refresh_dir = src.parent().map(Path::to_path_buf);
                }
                let fname = src.file_name().ok_or("Invalid path")?.to_string_lossy();
                let trashed = trash_dir.join(format!("{ts}_{fname}"));
                fs::rename(&src, &trashed).map_err(|e| e.to_string())?;
                moves.push((trashed, src));
            }
            Ok((
                format!("Moved {} item(s) to Kaage Trash", moves.len()),
                refresh_dir.clone(),
                UndoRecord {
                    description: "Undo trash".into(),
                    refresh_dir,
                    moves,
                    created: vec![],
                },
            ))
        }
        FileOp::WriteText { path, content } => {
            let target = expand_path(&path);
            let parent = target.parent().map(Path::to_path_buf);
            fs::write(&target, content).map_err(|e| e.to_string())?;
            Ok((
                format!("Saved {}", target.display()),
                parent.clone(),
                UndoRecord {
                    description: "Edit file".into(),
                    refresh_dir: parent,
                    moves: vec![],
                    created: vec![],
                },
            ))
        }
    }
}

pub fn undo_file_op(record: UndoRecord) -> Result<(String, Option<PathBuf>), String> {
    for (current, original) in record.moves.into_iter().rev() {
        if let Some(parent) = original.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::rename(&current, &original)
            .map_err(|e| format!("Failed to restore {}: {e}", original.display()))?;
    }
    for created in record.created.into_iter().rev() {
        if created.is_dir() {
            fs::remove_dir_all(&created)
                .map_err(|e| format!("Failed to remove {}: {e}", created.display()))?;
        } else if created.exists() {
            fs::remove_file(&created)
                .map_err(|e| format!("Failed to remove {}: {e}", created.display()))?;
        }
    }
    Ok((record.description, record.refresh_dir))
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

pub fn build_preview(raw_path: &str) -> PreviewContent {
    let path = expand_path(raw_path);
    let kind = classify_kind(&path, path.is_dir());

    match kind {
        FileKind::Markdown | FileKind::Code | FileKind::Document | FileKind::Other => {
            let Ok(bytes) = fs::read(&path) else {
                return PreviewContent::Unsupported {
                    reason: "Cannot read file".into(),
                };
            };
            let truncated = bytes.len() > 256 * 1024;
            let slice = if truncated {
                &bytes[..256 * 1024]
            } else {
                &bytes
            };
            match String::from_utf8(slice.to_vec()) {
                Ok(content) => PreviewContent::Text {
                    content,
                    is_markdown: kind == FileKind::Markdown,
                    truncated,
                },
                Err(_) => PreviewContent::Unsupported {
                    reason: "Binary file preview not available".into(),
                },
            }
        }
        FileKind::Image => {
            let Ok(bytes) = fs::read(&path) else {
                return PreviewContent::Unsupported {
                    reason: "Cannot read image".into(),
                };
            };
            if bytes.len() > 8 * 1024 * 1024 {
                return PreviewContent::Unsupported {
                    reason: "Image > 8 MB (download to view)".into(),
                };
            }
            let mime = match path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "jpg" | "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                "gif" => "image/gif",
                "svg" => "image/svg+xml",
                _ => "image/png",
            };
            PreviewContent::ImageBytes {
                mime: mime.into(),
                bytes,
            }
        }
        FileKind::ModelWeights => {
            if path.extension().and_then(|e| e.to_str()) == Some("safetensors")
                && let Ok(mut f) = fs::File::open(&path)
            {
                use std::io::Read;
                let mut len_buf = [0u8; 8];
                if f.read_exact(&mut len_buf).is_ok() {
                    let header_len = u64::from_le_bytes(len_buf).min(65536) as usize;
                    let mut hdr = vec![0u8; header_len];
                    if f.read_exact(&mut hdr).is_ok()
                        && let Ok(val) = serde_json::from_slice::<serde_json::Value>(&hdr)
                    {
                        let mut meta = Vec::new();
                        if let Some(obj) = val.as_object() {
                            meta.push(("Tensors".into(), obj.len().to_string()));
                            if let Some(m) = obj.get("__metadata__").and_then(|v| v.as_object()) {
                                for (k, v) in m.iter().take(12) {
                                    meta.push((k.clone(), v.to_string()));
                                }
                            }
                        }
                        return PreviewContent::ModelHeader {
                            format: "SafeTensors".into(),
                            metadata: meta,
                        };
                    }
                }
            }
            PreviewContent::Unsupported {
                reason: "Model weight binary".into(),
            }
        }
        _ => PreviewContent::Unsupported {
            reason: "Select a text, Markdown, image, or SafeTensors file to preview".into(),
        },
    }
}

/// Bounded recursive file search (filename and optional UTF-8 content match).
pub fn search_files(root: &str, pattern: &str, search_contents: bool) -> Vec<FileEntry> {
    let needle = pattern.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let root_path = expand_path(root);
    let open_units = HashMap::new();
    let mut matches = Vec::new();
    let mut stack = vec![(root_path, 0usize)];

    while let Some((dir, depth)) = stack.pop() {
        if depth > 6 || matches.len() >= 100 {
            continue;
        }
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for item in rd.flatten() {
            let p = item.path();
            let fname = item.file_name().to_string_lossy().to_string();
            if fname.starts_with('.') || fname == "target" || fname == "node_modules" {
                continue;
            }
            let is_dir = item.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let mut matched = fname.to_ascii_lowercase().contains(&needle);
            if !matched
                && search_contents
                && !is_dir
                && item.metadata().map(|m| m.len() <= 256 * 1024).unwrap_or(false)
                && let Ok(text) = fs::read_to_string(&p)
                && text.to_ascii_lowercase().contains(&needle)
            {
                matched = true;
            }
            if matched {
                matches.push(build_file_entry(&p, &open_units));
                if matches.len() >= 100 {
                    break;
                }
            }
            if is_dir {
                stack.push((p, depth + 1));
            }
        }
    }
    matches
}

pub fn load_workspaces() -> Vec<WorkspaceDef> {
    let dir = expand_path("~/.config/kaage/workspaces");
    let _ = fs::create_dir_all(&dir);
    let mut list = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.path().extension().and_then(|s| s.to_str()) == Some("json")
                && let Ok(raw) = fs::read_to_string(e.path())
                && let Ok(ws) = serde_json::from_str::<WorkspaceDef>(&raw)
            {
                list.push(ws);
            }
        }
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

pub fn save_workspace(ws: &WorkspaceDef) -> Result<(), String> {
    let dir = expand_path("~/.config/kaage/workspaces");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe_name: String = ws
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let json = serde_json::to_string_pretty(ws).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{safe_name}.json")), json).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
pub struct DownloadQuery {
    pub path: String,
}

pub async fn http_download_file(
    headers: HeaderMap,
    Query(q): Query<DownloadQuery>,
    State(state): State<Arc<crate::AppState>>,
) -> impl IntoResponse {
    if !crate::check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    let path = expand_path(&q.path);
    let Ok(file) = tokio::fs::File::open(&path).await else {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    };
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download.bin");
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_file_kinds() {
        assert_eq!(classify_kind(Path::new("/tmp/foo"), true), FileKind::Directory);
        assert_eq!(classify_kind(Path::new("README.md"), false), FileKind::Markdown);
        assert_eq!(
            classify_kind(Path::new("model.safetensors"), false),
            FileKind::ModelWeights
        );
        assert_eq!(classify_kind(Path::new("main.rs"), false), FileKind::Code);
        assert_eq!(classify_kind(Path::new("photo.jpg"), false), FileKind::Image);
        assert_eq!(classify_kind(Path::new("pkg.tar.zst"), false), FileKind::Archive);
    }

    #[test]
    fn mkdir_and_undo_roundtrip() {
        let base = std::env::temp_dir().join(format!("kaage_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let (msg, _, undo) = execute_file_op(FileOp::Mkdir {
            parent: base.display().to_string(),
            name: "sub".into(),
        })
        .unwrap();
        assert!(msg.contains("Created"));
        assert!(base.join("sub").is_dir());

        let (undo_msg, _) = undo_file_op(undo).unwrap();
        assert!(undo_msg.contains("Undo"));
        assert!(!base.join("sub").exists());

        let _ = fs::remove_dir_all(&base);
    }
}

use std::path::{Path, PathBuf};

use fastrace::trace;
use leta_types::{CreateFileParams, CreateFileResult};

use super::{relative_path, HandlerContext};

/// Create `path` with `content`, creating missing parent directories.
///
/// Returns `(created, overwritten, line_count)`. When `dry_run` is true no
/// filesystem changes are made.
pub(crate) fn create_file_impl(
    path: &Path,
    content: &str,
    overwrite: bool,
    dry_run: bool,
) -> Result<(bool, bool, u32), String> {
    let exists = path.exists();
    let overwritten = if exists {
        if path.is_dir() {
            return Err(format!("Path is a directory: {}", path.display()));
        }
        if !overwrite {
            return Err(format!(
                "File already exists: {} (use --force to overwrite)",
                path.display()
            ));
        }
        true
    } else {
        false
    };

    let line_count = content.lines().count() as u32;

    if dry_run {
        return Ok((!exists, overwritten, line_count));
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
        }
    }

    std::fs::write(path, content)
        .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;

    Ok((!exists, overwritten, line_count))
}

/// True when `path` is lexically or physically located inside `workspace_root`.
///
/// The target may not exist yet, so the closest existing ancestor is
/// canonicalized to resolve `..` segments and symlinks before comparing.
fn is_within_workspace(path: &Path, workspace_root: &Path) -> bool {
    let root = match workspace_root.canonicalize() {
        Ok(root) => root,
        Err(_) => return false,
    };

    let mut ancestor = path.parent();
    while let Some(current) = ancestor {
        if current.exists() {
            return match current.canonicalize() {
                Ok(current) => current.starts_with(&root),
                Err(_) => false,
            };
        }
        ancestor = current.parent();
    }

    false
}

#[trace]
pub async fn handle_create_file(
    ctx: &HandlerContext,
    params: CreateFileParams,
) -> Result<CreateFileResult, String> {
    let workspace_root = PathBuf::from(&params.workspace_root);
    let file_path = PathBuf::from(&params.path);

    if !file_path.is_absolute() {
        return Err(format!("Path must be absolute: {}", params.path));
    }

    if !is_within_workspace(&file_path, &workspace_root) {
        return Err(format!(
            "Path is outside workspace: {} (workspace: {})",
            file_path.display(),
            workspace_root.display()
        ));
    }

    let (created, overwritten, line_count) = create_file_impl(
        &file_path,
        &params.content,
        params.overwrite,
        params.dry_run,
    )?;

    let result_path = relative_path(&file_path, &workspace_root);

    if params.dry_run {
        return Ok(CreateFileResult {
            created,
            overwritten,
            path: result_path,
            line_count,
            dry_run: true,
        });
    }

    // Best-effort: notify the language server if a workspace is already
    // running for this file. Unsupported file types are created without
    // starting a language server.
    if let Some(workspace) = ctx.session.get_workspace_for_file(&file_path).await {
        if overwritten {
            let _ = workspace.ensure_document_open(&file_path).await;
            workspace
                .notify_files_changed(&[(
                    file_path.clone(),
                    leta_lsp::lsp_types::FileChangeType::CHANGED,
                )])
                .await;
        } else {
            workspace
                .notify_files_changed(&[(
                    file_path.clone(),
                    leta_lsp::lsp_types::FileChangeType::CREATED,
                )])
                .await;
        }
    }

    Ok(CreateFileResult {
        created,
        overwritten,
        path: result_path,
        line_count,
        dry_run: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn creates_missing_parents() {
        let dir = workspace();
        let path = dir.path().join("a/b/c.txt");

        let (created, overwritten, line_count) =
            create_file_impl(&path, "alpha\nbeta\n", false, false).unwrap();

        assert!(created);
        assert!(!overwritten);
        assert_eq!(line_count, 2);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\nbeta\n");
    }

    #[test]
    fn refuses_existing_file_without_overwrite() {
        let dir = workspace();
        let path = dir.path().join("exists.txt");
        std::fs::write(&path, "old").unwrap();

        let err = create_file_impl(&path, "new", false, false).unwrap_err();
        assert!(err.contains("File already exists"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    }

    #[test]
    fn overwrites_existing_file_with_force() {
        let dir = workspace();
        let path = dir.path().join("exists.txt");
        std::fs::write(&path, "old").unwrap();

        let (created, overwritten, _) = create_file_impl(&path, "new", true, false).unwrap();

        assert!(!created);
        assert!(overwritten);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = workspace();
        let path = dir.path().join("nested/file.txt");

        let (created, overwritten, line_count) =
            create_file_impl(&path, "hello\nworld\n", false, true).unwrap();

        assert!(created);
        assert!(!overwritten);
        assert_eq!(line_count, 2);
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn rejects_existing_directory() {
        let dir = workspace();
        let path = dir.path().join("folder");
        std::fs::create_dir(&path).unwrap();

        let err = create_file_impl(&path, "", false, false).unwrap_err();
        assert!(err.contains("Path is a directory"));
    }

    #[test]
    fn detects_paths_inside_workspace() {
        let dir = workspace();
        let path = dir.path().join("nested/file.txt");

        assert!(is_within_workspace(&path, dir.path()));
    }

    #[test]
    fn rejects_paths_outside_workspace() {
        let dir = workspace();
        let outside = dir.path().join("../outside.txt");

        assert!(!is_within_workspace(&outside, dir.path()));
    }
}

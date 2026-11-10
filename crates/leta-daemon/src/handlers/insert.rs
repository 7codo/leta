use std::path::PathBuf;

use fastrace::trace;
use leta_fs::read_file_content;
use leta_types::{InsertParams, InsertPosition, InsertResult};

use super::text_edit::{is_decorator_line, leading_whitespace, reindent};
use super::{relative_path, HandlerContext};

#[trace]
pub async fn handle_insert(
    ctx: &HandlerContext,
    params: InsertParams,
) -> Result<InsertResult, String> {
    let workspace_root = PathBuf::from(&params.workspace_root);
    let file_path = PathBuf::from(&params.path);

    let start_line = params
        .range_start_line
        .ok_or_else(|| "Language server does not provide symbol ranges".to_string())?;
    let end_line = params
        .range_end_line
        .ok_or_else(|| "Language server does not provide symbol ranges".to_string())?;

    if start_line == 0 || end_line < start_line {
        return Err(format!("Invalid symbol range: {}-{}", start_line, end_line));
    }

    if params.new_text.trim().is_empty() {
        return Err("Inserted text is empty".to_string());
    }

    let original = read_file_content(&file_path)
        .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;

    let (new_content, insert_line, new_line_count) = insert_relative_to_symbol(
        &original,
        start_line,
        end_line,
        &params.new_text,
        params.position,
    )?;

    let result_path = relative_path(&file_path, &workspace_root);

    if params.dry_run {
        return Ok(InsertResult {
            inserted: false,
            path: result_path,
            position: params.position,
            insert_line,
            new_line_count,
            dry_run: true,
        });
    }

    // Acquire the workspace before touching the file so a missing language
    // server surfaces as an error without leaving a half-applied change.
    let workspace = ctx
        .session
        .get_or_create_workspace(&file_path, &workspace_root)
        .await
        .map_err(|e| e.to_string())?;

    std::fs::write(&file_path, &new_content)
        .map_err(|e| format!("Failed to write {}: {}", file_path.display(), e))?;

    workspace.ensure_document_open(&file_path).await?;
    workspace
        .notify_files_changed(&[(
            file_path.clone(),
            leta_lsp::lsp_types::FileChangeType::CHANGED,
        )])
        .await;

    Ok(InsertResult {
        inserted: true,
        path: result_path,
        position: params.position,
        insert_line,
        new_line_count,
        dry_run: false,
    })
}

/// Insert `new_text` before or after the 1-based inclusive symbol range
/// `start_line..=end_line`, re-indenting it to match the symbol indentation.
fn insert_relative_to_symbol(
    content: &str,
    start_line: u32,
    end_line: u32,
    new_text: &str,
    position: InsertPosition,
) -> Result<(String, u32, u32), String> {
    let had_trailing_newline = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(String::from).collect();

    let start_idx = (start_line as usize)
        .checked_sub(1)
        .ok_or_else(|| "Symbol range starts at line 0".to_string())?;
    if start_idx >= lines.len() {
        return Err(format!(
            "Symbol range starts at line {} but file has {} line(s)",
            start_line,
            lines.len()
        ));
    }
    let end_idx = ((end_line as usize).saturating_sub(1)).min(lines.len() - 1);

    let base_indent = leading_whitespace(&lines[start_idx]).to_string();
    let reindented = reindent(new_text, &base_indent);
    let new_lines: Vec<String> = reindented.split('\n').map(String::from).collect();
    let new_line_count = new_lines.len() as u32;

    let insert_idx = match position {
        InsertPosition::Before => {
            let mut idx = start_idx;
            // Some language servers exclude decorators/attributes from the
            // symbol range. Move above them so the new source is not inserted
            // between an attribute and the item it applies to.
            while idx > 0 && is_decorator_line(&lines[idx - 1]) {
                idx -= 1;
            }
            idx
        }
        InsertPosition::After => end_idx + 1,
    };

    lines.splice(insert_idx..insert_idx, new_lines);

    let mut result = lines.join("\n");
    if had_trailing_newline {
        result.push('\n');
    }

    Ok((result, insert_idx as u32 + 1, new_line_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_before_line() {
        let original = "def a():\n    return 1\n\ndef b():\n    return 2\n";
        let (updated, line, count) = insert_relative_to_symbol(
            original,
            4,
            5,
            "def c():\n    return 3\n",
            InsertPosition::Before,
        )
        .unwrap();
        assert_eq!(line, 4);
        assert_eq!(count, 2);
        assert_eq!(
            updated,
            "def a():\n    return 1\n\ndef c():\n    return 3\ndef b():\n    return 2\n"
        );
    }

    #[test]
    fn inserts_after_line() {
        let original = "def a():\n    return 1\n\ndef b():\n    return 2\n";
        let (updated, line, count) = insert_relative_to_symbol(
            original,
            1,
            2,
            "def c():\n    return 3\n",
            InsertPosition::After,
        )
        .unwrap();
        assert_eq!(line, 3);
        assert_eq!(count, 2);
        assert_eq!(
            updated,
            "def a():\n    return 1\ndef c():\n    return 3\n\ndef b():\n    return 2\n"
        );
    }

    #[test]
    fn reindents_after_indented_method() {
        let original = "class A:\n    def save(self):\n        pass\n";
        let (updated, line, _) = insert_relative_to_symbol(
            original,
            2,
            3,
            "def load(self):\n    return None\n",
            InsertPosition::After,
        )
        .unwrap();
        assert_eq!(line, 4);
        assert_eq!(
            updated,
            "class A:\n    def save(self):\n        pass\n    def load(self):\n        return None\n"
        );
    }

    #[test]
    fn inserts_before_decorator_above_reported_range() {
        let original = "@decorator\ndef foo():\n    return 1\n";
        // Range reported as the `def` line only.
        let (updated, line, _) = insert_relative_to_symbol(
            original,
            2,
            3,
            "def bar():\n    return 2\n",
            InsertPosition::Before,
        )
        .unwrap();
        assert_eq!(line, 1);
        assert_eq!(
            updated,
            "def bar():\n    return 2\n@decorator\ndef foo():\n    return 1\n"
        );
    }

    #[test]
    fn preserves_missing_trailing_newline() {
        let original = "def a():\n    return 1";
        let (updated, _, _) =
            insert_relative_to_symbol(original, 1, 2, "def c(): pass", InsertPosition::Before)
                .unwrap();
        assert_eq!(updated, "def c(): pass\ndef a():\n    return 1");
    }
}

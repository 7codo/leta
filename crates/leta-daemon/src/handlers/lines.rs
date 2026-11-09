use std::path::{Path, PathBuf};

use fastrace::trace;
use leta_fs::read_file_content;
use leta_lsp::lsp_types::FileChangeType;
use leta_types::{
    DeleteLinesParams, DeleteLinesResult, InsertAtLineParams, InsertAtLineResult,
    ReplaceLinesParams, ReplaceLinesResult,
};

use super::{relative_path, HandlerContext};

#[trace]
pub async fn handle_delete_lines(
    ctx: &HandlerContext,
    params: DeleteLinesParams,
) -> Result<DeleteLinesResult, String> {
    let workspace_root = PathBuf::from(&params.workspace_root);
    let file_path = PathBuf::from(&params.path);
    let result_path = relative_path(&file_path, &workspace_root);

    let content = read_target_file(&file_path)?;
    let (new_content, deleted_line_count) =
        delete_lines_in_content(&content, params.start_line, params.end_line, &result_path)?;

    if !params.dry_run {
        write_and_sync(ctx, &file_path, &new_content).await?;
    }

    Ok(DeleteLinesResult {
        deleted: !params.dry_run,
        path: result_path,
        start_line: params.start_line,
        end_line: params.end_line,
        deleted_line_count,
        dry_run: params.dry_run,
    })
}

#[trace]
pub async fn handle_insert_at_line(
    ctx: &HandlerContext,
    params: InsertAtLineParams,
) -> Result<InsertAtLineResult, String> {
    if params.new_text.trim().is_empty() {
        return Err("Inserted text is empty".to_string());
    }

    let workspace_root = PathBuf::from(&params.workspace_root);
    let file_path = PathBuf::from(&params.path);
    let result_path = relative_path(&file_path, &workspace_root);

    let content = read_target_file(&file_path)?;
    let (new_content, new_line_count) =
        insert_at_line_in_content(&content, params.line, &params.new_text, &result_path)?;

    if !params.dry_run {
        write_and_sync(ctx, &file_path, &new_content).await?;
    }

    Ok(InsertAtLineResult {
        inserted: !params.dry_run,
        path: result_path,
        insert_line: params.line,
        new_line_count,
        dry_run: params.dry_run,
    })
}

#[trace]
pub async fn handle_replace_lines(
    ctx: &HandlerContext,
    params: ReplaceLinesParams,
) -> Result<ReplaceLinesResult, String> {
    if params.new_text.trim().is_empty() {
        return Err("Replacement text is empty".to_string());
    }

    let workspace_root = PathBuf::from(&params.workspace_root);
    let file_path = PathBuf::from(&params.path);
    let result_path = relative_path(&file_path, &workspace_root);

    let content = read_target_file(&file_path)?;
    let (new_content, new_line_count) = replace_lines_in_content(
        &content,
        params.start_line,
        params.end_line,
        &params.new_text,
        &result_path,
    )?;

    if !params.dry_run {
        write_and_sync(ctx, &file_path, &new_content).await?;
    }

    let new_end_line = params.start_line + new_line_count.saturating_sub(1);

    Ok(ReplaceLinesResult {
        replaced: !params.dry_run,
        path: result_path,
        start_line: params.start_line,
        end_line: params.end_line,
        new_start_line: params.start_line,
        new_end_line,
        new_line_count,
        dry_run: params.dry_run,
    })
}

fn read_target_file(file_path: &Path) -> Result<String, String> {
    if !file_path.is_file() {
        return Err(format!("Not a file: {}", file_path.display()));
    }

    read_file_content(file_path)
        .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))
}

/// Write the file and, if a language server is already running for it, reopen
/// the document so the server sees the new content. Never starts a server just
/// to apply a line edit.
async fn write_and_sync(
    ctx: &HandlerContext,
    file_path: &Path,
    new_content: &str,
) -> Result<(), String> {
    std::fs::write(file_path, new_content)
        .map_err(|e| format!("Failed to write {}: {}", file_path.display(), e))?;

    if let Some(workspace) = ctx.session.get_workspace_for_file(file_path).await {
        let _ = workspace.ensure_document_open(file_path).await;
        workspace
            .notify_files_changed(&[(file_path.to_path_buf(), FileChangeType::Changed)])
            .await;
    }

    Ok(())
}

struct SplitContent<'a> {
    lines: Vec<&'a str>,
    trailing_newline: bool,
    eol: &'static str,
}

fn split_content(content: &str) -> SplitContent<'_> {
    SplitContent {
        lines: content.lines().collect(),
        trailing_newline: content.ends_with('\n'),
        eol: if content.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        },
    }
}

fn join_lines(lines: &[&str], trailing_newline: bool, eol: &str) -> String {
    if lines.is_empty() {
        return String::new();
    }

    let mut result = lines.join(eol);
    if trailing_newline {
        result.push_str(eol);
    }
    result
}

fn line_word(count: u32) -> &'static str {
    if count == 1 {
        "line"
    } else {
        "lines"
    }
}

fn validate_range(
    line_count: u32,
    start_line: u32,
    end_line: u32,
    path: &str,
) -> Result<(), String> {
    if start_line == 0 || end_line < start_line || end_line > line_count {
        return Err(format!(
            "Invalid line range {}-{} for {} ({} {})",
            start_line,
            end_line,
            path,
            line_count,
            line_word(line_count)
        ));
    }

    Ok(())
}

fn validate_insert_line(line_count: u32, line: u32, path: &str) -> Result<(), String> {
    let max_line = line_count.saturating_add(1);
    if line == 0 || line > max_line {
        return Err(format!(
            "Invalid insert line {} for {} ({} {}); valid range is 1..={}",
            line,
            path,
            line_count,
            line_word(line_count),
            max_line
        ));
    }

    Ok(())
}

fn delete_lines_in_content(
    content: &str,
    start_line: u32,
    end_line: u32,
    path: &str,
) -> Result<(String, u32), String> {
    let split = split_content(content);
    let line_count = split.lines.len() as u32;
    validate_range(line_count, start_line, end_line, path)?;

    let start_idx = start_line as usize - 1;
    let end_idx = end_line as usize;

    let mut lines: Vec<&str> = split.lines;
    lines.drain(start_idx..end_idx);

    Ok((
        join_lines(&lines, split.trailing_newline, split.eol),
        (end_idx - start_idx) as u32,
    ))
}

fn insert_at_line_in_content(
    content: &str,
    line: u32,
    new_text: &str,
    path: &str,
) -> Result<(String, u32), String> {
    let split = split_content(content);
    let line_count = split.lines.len() as u32;
    validate_insert_line(line_count, line, path)?;

    let new_lines: Vec<&str> = new_text.lines().collect();
    if new_lines.is_empty() {
        return Err("Inserted text is empty".to_string());
    }
    let new_line_count = new_lines.len() as u32;

    let insert_idx = line as usize - 1;
    let mut lines: Vec<&str> = split.lines;
    lines.splice(insert_idx..insert_idx, new_lines);

    Ok((
        join_lines(&lines, split.trailing_newline, split.eol),
        new_line_count,
    ))
}

fn replace_lines_in_content(
    content: &str,
    start_line: u32,
    end_line: u32,
    new_text: &str,
    path: &str,
) -> Result<(String, u32), String> {
    let split = split_content(content);
    let line_count = split.lines.len() as u32;
    validate_range(line_count, start_line, end_line, path)?;

    let new_lines: Vec<&str> = new_text.lines().collect();
    if new_lines.is_empty() {
        return Err("Replacement text is empty".to_string());
    }
    let new_line_count = new_lines.len() as u32;

    let start_idx = start_line as usize - 1;
    let end_idx = end_line as usize;
    let mut lines: Vec<&str> = split.lines;
    lines.splice(start_idx..end_idx, new_lines);

    Ok((
        join_lines(&lines, split.trailing_newline, split.eol),
        new_line_count,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletes_single_line() {
        let content = "alpha\nbeta\ngamma\n";
        let (updated, count) = delete_lines_in_content(content, 2, 2, "notes.txt").unwrap();
        assert_eq!(count, 1);
        assert_eq!(updated, "alpha\ngamma\n");
    }

    #[test]
    fn deletes_range() {
        let content = "alpha\nbeta\ngamma\ndelta\n";
        let (updated, count) = delete_lines_in_content(content, 2, 3, "notes.txt").unwrap();
        assert_eq!(count, 2);
        assert_eq!(updated, "alpha\ndelta\n");
    }

    #[test]
    fn deleting_everything_empties_file() {
        let content = "alpha\nbeta\n";
        let (updated, count) = delete_lines_in_content(content, 1, 2, "notes.txt").unwrap();
        assert_eq!(count, 2);
        assert_eq!(updated, "");
    }

    #[test]
    fn delete_preserves_missing_trailing_newline() {
        let content = "alpha\nbeta";
        let (updated, _) = delete_lines_in_content(content, 1, 1, "notes.txt").unwrap();
        assert_eq!(updated, "beta");
    }

    #[test]
    fn delete_rejects_out_of_range() {
        let err = delete_lines_in_content("alpha\nbeta\n", 2, 3, "notes.txt").unwrap_err();
        assert_eq!(err, "Invalid line range 2-3 for notes.txt (2 lines)");
    }

    #[test]
    fn inserts_at_start_middle_and_end() {
        let content = "alpha\nbeta\n";

        let (updated, count) =
            insert_at_line_in_content(content, 1, "first\n", "notes.txt").unwrap();
        assert_eq!(count, 1);
        assert_eq!(updated, "first\nalpha\nbeta\n");

        let (updated, _) = insert_at_line_in_content(content, 2, "between", "notes.txt").unwrap();
        assert_eq!(updated, "alpha\nbetween\nbeta\n");

        let (updated, _) = insert_at_line_in_content(content, 3, "last", "notes.txt").unwrap();
        assert_eq!(updated, "alpha\nbeta\nlast\n");
    }

    #[test]
    fn inserts_into_empty_file() {
        let (updated, count) =
            insert_at_line_in_content("", 1, "alpha\nbeta\n", "notes.txt").unwrap();
        assert_eq!(count, 2);
        assert_eq!(updated, "alpha\nbeta");
    }

    #[test]
    fn insert_rejects_out_of_range() {
        let err = insert_at_line_in_content("alpha\n", 3, "x", "notes.txt").unwrap_err();
        assert_eq!(
            err,
            "Invalid insert line 3 for notes.txt (1 line); valid range is 1..=2"
        );
    }

    #[test]
    fn replaces_range_with_more_lines() {
        let content = "alpha\nbeta\ngamma\n";
        let (updated, count) =
            replace_lines_in_content(content, 2, 2, "beta-1\nbeta-2\n", "notes.txt").unwrap();
        assert_eq!(count, 2);
        assert_eq!(updated, "alpha\nbeta-1\nbeta-2\ngamma\n");
    }

    #[test]
    fn replaces_range_with_fewer_lines() {
        let content = "alpha\nbeta\ngamma\ndelta\n";
        let (updated, count) =
            replace_lines_in_content(content, 2, 4, "middle\n", "notes.txt").unwrap();
        assert_eq!(count, 1);
        assert_eq!(updated, "alpha\nmiddle\n");
    }

    #[test]
    fn replace_rejects_out_of_range() {
        let err = replace_lines_in_content("alpha\n", 1, 2, "x", "notes.txt").unwrap_err();
        assert_eq!(err, "Invalid line range 1-2 for notes.txt (1 line)");
    }

    #[test]
    fn preserves_crlf_line_endings() {
        let content = "alpha\r\nbeta\r\ngamma\r\n";
        let (updated, _) = replace_lines_in_content(content, 2, 2, "delta\n", "notes.txt").unwrap();
        assert_eq!(updated, "alpha\r\ndelta\r\ngamma\r\n");
    }
}

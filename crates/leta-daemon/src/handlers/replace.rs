use std::path::{Path, PathBuf};

use fastrace::trace;
use leta_fs::read_file_content;
use leta_types::{ReplaceParams, ReplaceResult};

use super::text_edit::{leading_whitespace, reindent};
use super::{relative_path, HandlerContext};

#[trace]
pub async fn handle_replace(
    ctx: &HandlerContext,
    params: ReplaceParams,
) -> Result<ReplaceResult, String> {
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
        return Err("Replacement text is empty".to_string());
    }

    let original = read_file_content(&file_path)
        .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;

    let (new_content, new_line_count) =
        replace_line_range(&original, start_line, end_line, &params.new_text)?;

    let symbol_label = params
        .symbol_name
        .as_deref()
        .unwrap_or(params.path.as_str());

    let check_signature = params.check_signature && is_function_like(params.symbol_kind.as_deref());
    if check_signature {
        let lines: Vec<&str> = original.lines().collect();
        let style = header_style(&file_path);

        let start_idx = start_line as usize - 1;
        let end_idx = (end_line as usize - 1).min(lines.len().saturating_sub(1));
        let old_header =
            extract_header_from_lines(lines[start_idx..=end_idx].iter().copied(), style)
                .ok_or_else(|| {
                    format!(
                        "Could not extract signature for '{}'; use --no-check-signature \
                 to replace anyway",
                        symbol_label
                    )
                })?;
        let new_header =
            extract_header_from_lines(params.new_text.lines(), style).ok_or_else(|| {
                format!(
                    "Could not extract signature from replacement text; \
                 use --no-check-signature to replace anyway"
                )
            })?;

        let old_normalized = normalize_signature(&old_header);
        let new_normalized = normalize_signature(&new_header);
        if old_normalized != new_normalized {
            return Err(format!(
                "Signature mismatch for '{}':\n  old: {}\n  new: {}\nUse --no-check-signature to replace anyway",
                symbol_label, old_header, new_header
            ));
        }
    }

    let result_path = relative_path(&file_path, &workspace_root);
    let new_end_line = start_line + new_line_count.saturating_sub(1);

    if params.dry_run {
        return Ok(ReplaceResult {
            replaced: false,
            path: result_path,
            old_start_line: start_line,
            old_end_line: end_line,
            new_start_line: start_line,
            new_end_line,
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

    // Refresh the server's copy of the document. `ensure_document_open` closes
    // and reopens when the on-disk content differs from the tracked content.
    workspace.ensure_document_open(&file_path).await?;
    workspace
        .notify_files_changed(&[(
            file_path.clone(),
            leta_lsp::lsp_types::FileChangeType::CHANGED,
        )])
        .await;

    Ok(ReplaceResult {
        replaced: true,
        path: result_path,
        old_start_line: start_line,
        old_end_line: end_line,
        new_start_line: start_line,
        new_end_line,
        new_line_count,
        dry_run: false,
    })
}

fn is_function_like(kind: Option<&str>) -> bool {
    matches!(kind, Some("Function") | Some("Method"))
}

/// Replace the 1-based inclusive line range `start_line..=end_line` with
/// `new_text`, re-indenting it to match the original symbol indentation.
fn replace_line_range(
    content: &str,
    start_line: u32,
    end_line: u32,
    new_text: &str,
) -> Result<(String, u32), String> {
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

    lines.splice(start_idx..=end_idx, new_lines);

    let mut result = lines.join("\n");
    if had_trailing_newline {
        result.push('\n');
    }

    Ok((result, new_line_count))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderStyle {
    /// Header is the first source line (Ruby, Lua, OCaml, Elixir, ...).
    FirstLine,
    /// Header ends at a line ending with `:` (Python).
    ColonTerminated,
    /// Header ends at the first line containing `{` (Rust, Go, TS, ...).
    BraceTerminated,
}

fn header_style(path: &Path) -> HeaderStyle {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "py" | "pyi" => HeaderStyle::ColonTerminated,
        "rs" | "go" | "ts" | "tsx" | "js" | "jsx" | "java" | "c" | "h" | "cc" | "cpp" | "cxx"
        | "hpp" | "cs" | "php" | "zig" | "kt" | "kts" | "swift" | "dart" | "scala" => {
            HeaderStyle::BraceTerminated
        }
        _ => HeaderStyle::FirstLine,
    }
}

fn extract_header_from_lines<'a, I>(lines: I, style: HeaderStyle) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut header = String::new();
    let mut parens: i32 = 0;
    let mut brackets: i32 = 0;
    let mut braces: i32 = 0;
    let mut started = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if started {
                break;
            }
            continue;
        }

        if !started && is_decorator(trimmed) {
            continue;
        }
        started = true;

        let line_start = if header.is_empty() {
            0
        } else {
            header.push(' ');
            header.len()
        };
        header.push_str(trimmed);

        let mut terminator: Option<usize> = None;

        for (offset, ch) in trimmed.char_indices() {
            let pos = line_start + offset;

            match ch {
                '(' => parens += 1,
                ')' => parens -= 1,
                '[' => brackets += 1,
                ']' => brackets -= 1,
                '{' if parens <= 0 && brackets <= 0 => {
                    if style == HeaderStyle::BraceTerminated {
                        if braces == 0 {
                            // The body opening brace is the last top-level `{`
                            // on this line, so keep scanning: `fn f() -> {A: B} {`
                            // has a type-level brace before the body.
                            terminator = Some(pos + 1);
                        }
                        braces += 1;
                    }
                }
                '}' if parens <= 0 && brackets <= 0 && braces > 0 => {
                    braces -= 1;
                }
                ':' if style == HeaderStyle::ColonTerminated && parens <= 0 && brackets <= 0 => {
                    terminator = Some(pos + 1);
                }
                ';' | '=' if style == HeaderStyle::BraceTerminated => {
                    // `=>` and `;` end a signature/declaration when they are
                    // not nested inside parentheses or brackets.
                    if parens <= 0 && brackets <= 0 && braces == 0 {
                        if ch == ';' && terminator.is_none() {
                            terminator = Some(pos + 1);
                        } else if ch == '=' && trimmed[offset..].starts_with("=>") {
                            terminator = Some(pos + 2);
                        }
                    }
                }
                _ => {}
            }

            if style == HeaderStyle::ColonTerminated && terminator.is_some() {
                break;
            }
            if matches!(ch, ';') && terminator.is_some() {
                break;
            }
        }

        if let Some(end) = terminator {
            header.truncate(end);
            return Some(header.trim().to_string());
        }

        if style == HeaderStyle::FirstLine {
            return Some(header.trim().to_string());
        }

        // Guard against pathological input that never terminates.
        if header.len() > 4096 {
            break;
        }
    }

    if header.trim().is_empty() {
        None
    } else {
        Some(header.trim().to_string())
    }
}

fn is_decorator(line: &str) -> bool {
    line.starts_with('@') || line.starts_with("#[") || line.starts_with("#![")
}

/// Formatting-insensitive signature comparison: whitespace, and trailing
/// commas before a closing delimiter, are irrelevant.
fn normalize_signature(header: &str) -> String {
    let compact: String = header.chars().filter(|c| !c.is_whitespace()).collect();
    compact
        .replace(",)", ")")
        .replace(",{", "{")
        .replace(",;", ";")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_line_range_and_preserves_trailing_newline() {
        let original = "def foo():\n    return 1\n\ndef bar():\n    return 2\n";
        let (updated, count) =
            replace_line_range(original, 1, 2, "def foo():\n    return 42\n").unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            updated,
            "def foo():\n    return 42\n\ndef bar():\n    return 2\n"
        );
    }

    #[test]
    fn replaces_last_line_without_trailing_newline() {
        let original = "def foo():\n    return 1";
        let (updated, count) = replace_line_range(original, 1, 2, "def foo(): return 1").unwrap();
        assert_eq!(count, 1);
        assert_eq!(updated, "def foo(): return 1");
    }

    #[test]
    fn extracts_python_signature_with_multiline_params() {
        let lines = [
            "def foo(",
            "    a: int,",
            "    b: str,",
            ") -> bool:",
            "    return True",
        ];
        let header =
            extract_header_from_lines(lines.iter().copied(), HeaderStyle::ColonTerminated).unwrap();
        assert_eq!(header, "def foo( a: int, b: str, ) -> bool:");
        assert_eq!(
            normalize_signature(&header),
            normalize_signature("def foo(a: int, b: str) -> bool:")
        );
    }

    #[test]
    fn truncates_python_one_line_body() {
        let lines = ["def foo() -> int: return 1", "def bar() -> int: return 2"];
        let header =
            extract_header_from_lines(lines.iter().copied(), HeaderStyle::ColonTerminated).unwrap();
        assert_eq!(header, "def foo() -> int:");
    }

    #[test]
    fn extracts_rust_signature_with_where_clause() {
        let lines = [
            "pub fn foo<T>(value: T) -> T",
            "where",
            "    T: Clone,",
            "{",
            "    value",
            "}",
        ];
        let header =
            extract_header_from_lines(lines.iter().copied(), HeaderStyle::BraceTerminated).unwrap();
        assert_eq!(header, "pub fn foo<T>(value: T) -> T where T: Clone, {");
    }

    #[test]
    fn truncates_brace_one_line_body() {
        let lines = ["fn foo() -> i32 { 1 }", "fn bar() -> i32 { 2 }"];
        let header =
            extract_header_from_lines(lines.iter().copied(), HeaderStyle::BraceTerminated).unwrap();
        assert_eq!(header, "fn foo() -> i32 {");
    }

    #[test]
    fn extracts_arrow_signature_without_body() {
        let lines = ["const foo = (value: number) => value + 1;"];
        let header =
            extract_header_from_lines(lines.iter().copied(), HeaderStyle::BraceTerminated).unwrap();
        assert_eq!(header, "const foo = (value: number) =>");
    }

    #[test]
    fn extracts_first_line_for_ruby() {
        let lines = ["def foo", "  puts 1", "end"];
        let header =
            extract_header_from_lines(lines.iter().copied(), HeaderStyle::FirstLine).unwrap();
        assert_eq!(header, "def foo");
    }

    #[test]
    fn detects_signature_mismatch() {
        let old = "def foo(a: int) -> int:";
        let new = "def foo(a: int, b: str) -> int:";
        assert_ne!(normalize_signature(old), normalize_signature(new));
    }

    #[test]
    fn ignores_signature_formatting_differences() {
        let old = "def foo( a: int, b: str, ) -> bool:";
        let new = "def foo(a: int, b: str) -> bool:";
        assert_eq!(normalize_signature(old), normalize_signature(new));
    }
}

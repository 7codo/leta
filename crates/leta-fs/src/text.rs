use std::path::Path;

use fastrace::trace;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TextError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("UTF-8 decoding error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub fn get_language_id(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

    match ext {
        "py" | "pyi" => "python",
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" => "javascript",
        "jsx" => "javascriptreact",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" | "cxx" | "hxx" => "cpp",
        "java" => "java",
        "rb" | "rake" => "ruby",
        "php" | "phtml" => "php",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "ml" | "mli" => "ocaml",
        "lua" => "lua",
        "zig" => "zig",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "md" | "markdown" => "markdown",
        "toml" => "toml",
        "xml" => "xml",
        "sh" | "bash" => "shellscript",
        "sql" => "sql",
        "dummy-doesnt-exist" => "dummy-doesnt-exist",
        _ => match filename {
            "Gemfile" | "Rakefile" => "ruby",
            "Makefile" | "makefile" | "GNUmakefile" => "makefile",
            "Dockerfile" => "dockerfile",
            _ => "plaintext",
        },
    }
}

pub fn read_file_content(path: &Path) -> Result<String, TextError> {
    let bytes = std::fs::read(path)?;
    let content = String::from_utf8(bytes)?;
    Ok(content)
}

pub fn file_mtime(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(meta) => match meta.modified() {
            Ok(mtime) => match mtime.duration_since(std::time::UNIX_EPOCH) {
                Ok(duration) => format!("{}.{}", duration.as_secs(), duration.subsec_nanos()),
                Err(_) => String::new(),
            },
            Err(_) => String::new(),
        },
        Err(_) => String::new(),
    }
}

#[derive(Debug, Clone)]
pub struct LineSlice {
    pub start_line: u32,
    pub end_line: u32,
    pub total_lines: u32,
    pub content: String,
    pub truncated: bool,
    pub next_start_line: Option<u32>,
}

/// Extract an inclusive 1-based line range from `content`.
///
/// `end_line` defaults to the last line, and values past EOF are clamped. A
/// positive `head` limits the number of returned lines; `head == 0` means
/// unlimited.
#[trace]
pub fn slice_lines(
    content: &str,
    start_line: u32,
    end_line: Option<u32>,
    head: u32,
) -> Result<LineSlice, String> {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len() as u32;

    if start_line == 0 {
        return Err("--start-line must be >= 1".to_string());
    }
    if let Some(end) = end_line {
        if end < start_line {
            return Err(format!(
                "--end-line ({}) must be >= --start-line ({})",
                end, start_line
            ));
        }
    }

    if total_lines == 0 {
        if start_line > 1 {
            return Err(format!(
                "--start-line {} is beyond end of file (0 lines)",
                start_line
            ));
        }
        return Ok(LineSlice {
            start_line: 1,
            end_line: 0,
            total_lines: 0,
            content: String::new(),
            truncated: false,
            next_start_line: None,
        });
    }

    if start_line > total_lines {
        return Err(format!(
            "--start-line {} is beyond end of file ({} lines)",
            start_line, total_lines
        ));
    }

    let start = start_line;
    let mut end = end_line.unwrap_or(total_lines).min(total_lines);
    let requested = end - start + 1;
    let truncated = head > 0 && requested > head;
    if truncated {
        end = start + head - 1;
    }

    let content = lines[(start - 1) as usize..end as usize].join("\n");

    Ok(LineSlice {
        start_line: start,
        end_line: end,
        total_lines,
        content,
        truncated,
        next_start_line: if truncated { Some(end + 1) } else { None },
    })
}

#[trace]
pub fn get_lines_around(
    content: &str,
    center_line: usize,
    context: usize,
) -> (Vec<String>, usize, usize) {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    if total == 0 {
        return (vec![], 0, 0);
    }

    let center = center_line.min(total.saturating_sub(1));
    let start = center.saturating_sub(context);
    let end = (center + context).min(total.saturating_sub(1));

    let extracted: Vec<String> = lines[start..=end].iter().map(|s| s.to_string()).collect();
    (extracted, start, end)
}

#[trace]
pub fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_detection() {
        assert_eq!(get_language_id(Path::new("test.py")), "python");
        assert_eq!(get_language_id(Path::new("test.rs")), "rust");
        assert_eq!(get_language_id(Path::new("test.go")), "go");
        assert_eq!(get_language_id(Path::new("test.ts")), "typescript");
        assert_eq!(get_language_id(Path::new("Gemfile")), "ruby");
    }

    #[test]
    fn test_get_lines_around() {
        let content = "line0\nline1\nline2\nline3\nline4";
        let (lines, start, end) = get_lines_around(content, 2, 1);
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
        assert_eq!(start, 1);
        assert_eq!(end, 3);
    }

    #[test]
    fn slice_returns_whole_file() {
        let slice = slice_lines("alpha\nbeta\ngamma\n", 1, None, 0).unwrap();
        assert_eq!(slice.start_line, 1);
        assert_eq!(slice.end_line, 3);
        assert_eq!(slice.total_lines, 3);
        assert_eq!(slice.content, "alpha\nbeta\ngamma");
        assert!(!slice.truncated);
        assert_eq!(slice.next_start_line, None);
    }

    #[test]
    fn slice_returns_inclusive_range() {
        let slice = slice_lines("alpha\nbeta\ngamma\ndelta", 2, Some(3), 0).unwrap();
        assert_eq!(slice.start_line, 2);
        assert_eq!(slice.end_line, 3);
        assert_eq!(slice.content, "beta\ngamma");
    }

    #[test]
    fn slice_clamps_end_to_eof() {
        let slice = slice_lines("alpha\nbeta", 1, Some(99), 0).unwrap();
        assert_eq!(slice.end_line, 2);
        assert_eq!(slice.content, "alpha\nbeta");
    }

    #[test]
    fn slice_truncates_at_head() {
        let slice = slice_lines("alpha\nbeta\ngamma\ndelta", 2, None, 2).unwrap();
        assert_eq!(slice.start_line, 2);
        assert_eq!(slice.end_line, 3);
        assert_eq!(slice.content, "beta\ngamma");
        assert!(slice.truncated);
        assert_eq!(slice.next_start_line, Some(4));
    }

    #[test]
    fn slice_head_zero_is_unlimited() {
        let slice = slice_lines("alpha\nbeta\ngamma", 1, None, 0).unwrap();
        assert_eq!(slice.end_line, 3);
        assert!(!slice.truncated);
    }

    #[test]
    fn slice_empty_file() {
        let slice = slice_lines("", 1, None, 0).unwrap();
        assert_eq!(slice.start_line, 1);
        assert_eq!(slice.end_line, 0);
        assert_eq!(slice.total_lines, 0);
        assert_eq!(slice.content, "");
        assert!(!slice.truncated);
    }

    #[test]
    fn slice_rejects_zero_start_line() {
        let err = slice_lines("alpha", 0, None, 0).unwrap_err();
        assert_eq!(err, "--start-line must be >= 1");
    }

    #[test]
    fn slice_rejects_end_before_start() {
        let err = slice_lines("alpha\nbeta", 2, Some(1), 0).unwrap_err();
        assert_eq!(err, "--end-line (1) must be >= --start-line (2)");
    }

    #[test]
    fn slice_rejects_start_past_eof() {
        let err = slice_lines("alpha\nbeta", 3, None, 0).unwrap_err();
        assert_eq!(err, "--start-line 3 is beyond end of file (2 lines)");
    }

    #[test]
    fn slice_handles_crlf() {
        let slice = slice_lines("alpha\r\nbeta\r\ngamma\r\n", 2, Some(2), 0).unwrap();
        assert_eq!(slice.content, "beta");
    }
}

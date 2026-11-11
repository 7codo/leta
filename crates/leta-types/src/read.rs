use serde::{Deserialize, Serialize};

/// Result of reading a file, or a line range within a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResult {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub total_lines: u32,
    /// Selected lines joined with `\n`, without line numbers.
    pub content: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_start_line: Option<u32>,
}

impl ReadResult {
    pub fn new(
        path: String,
        start_line: u32,
        end_line: u32,
        total_lines: u32,
        content: String,
        truncated: bool,
        next_start_line: Option<u32>,
    ) -> Self {
        Self {
            path,
            start_line,
            end_line,
            total_lines,
            content,
            truncated,
            next_start_line,
        }
    }
}

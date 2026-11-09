/// Return the leading whitespace of a line.
pub(crate) fn leading_whitespace(line: &str) -> &str {
    let idx = line
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    &line[..idx]
}

/// Strip the text's common leading indentation and re-apply `base_indent`
/// to every non-blank line.
pub(crate) fn reindent(text: &str, base_indent: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let first_indent = lines
        .iter()
        .find(|line| !line.trim().is_empty())
        .map(|line| leading_whitespace(line).len())
        .unwrap_or(0);

    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                let indent = leading_whitespace(line);
                if indent.len() >= first_indent {
                    format!("{}{}", base_indent, &line[first_indent..])
                } else {
                    format!("{}{}", base_indent, line.trim_start())
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// True for lines that are decorators/attributes attached to the next item.
pub(crate) fn is_decorator_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('@') || trimmed.starts_with("#[") || trimmed.starts_with("#![")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reindents_top_level_text() {
        let replacement = "def foo():\n    return 1\n";
        assert_eq!(reindent(replacement, ""), "def foo():\n    return 1");
    }

    #[test]
    fn reindents_nested_method() {
        let replacement = "def save(self):\n    return 1\n";
        assert_eq!(
            reindent(replacement, "    "),
            "    def save(self):\n        return 1"
        );
    }

    #[test]
    fn keeps_already_indented_text() {
        let replacement = "    def save(self):\n        return 1\n";
        assert_eq!(
            reindent(replacement, "    "),
            "    def save(self):\n        return 1"
        );
    }

    #[test]
    fn detects_decorators() {
        assert!(is_decorator_line("@decorator"));
        assert!(is_decorator_line("  #[derive(Debug)]"));
        assert!(is_decorator_line("#![allow(dead_code)]"));
        assert!(!is_decorator_line("def foo():"));
    }
}

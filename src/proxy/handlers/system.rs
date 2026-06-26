//! System command handlers: ls, find, grep, cat.

use super::{ProxyContext, ProxyHandler};

// ---------------------------------------------------------------------------
// ls
// ---------------------------------------------------------------------------

pub struct LsHandler;

impl ProxyHandler for LsHandler {
    fn name(&self) -> &'static str {
        "ls"
    }

    fn matches(&self, program: &str, _args: &[String]) -> bool {
        program == "ls"
    }

    fn filter(
        &self,
        stdout: &str,
        _stderr: &str,
        _exit_code: i32,
        _args: &[String],
        ctx: Option<&ProxyContext>,
    ) -> String {
        let lines: Vec<&str> = stdout.lines().collect();
        if lines.len() <= 30 {
            let mut out = stdout.to_string();
            if let Some(ctx) = ctx {
                let annotations = ctx.render_annotations();
                if !annotations.is_empty() {
                    out.push('\n');
                    out.push_str(&annotations);
                }
            }
            return out;
        }

        // Group by extension
        let mut by_ext: std::collections::HashMap<String, Vec<&str>> =
            std::collections::HashMap::new();
        let mut dirs = Vec::new();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("total ") {
                continue;
            }
            // Extract filename from ls -l output or plain ls
            let fname = if trimmed.contains(' ') {
                trimmed.rsplit(' ').next().unwrap_or(trimmed)
            } else {
                trimmed
            };

            if fname.ends_with('/') || (trimmed.starts_with('d') && trimmed.contains(' ')) {
                dirs.push(fname);
            } else {
                let ext = fname.rsplit('.').next().unwrap_or("other").to_string();
                by_ext.entry(ext).or_default().push(fname);
            }
        }

        let mut out = String::new();
        if !dirs.is_empty() {
            out.push_str(&format!("dirs ({}): {}\n", dirs.len(), dirs.join(", ")));
        }

        let mut exts: Vec<_> = by_ext.iter().collect();
        exts.sort_by_key(|x| std::cmp::Reverse(x.1.len()));
        for (ext, files) in &exts {
            if files.len() <= 5 {
                out.push_str(&format!(".{ext} ({}): {}\n", files.len(), files.join(", ")));
            } else {
                out.push_str(&format!(
                    ".{ext} ({}): {}, ...\n",
                    files.len(),
                    files[..5].join(", ")
                ));
            }
        }

        out.push_str(&format!("total: {} items\n", lines.len()));
        out
    }
}

// ---------------------------------------------------------------------------
// find
// ---------------------------------------------------------------------------

pub struct FindHandler;

impl ProxyHandler for FindHandler {
    fn name(&self) -> &'static str {
        "find"
    }

    fn matches(&self, program: &str, _args: &[String]) -> bool {
        program == "find"
    }

    fn filter(
        &self,
        stdout: &str,
        _stderr: &str,
        _exit_code: i32,
        _args: &[String],
        _ctx: Option<&ProxyContext>,
    ) -> String {
        let lines: Vec<&str> = stdout.lines().collect();
        if lines.len() <= 50 {
            return stdout.to_string();
        }

        // Group by directory
        let mut by_dir: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let mut shown_lines = Vec::new();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let dir = trimmed.rsplit_once('/').map(|(d, _)| d).unwrap_or(".");
            *by_dir.entry(dir.to_string()).or_default() += 1;

            if shown_lines.len() < 30 {
                shown_lines.push(trimmed);
            }
        }

        let mut out = String::new();
        out.push_str(&format!(
            "{} results found. Summary by directory:\n",
            lines.len()
        ));

        let mut dirs: Vec<_> = by_dir.iter().collect();
        dirs.sort_by(|a, b| b.1.cmp(a.1));
        for (dir, count) in dirs.iter().take(15) {
            out.push_str(&format!("  {dir}/ ({count} files)\n"));
        }
        if dirs.len() > 15 {
            out.push_str(&format!("  ... and {} more directories\n", dirs.len() - 15));
        }

        out.push_str("\nFirst 30 results:\n");
        for line in &shown_lines {
            out.push_str(line);
            out.push('\n');
        }
        if lines.len() > 30 {
            out.push_str(&format!("... {} more\n", lines.len() - 30));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// grep
// ---------------------------------------------------------------------------

pub struct GrepHandler;

const GREP_MAX_RESULTS: usize = 100;
const GREP_MAX_PER_FILE: usize = 10;
const GREP_MAX_FILES_DETAIL: usize = 15;

impl ProxyHandler for GrepHandler {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn matches(&self, program: &str, _args: &[String]) -> bool {
        program == "grep" || program == "rg" || program == "ag"
    }

    fn filter(
        &self,
        stdout: &str,
        _stderr: &str,
        _exit_code: i32,
        _args: &[String],
        _ctx: Option<&ProxyContext>,
    ) -> String {
        let lines: Vec<&str> = stdout.lines().collect();
        if lines.len() <= GREP_MAX_RESULTS {
            return stdout.to_string();
        }

        // Group by file
        let mut by_file: std::collections::HashMap<String, Vec<&str>> =
            std::collections::HashMap::new();
        let mut no_file_lines = Vec::new();

        for line in &lines {
            if let Some((file, _rest)) = line.split_once(':')
                && (file.contains('/') || file.contains('.'))
            {
                by_file.entry(file.to_string()).or_default().push(line);
                continue;
            }
            no_file_lines.push(*line);
        }

        let mut out = String::new();
        out.push_str(&format!(
            "{} matches across {} files\n\n",
            lines.len(),
            by_file.len()
        ));

        let mut files: Vec<_> = by_file.iter().collect();
        files.sort_by_key(|x| std::cmp::Reverse(x.1.len()));

        // For very large result sets (>500 lines), summary-only mode:
        // show file names + counts, detail only for top files
        let detail_files = if lines.len() > 500 {
            GREP_MAX_FILES_DETAIL.min(5)
        } else {
            GREP_MAX_FILES_DETAIL
        };

        for (i, (file, matches)) in files.iter().enumerate() {
            if i < detail_files {
                let shown = matches.len().min(GREP_MAX_PER_FILE);
                out.push_str(&format!("--- {file} ({} matches) ---\n", matches.len()));
                for m in matches.iter().take(shown) {
                    out.push_str(m);
                    out.push('\n');
                }
                if matches.len() > GREP_MAX_PER_FILE {
                    out.push_str(&format!(
                        "  ... {} more in this file\n",
                        matches.len() - GREP_MAX_PER_FILE
                    ));
                }
            } else {
                // Summary line only
                out.push_str(&format!("  {file} ({} matches)\n", matches.len()));
            }
        }

        for line in no_file_lines.iter().take(10) {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

// ---------------------------------------------------------------------------
// cat (smart read — tree-sitter AST body strip with brace-count fallback)
// ---------------------------------------------------------------------------

pub struct CatHandler;

impl ProxyHandler for CatHandler {
    fn name(&self) -> &'static str {
        "cat"
    }

    fn matches(&self, program: &str, _args: &[String]) -> bool {
        program == "cat"
    }

    fn filter(
        &self,
        stdout: &str,
        _stderr: &str,
        _exit_code: i32,
        args: &[String],
        _ctx: Option<&ProxyContext>,
    ) -> String {
        let lines: Vec<&str> = stdout.lines().collect();
        if lines.len() <= 100 {
            return stdout.to_string();
        }

        // Try tree-sitter path: extract file path from args, detect language
        let file_path = args.first().map(|s| s.as_str()).unwrap_or("");
        if let Some(result) = treesitter_strip(stdout, file_path) {
            return result;
        }

        // Fallback: brace-counting heuristic (for unsupported languages)
        brace_count_strip(stdout)
    }
}

fn treesitter_strip(source: &str, file_path: &str) -> Option<String> {
    use crate::indexer::parser::{MultiParser, detect_language};

    let lang = detect_language(file_path)?;
    let parser = MultiParser::new();
    let tree = parser.parse(source, &lang).ok()?;
    let root = tree.root_node();

    // Collect byte ranges of function/method bodies to elide
    let mut body_ranges: Vec<(usize, usize, usize)> = Vec::new(); // (start_byte, end_byte, line_count)
    collect_body_ranges(root, source.as_bytes(), &mut body_ranges);

    if body_ranges.is_empty() {
        return None;
    }

    // Build output: copy source but replace body ranges with placeholders
    let bytes = source.as_bytes();
    let mut out = String::new();
    let mut cursor = 0usize;

    for (start, end, line_count) in &body_ranges {
        if *start <= cursor {
            continue; // nested body, already elided by parent
        }
        // Copy everything before this body
        if let Ok(before) = std::str::from_utf8(&bytes[cursor..*start]) {
            out.push_str(before);
        }
        // Insert placeholder
        // Find the indentation of the opening brace line
        let indent = find_indent_at(source, *start);
        out.push_str(&format!("{indent}// ... {line_count} lines\n"));
        // Find the closing brace and include it
        if *end <= bytes.len() {
            // Include the closing line (e.g., "}")
            let close_line_start = bytes[..*end]
                .iter()
                .rposition(|&b| b == b'\n')
                .map(|p| p + 1)
                .unwrap_or(*end);
            if let Ok(close) = std::str::from_utf8(&bytes[close_line_start..*end]) {
                out.push_str(close);
            }
        }
        cursor = *end;
    }

    // Copy remaining
    if cursor < bytes.len()
        && let Ok(rest) = std::str::from_utf8(&bytes[cursor..])
    {
        out.push_str(rest);
    }

    if out.trim().is_empty() {
        return None;
    }
    Some(out)
}

fn collect_body_ranges(
    node: tree_sitter::Node,
    source: &[u8],
    ranges: &mut Vec<(usize, usize, usize)>,
) {
    // Node kinds that contain function/method bodies we want to elide
    let is_fn_like = matches!(
        node.kind(),
        "function_item"
            | "function_definition"
            | "method_definition"
            | "method_declaration"
            | "function_declaration"
            | "func_literal"
            | "arrow_function"
            | "closure_expression"
    );

    if is_fn_like {
        // Find the body child node
        if let Some(body) = node.child_by_field_name("body") {
            let start = body.start_byte();
            let end = body.end_byte();
            let body_lines = source[start..end].iter().filter(|&&b| b == b'\n').count();
            // Only elide bodies > 3 lines (keep short functions readable)
            if body_lines > 3 {
                ranges.push((start, end, body_lines));
                return; // Don't recurse into elided bodies
            }
        }
    }

    // Recurse into children
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            collect_body_ranges(cursor.node(), source, ranges);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn find_indent_at(source: &str, byte_pos: usize) -> String {
    let before = &source[..byte_pos];
    let line_start = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line = &source[line_start..byte_pos];
    let indent_len = line.len() - line.trim_start().len();
    " ".repeat(indent_len)
}

fn brace_count_strip(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::new();
    let mut brace_depth: i32 = 0;
    let mut in_body = false;
    let mut body_start_line = 0usize;
    let mut line_num = 0usize;

    for line in &lines {
        line_num += 1;
        let trimmed = line.trim();
        let opens: i32 = trimmed.matches('{').count() as i32;
        let closes: i32 = trimmed.matches('}').count() as i32;

        if !in_body {
            out.push_str(line);
            out.push('\n');
            if opens > closes {
                brace_depth += opens - closes;
                if brace_depth >= 2 {
                    in_body = true;
                    body_start_line = line_num;
                }
            } else {
                brace_depth = (brace_depth + opens - closes).max(0);
            }
        } else {
            brace_depth += opens - closes;
            if brace_depth <= 1 {
                let skipped = line_num - body_start_line;
                if skipped > 0 {
                    out.push_str(&format!("    // ... {skipped} lines\n"));
                }
                out.push_str(line);
                out.push('\n');
                in_body = false;
            }
        }
    }

    if out.trim().is_empty() && !source.trim().is_empty() {
        return source.to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_small_passthrough() {
        let h = LsHandler;
        let out = h.filter("file1.rs\nfile2.rs\n", "", 0, &[], None);
        assert_eq!(out, "file1.rs\nfile2.rs\n");
    }

    #[test]
    fn grep_caps_results() {
        let h = GrepHandler;
        let mut input = String::new();
        for i in 0..300 {
            input.push_str(&format!("src/file{}.rs:10: match {i}\n", i % 5));
        }
        let out = h.filter(&input, "", 0, &[], None);
        assert!(out.contains("300 matches"));
        assert!(out.contains("5 files"));
    }

    #[test]
    fn cat_strips_bodies() {
        let h = CatHandler;
        let mut input = String::new();
        input.push_str("pub struct Foo {\n");
        input.push_str("    x: i32,\n");
        input.push_str("}\n");
        input.push_str("impl Foo {\n");
        input.push_str("    pub fn new() -> Self {\n");
        // Add >100 lines to trigger compression
        for i in 0..110 {
            input.push_str(&format!("        let x{i} = {i};\n"));
        }
        input.push_str("    }\n");
        input.push_str("}\n");
        let out = h.filter(&input, "", 0, &[], None);
        assert!(out.contains("... "));
        assert!(out.len() < input.len());
    }
}

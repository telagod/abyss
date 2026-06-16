//! Entry-point detection — language-aware "this file starts the program"
//! signal. High confidence (weight 1.0 by convention in the fusion layer);
//! callers should let an entry-point match override most other layer hints.
//!
//! The detector accepts `(path, content, lang)` so it can combine filename
//! conventions with a quick content scan. Reading file content is expensive
//! at scale — the integrator is expected to gate calls behind a cheap path
//! pre-filter (e.g. only files whose basename matches `main.*` / `index.*` /
//! `__main__.py`). This module purposefully does no I/O.

/// Language tags recognised by `is_entry_point`. Anything else returns
/// `false`. The strings match the language IDs produced by
/// `code_abyss::indexer::parser::detect_language`.
const LANG_GO: &str = "go";
const LANG_RUST: &str = "rust";
const LANG_PYTHON: &str = "python";
const LANG_JS: &str = "javascript";
const LANG_TS: &str = "typescript";
const LANG_TSX: &str = "tsx";
const LANG_JAVA: &str = "java";
const LANG_C: &str = "c";
const LANG_CPP: &str = "cpp";

/// Detect whether `rel_path` is an executable entry point for `lang`.
///
/// `rel_path` is treated as a forward-slash path; backslashes are
/// normalised. `file_content` is the raw source as UTF-8.
pub fn is_entry_point(rel_path: &str, file_content: &str, lang: &str) -> bool {
    let path = rel_path.replace('\\', "/");
    let basename = path.rsplit('/').next().unwrap_or(&path);

    // Universal "this is inside a test tree" exclusion — entry-point heuristics
    // should never trigger from test fixtures.
    if is_in_tests(&path) {
        return false;
    }

    match lang {
        LANG_GO => detect_go(basename, file_content),
        LANG_RUST => detect_rust(&path, basename, file_content),
        LANG_PYTHON => detect_python(&path, basename, file_content),
        LANG_JS | LANG_TS | LANG_TSX => detect_js_ts(basename, file_content),
        LANG_JAVA => detect_java(file_content),
        LANG_C | LANG_CPP => detect_c_cpp(file_content),
        _ => false,
    }
}

fn is_in_tests(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/__tests__/")
        || lower.starts_with("__tests__/")
}

fn detect_go(basename: &str, content: &str) -> bool {
    basename == "main.go" && contains_token(content, "func main()")
}

fn detect_rust(path: &str, basename: &str, content: &str) -> bool {
    if !contains_fn_main(content) {
        return false;
    }
    if basename == "main.rs" {
        return true;
    }
    // Common Cargo layouts: src/bin/foo.rs, examples/foo.rs
    path.contains("/bin/") || path.starts_with("bin/")
}

fn detect_python(path: &str, basename: &str, content: &str) -> bool {
    if basename == "__main__.py" {
        return true;
    }
    // Skip obvious non-entry locations
    if path.contains("/site-packages/") {
        return false;
    }
    contains_dunder_main(content)
}

fn detect_js_ts(basename: &str, content: &str) -> bool {
    let is_index_or_main = matches!(
        basename,
        "index.ts" | "index.js" | "index.tsx" | "index.jsx" | "main.ts" | "main.js"
    );
    if !is_index_or_main {
        return false;
    }
    // Shebang wins — definitely a script.
    if content.starts_with("#!") {
        return true;
    }
    // Heuristic: if there's no top-level `export`, treat as an entry. This
    // intentionally errs toward false positives only for index.*/main.*
    // filenames (per task spec).
    !has_top_level_export(content)
}

fn detect_java(content: &str) -> bool {
    // Java mains: `public static void main(String[] args)` and variants. We
    // tolerate whitespace and either `String[]` or `String...`.
    contains_java_main(content)
}

fn detect_c_cpp(content: &str) -> bool {
    contains_c_main(content)
}

// ─── Lightweight token scanners ────────────────────────────────────────────

/// Skip over a `//` line comment, `/* … */` block comment, or `"…"` /
/// `'…'` string literal starting at byte `i`. Returns the new cursor
/// position, or `None` if `i` does not begin one of those tokens.
fn skip_comment_or_string(src: &[u8], i: usize) -> Option<usize> {
    if i + 1 >= src.len() {
        return None;
    }
    match (src[i], src[i + 1]) {
        (b'/', b'/') => {
            let mut j = i + 2;
            while j < src.len() && src[j] != b'\n' {
                j += 1;
            }
            Some(j)
        }
        (b'/', b'*') => {
            let mut j = i + 2;
            while j + 1 < src.len() {
                if src[j] == b'*' && src[j + 1] == b'/' {
                    return Some(j + 2);
                }
                j += 1;
            }
            Some(src.len())
        }
        _ => {
            let q = src[i];
            if q == b'"' || q == b'\'' {
                let mut j = i + 1;
                while j < src.len() {
                    if src[j] == b'\\' && j + 1 < src.len() {
                        j += 2;
                        continue;
                    }
                    if src[j] == q {
                        return Some(j + 1);
                    }
                    j += 1;
                }
                Some(src.len())
            } else {
                None
            }
        }
    }
}

fn contains_token(content: &str, needle: &str) -> bool {
    let src = content.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut i = 0;
    while i < src.len() {
        if let Some(next) = skip_comment_or_string(src, i) {
            i = next;
            continue;
        }
        if src[i..].starts_with(needle_bytes) {
            return true;
        }
        i += 1;
    }
    false
}

/// Detect `fn main(` allowing arbitrary whitespace between `fn` and `main`.
fn contains_fn_main(content: &str) -> bool {
    scan_with_pattern(content, |src, i| {
        if !src[i..].starts_with(b"fn") {
            return false;
        }
        let mut j = i + 2;
        if j >= src.len() || !src[j].is_ascii_whitespace() {
            return false;
        }
        while j < src.len() && src[j].is_ascii_whitespace() {
            j += 1;
        }
        src[j..].starts_with(b"main(")
    })
}

/// Detect `if __name__ == "__main__":` and `'__main__'` variants.
fn contains_dunder_main(content: &str) -> bool {
    scan_with_pattern(content, |src, i| {
        if !src[i..].starts_with(b"if") {
            return false;
        }
        let mut j = i + 2;
        if j >= src.len() || !src[j].is_ascii_whitespace() {
            return false;
        }
        while j < src.len() && src[j].is_ascii_whitespace() {
            j += 1;
        }
        if !src[j..].starts_with(b"__name__") {
            return false;
        }
        j += b"__name__".len();
        while j < src.len() && src[j].is_ascii_whitespace() {
            j += 1;
        }
        if !src[j..].starts_with(b"==") {
            return false;
        }
        j += 2;
        while j < src.len() && src[j].is_ascii_whitespace() {
            j += 1;
        }
        src[j..].starts_with(b"\"__main__\"") || src[j..].starts_with(b"'__main__'")
    })
}

/// Detect `public static void main(` allowing extra whitespace and the
/// `String...`/`String[]` parameter forms.
fn contains_java_main(content: &str) -> bool {
    scan_with_pattern(content, |src, i| {
        if !src[i..].starts_with(b"public") {
            return false;
        }
        let mut j = i + b"public".len();
        if !consume_ws_required(src, &mut j) {
            return false;
        }
        if !src[j..].starts_with(b"static") {
            return false;
        }
        j += b"static".len();
        if !consume_ws_required(src, &mut j) {
            return false;
        }
        if !src[j..].starts_with(b"void") {
            return false;
        }
        j += b"void".len();
        if !consume_ws_required(src, &mut j) {
            return false;
        }
        src[j..].starts_with(b"main(")
    })
}

/// Detect `int main(` (C/C++). We require the surrounding context to look
/// like a definition — preceding char is start-of-file, whitespace, `}` or
/// `;` — to suppress false hits from `int main_loop(`.
fn contains_c_main(content: &str) -> bool {
    scan_with_pattern(content, |src, i| {
        // Boundary check on the preceding byte.
        let preceded_by_boundary = if i == 0 {
            true
        } else {
            let prev = src[i - 1];
            prev.is_ascii_whitespace() || prev == b'}' || prev == b';' || prev == b')'
        };
        if !preceded_by_boundary {
            return false;
        }
        if !src[i..].starts_with(b"int") {
            return false;
        }
        let mut j = i + 3;
        if !consume_ws_required(src, &mut j) {
            return false;
        }
        src[j..].starts_with(b"main(")
    })
}

fn consume_ws_required(src: &[u8], j: &mut usize) -> bool {
    if *j >= src.len() || !src[*j].is_ascii_whitespace() {
        return false;
    }
    while *j < src.len() && src[*j].is_ascii_whitespace() {
        *j += 1;
    }
    true
}

/// Generic scanner: skip comments + strings, then invoke `matcher(src, i)`
/// at every other byte. Returns `true` on first match.
fn scan_with_pattern<F>(content: &str, matcher: F) -> bool
where
    F: Fn(&[u8], usize) -> bool,
{
    let src = content.as_bytes();
    let mut i = 0;
    while i < src.len() {
        if let Some(next) = skip_comment_or_string(src, i) {
            i = next;
            continue;
        }
        if matcher(src, i) {
            return true;
        }
        i += 1;
    }
    false
}

/// Cheap heuristic for "this module exports something". We look for a
/// line-start `export` keyword that is not inside a string/comment. Used
/// only as a tiebreaker for index.* / main.* filenames.
fn has_top_level_export(content: &str) -> bool {
    let src = content.as_bytes();
    let mut i = 0;
    let mut at_line_start = true;
    while i < src.len() {
        if let Some(next) = skip_comment_or_string(src, i) {
            i = next;
            at_line_start = false;
            continue;
        }
        if src[i] == b'\n' {
            at_line_start = true;
            i += 1;
            continue;
        }
        if at_line_start && src[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if at_line_start && src[i..].starts_with(b"export") {
            let after = i + b"export".len();
            if after >= src.len() || !src[after].is_ascii_alphanumeric() {
                return true;
            }
        }
        at_line_start = false;
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_main_detected() {
        let src = "package main\n\nfunc main() {\n    println(\"hi\")\n}\n";
        assert!(is_entry_point("cmd/server/main.go", src, "go"));
    }

    #[test]
    fn go_non_main_filename_rejected() {
        let src = "package main\n\nfunc main() {}\n";
        assert!(!is_entry_point("cmd/server/helper.go", src, "go"));
    }

    #[test]
    fn go_main_word_in_comment_rejected() {
        let src = "package foo\n\n// func main() lives elsewhere\nfunc Run() {}\n";
        assert!(!is_entry_point("cmd/server/main.go", src, "go"));
    }

    #[test]
    fn rust_main_rs_detected() {
        let src = "fn main() { println!(\"hi\"); }";
        assert!(is_entry_point("src/main.rs", src, "rust"));
    }

    #[test]
    fn rust_bin_subdir_detected() {
        let src = "fn main() {}";
        assert!(is_entry_point("src/bin/server.rs", src, "rust"));
    }

    #[test]
    fn rust_lib_rs_rejected() {
        let src = "pub fn helper() {}";
        assert!(!is_entry_point("src/lib.rs", src, "rust"));
    }

    #[test]
    fn rust_main_with_whitespace_detected() {
        let src = "fn    main() {}";
        assert!(is_entry_point("src/main.rs", src, "rust"));
    }

    #[test]
    fn rust_main_in_string_rejected() {
        let src = r#"pub const HINT: &str = "fn main()";"#;
        assert!(!is_entry_point("src/lib.rs", src, "rust"));
    }

    #[test]
    fn python_dunder_main_file_detected() {
        let src = "from .cli import run\nrun()\n";
        assert!(is_entry_point("mypkg/__main__.py", src, "python"));
    }

    #[test]
    fn python_if_main_guard_detected() {
        let src = "def main():\n    pass\n\nif __name__ == \"__main__\":\n    main()\n";
        assert!(is_entry_point("scripts/run.py", src, "python"));
    }

    #[test]
    fn python_main_in_tests_rejected() {
        let src = "if __name__ == \"__main__\":\n    pass\n";
        assert!(!is_entry_point("tests/test_foo.py", src, "python"));
    }

    #[test]
    fn ts_index_no_export_detected() {
        let src = "import { run } from './cli';\nrun();\n";
        assert!(is_entry_point("src/index.ts", src, "typescript"));
    }

    #[test]
    fn ts_index_with_export_rejected() {
        let src = "export function foo() {}\n";
        assert!(!is_entry_point("src/index.ts", src, "typescript"));
    }

    #[test]
    fn js_main_with_shebang_detected() {
        let src = "#!/usr/bin/env node\nconsole.log('hi');\n";
        assert!(is_entry_point("bin/main.js", src, "javascript"));
    }

    #[test]
    fn java_main_detected() {
        let src = "class App { public static void main(String[] args) {} }";
        assert!(is_entry_point("src/main/java/App.java", src, "java"));
    }

    #[test]
    fn java_main_extra_whitespace_detected() {
        let src = "class App { public  static   void main(String[] args) {} }";
        assert!(is_entry_point("App.java", src, "java"));
    }

    #[test]
    fn java_no_main_rejected() {
        let src = "class Util { static int helper() { return 1; } }";
        assert!(!is_entry_point("Util.java", src, "java"));
    }

    #[test]
    fn c_main_detected() {
        let src = "#include <stdio.h>\nint main(int argc, char** argv) { return 0; }\n";
        assert!(is_entry_point("src/main.c", src, "c"));
    }

    #[test]
    fn c_main_loop_lookalike_rejected() {
        // `int main_loop(` must NOT trigger.
        let src = "int main_loop(void) { return 0; }\n";
        assert!(!is_entry_point("src/loop.c", src, "c"));
    }

    #[test]
    fn cpp_main_detected() {
        let src = "int main() { return 0; }";
        assert!(is_entry_point("app.cpp", src, "cpp"));
    }

    #[test]
    fn unknown_language_returns_false() {
        let src = "anything";
        assert!(!is_entry_point("foo.txt", src, "plaintext"));
    }

    #[test]
    fn tests_path_excluded_for_all_langs() {
        let src = "int main() { return 0; }";
        assert!(!is_entry_point("tests/main.c", src, "c"));
    }

    #[test]
    fn windows_path_is_normalized() {
        let src = "fn main() {}";
        assert!(is_entry_point("src\\main.rs", src, "rust"));
    }
}

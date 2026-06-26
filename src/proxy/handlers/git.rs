//! Git command handlers: status, diff, log.

use super::{ProxyContext, ProxyHandler};

// ---------------------------------------------------------------------------
// git status — i18n aware (en / zh / ja / ko / de / fr / es / pt)
// ---------------------------------------------------------------------------

pub struct GitStatusHandler;

impl ProxyHandler for GitStatusHandler {
    fn name(&self) -> &'static str {
        "git-status"
    }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "git" && args.first().map(|s| s.as_str()) == Some("status")
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
        if lines.is_empty() {
            return "working tree clean".into();
        }

        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        let mut untracked = Vec::new();
        let mut section = Section::None;

        for line in &lines {
            let trimmed = line.trim();

            // Section headers — multi-locale (unstaged checked BEFORE staged
            // because zh "尚未暂存以备提交的变更" is a superset of "以备提交的变更")
            if is_unstaged_header(trimmed) {
                section = Section::Unstaged;
                continue;
            }
            if is_staged_header(trimmed) {
                section = Section::Staged;
                continue;
            }
            if is_untracked_header(trimmed) {
                section = Section::Untracked;
                continue;
            }

            // Skip hint lines in all locales
            if is_hint_line(trimmed) || trimmed.is_empty() {
                continue;
            }

            match section {
                Section::Staged => staged.push(trimmed),
                Section::Unstaged => unstaged.push(trimmed),
                Section::Untracked => untracked.push(trimmed),
                Section::None => {}
            }
        }

        let mut out = String::new();

        // Branch line (multi-locale)
        if let Some(first) = lines.first()
            && is_branch_line(first.trim())
        {
            out.push_str(first.trim());
            out.push('\n');
        }

        if !staged.is_empty() {
            out.push_str(&format!("staged ({}): ", staged.len()));
            out.push_str(&staged.join(", "));
            out.push('\n');
        }
        if !unstaged.is_empty() {
            out.push_str(&format!("unstaged ({}): ", unstaged.len()));
            out.push_str(&unstaged.join(", "));
            out.push('\n');
        }
        if !untracked.is_empty() {
            if untracked.len() <= 10 {
                out.push_str(&format!("untracked ({}): ", untracked.len()));
                out.push_str(&untracked.join(", "));
                out.push('\n');
            } else {
                out.push_str(&format!(
                    "untracked: {} files (first 10): ",
                    untracked.len()
                ));
                out.push_str(&untracked[..10].join(", "));
                out.push_str(" ...\n");
            }
        }

        if out.is_empty() {
            return stdout.to_string();
        }

        if let Some(ctx) = ctx {
            let annotations = ctx.render_annotations();
            if !annotations.is_empty() {
                out.push_str(&annotations);
            }
        }
        out
    }
}

fn is_branch_line(s: &str) -> bool {
    s.starts_with("On branch")
        || s.starts_with("位于分支")       // zh
        || s.starts_with("ブランチ")       // ja
        || s.starts_with("브랜치")         // ko
        || s.starts_with("Auf Branch")    // de
        || s.starts_with("Sur la branche") // fr
        || s.starts_with("En la rama")    // es
        || s.starts_with("No ramo") // pt
}

fn is_staged_header(s: &str) -> bool {
    s.starts_with("Changes to be committed")
        || s.contains("要提交的变更")      // zh
        || s.contains("以备提交的变更")
        || s.starts_with("コミット予定") // ja
        || s.starts_with("커밋할 변경")  // ko
        || s.starts_with("Änderungen, die committet werden") // de
        || s.starts_with("Modifications qui seront validées") // fr
}

fn is_unstaged_header(s: &str) -> bool {
    s.starts_with("Changes not staged")
        || s.contains("尚未暂存")          // zh
        || s.starts_with("ステージされていない") // ja
        || s.starts_with("커밋하도록 정하지 않은") // ko
        || s.starts_with("Änderungen, die nicht zum Commit vorgemerkt") // de
        || s.starts_with("Modifications qui ne seront pas validées") // fr
}

fn is_untracked_header(s: &str) -> bool {
    s.starts_with("Untracked files")
        || s.starts_with("未跟踪的文件")   // zh
        || s.starts_with("追跡されていない") // ja
        || s.starts_with("추적하지 않는 파일") // ko
        || s.starts_with("Unversionierte Dateien") // de
        || s.starts_with("Fichiers non suivis") // fr
}

fn is_hint_line(s: &str) -> bool {
    s.starts_with("(use ")
        || s.starts_with("（使用")         // zh (fullwidth parens)
        || s.starts_with("(使用")
        || s.starts_with("(utilice")      // es
        || s.starts_with("(utilisez")     // fr
        || s.starts_with("(benutzen Sie") // de
        || s.starts_with("(usar")         // pt
        || s.starts_with("Your branch is up to date")
        || s.starts_with("您的分支与上游分支")
        || s.starts_with("nothing to commit")
        || s.starts_with("无文件要提交")
        || s.starts_with("修改尚未加入提交")
}

#[derive(Clone, Copy)]
enum Section {
    None,
    Staged,
    Unstaged,
    Untracked,
}

// ---------------------------------------------------------------------------
// git diff — aggressive context reduction for large diffs
// ---------------------------------------------------------------------------

pub struct GitDiffHandler;

impl ProxyHandler for GitDiffHandler {
    fn name(&self) -> &'static str {
        "git-diff"
    }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "git" && args.first().map(|s| s.as_str()) == Some("diff")
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
        if lines.is_empty() {
            return "no diff".into();
        }

        // Determine total diff size for adaptive context budget
        let total_changed = lines
            .iter()
            .filter(|l| l.starts_with('+') || l.starts_with('-'))
            .count();
        let context_budget: usize = if total_changed > 200 {
            0 // huge diff: zero context lines
        } else if total_changed > 50 {
            1 // medium diff: 1 context line
        } else {
            2 // small diff: 2 context lines
        };

        let mut out = String::new();
        let mut current_file: Option<String> = None;
        let mut file_lines: Vec<&str> = Vec::new();

        for line in &lines {
            if line.starts_with("diff --git") {
                if let Some(ref file) = current_file {
                    flush_diff_file(&mut out, file, &file_lines, ctx, context_budget);
                }
                let fname = line.split(" b/").nth(1).unwrap_or("unknown").to_string();
                current_file = Some(fname);
                file_lines.clear();
                continue;
            }
            if line.starts_with("index ")
                || line.starts_with("---")
                || line.starts_with("+++")
                || line.starts_with("old mode")
                || line.starts_with("new mode")
                || line.starts_with("similarity index")
                || line.starts_with("rename from")
                || line.starts_with("rename to")
            {
                continue;
            }
            file_lines.push(line);
        }

        if let Some(ref file) = current_file {
            flush_diff_file(&mut out, file, &file_lines, ctx, context_budget);
        }

        if out.is_empty() {
            return stdout.to_string();
        }
        out
    }
}

fn flush_diff_file(
    out: &mut String,
    file: &str,
    lines: &[&str],
    ctx: Option<&ProxyContext>,
    context_budget: usize,
) {
    let added = lines.iter().filter(|l| l.starts_with('+')).count();
    let removed = lines.iter().filter(|l| l.starts_with('-')).count();

    out.push_str(&format!("--- {file} (+{added}/-{removed}) ---\n"));

    if let Some(ctx) = ctx {
        for (f, count) in &ctx.impacted_callers {
            if f == file && *count >= 3 {
                out.push_str(&format!("  ⚠ {count} callers\n"));
            }
        }
    }

    // For files with >100 changed lines, show first 30 + last 10 only
    let changed_count = added + removed;
    if changed_count > 100 {
        let mut shown = 0;
        let change_lines: Vec<&str> = lines
            .iter()
            .filter(|l| l.starts_with('+') || l.starts_with('-') || l.starts_with("@@"))
            .copied()
            .collect();
        let head = change_lines.len().min(30);
        let tail = change_lines.len().min(10);

        for line in &change_lines[..head] {
            out.push_str(line);
            out.push('\n');
            shown += 1;
        }
        if change_lines.len() > head + tail {
            out.push_str(&format!(
                "  ... ({} lines omitted)\n",
                change_lines.len() - head - tail
            ));
        }
        if change_lines.len() > head {
            let start = change_lines.len().saturating_sub(tail);
            for line in &change_lines[start.max(head)..] {
                out.push_str(line);
                out.push('\n');
                shown += 1;
            }
        }
        let _ = shown;
        return;
    }

    // Normal diff: keep hunks + changed lines + limited context
    let mut context_count: usize = 0;
    for line in lines {
        if line.starts_with("@@") || line.starts_with('+') || line.starts_with('-') {
            out.push_str(line);
            out.push('\n');
            context_count = 0;
        } else {
            context_count += 1;
            if context_count <= context_budget {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
}

// ---------------------------------------------------------------------------
// git log — compress long commit logs
// ---------------------------------------------------------------------------

pub struct GitLogHandler;

impl ProxyHandler for GitLogHandler {
    fn name(&self) -> &'static str {
        "git-log"
    }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "git" && args.first().map(|s| s.as_str()) == Some("log")
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
        if lines.len() <= 40 {
            return stdout.to_string();
        }

        let mut out = String::new();
        let mut commit_count = 0;
        let total_commits = lines
            .iter()
            .filter(|l| l.trim().starts_with("commit "))
            .count();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("commit ") {
                commit_count += 1;
                if commit_count > 20 {
                    out.push_str(&format!("... ({} more commits)\n", total_commits - 20));
                    break;
                }
                // Short hash only
                let hash = trimmed
                    .strip_prefix("commit ")
                    .unwrap_or("")
                    .get(..10)
                    .unwrap_or("");
                out.push_str(&format!("commit {hash}\n"));
            } else if trimmed.starts_with("Author:") {
                let short = trimmed.split('<').next().unwrap_or(trimmed).trim_end();
                out.push_str(short);
                out.push('\n');
            } else if !trimmed.is_empty()
                && !trimmed.starts_with("Date:")
                && !trimmed.starts_with("Merge:")
            {
                if trimmed.len() > 100 {
                    out.push_str(&trimmed[..100]);
                    out.push_str("...\n");
                } else {
                    out.push_str(trimmed);
                    out.push('\n');
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_status_clean() {
        let h = GitStatusHandler;
        let out = h.filter("", "", 0, &[], None);
        assert_eq!(out, "working tree clean");
    }

    #[test]
    fn git_status_compresses_en() {
        let raw = "\
On branch main
Changes to be committed:
  (use \"git restore --staged <file>...\" to unstage)
\tnew file:   src/proxy/mod.rs
\tnew file:   src/proxy/runner.rs

Changes not staged for commit:
  (use \"git add <file>...\" to update what will be committed)
\tmodified:   src/main.rs
";
        let h = GitStatusHandler;
        let out = h.filter(raw, "", 0, &[], None);
        assert!(out.contains("staged (2):"));
        assert!(out.contains("unstaged (1):"));
        assert!(!out.contains("use \"git restore"));
    }

    #[test]
    fn git_status_compresses_zh() {
        let raw = "\
位于分支 main
您的分支与上游分支 'origin/main' 一致。

尚未暂存以备提交的变更：
  （使用 \"git add <文件>...\" 更新要提交的内容）
  （使用 \"git restore <文件>...\" 丢弃工作区的改动）
\t修改：     src/lib.rs
\t修改：     src/main.rs

未跟踪的文件:
  （使用 \"git add <文件>...\" 以包含要提交的内容）
\t.claude/
\tsrc/proxy/

修改尚未加入提交（使用 \"git add\" 和/或 \"git commit -a\"）
";
        let h = GitStatusHandler;
        let out = h.filter(raw, "", 0, &[], None);
        assert!(
            out.contains("位于分支 main"),
            "should keep branch line: {out}"
        );
        assert!(
            out.contains("unstaged (2):"),
            "should detect unstaged: {out}"
        );
        assert!(
            out.contains("untracked (2):"),
            "should detect untracked: {out}"
        );
        assert!(!out.contains("使用"), "should strip hint lines: {out}");
        assert!(
            !out.contains("修改尚未加入提交"),
            "should strip footer hint: {out}"
        );
    }

    #[test]
    fn git_diff_summary() {
        let raw = "\
diff --git a/src/lib.rs b/src/lib.rs
index abc..def 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub mod arch;
+pub mod proxy;
 pub mod attach;
";
        let h = GitDiffHandler;
        let out = h.filter(raw, "", 0, &[], None);
        assert!(out.contains("src/lib.rs (+1/-0)"));
        assert!(out.contains("+pub mod proxy;"));
    }

    #[test]
    fn git_diff_adaptive_context() {
        // Build a medium-sized diff (>50 changes) to trigger context=1
        let mut raw = String::from(
            "diff --git a/big.rs b/big.rs\nindex abc..def 100644\n--- a/big.rs\n+++ b/big.rs\n@@ -1,100 +1,100 @@\n",
        );
        for i in 0..60 {
            raw.push_str(&format!("+line {i}\n"));
            raw.push_str(&format!("-old line {i}\n"));
            raw.push_str(" context unchanged\n");
            raw.push_str(" more context\n");
            raw.push_str(" even more context\n");
        }
        let h = GitDiffHandler;
        let out = h.filter(&raw, "", 0, &[], None);
        // Should have fewer context lines than the raw input
        let raw_context = raw.lines().filter(|l| l.starts_with(' ')).count();
        let out_context = out.lines().filter(|l| l.starts_with(' ')).count();
        assert!(
            out_context < raw_context,
            "should reduce context: raw={raw_context} out={out_context}"
        );
    }
}

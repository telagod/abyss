use std::path::{Path, PathBuf};

use anyhow::Result;
use ignore::WalkBuilder;

pub struct FileWalker {
    root: PathBuf,
}

impl FileWalker {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn walk(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let walker = WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .ignore(true)
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                // Skip common non-code directories
                !matches!(
                    name.as_ref(),
                    "node_modules"
                        | ".git"
                        | ".code-abyss"
                        | ".ace-tool"
                        | "target"
                        | "dist"
                        | "build"
                        | "__pycache__"
                        | ".venv"
                        | "venv"
                        | ".tox"
                        | "vendor"
                )
            })
            .build();

        for entry in walker {
            let entry = entry?;
            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                let path = entry.into_path();
                if self.is_indexable(&path) {
                    files.push(path);
                }
            }
        }

        Ok(files)
    }

    fn is_indexable(&self, path: &Path) -> bool {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        matches!(
            ext,
            "rs" | "py"
                | "pyi"
                | "js"
                | "mjs"
                | "cjs"
                | "ts"
                | "mts"
                | "cts"
                | "tsx"
                | "jsx"
                | "go"
                | "java"
                | "c"
                | "h"
                | "cpp"
                | "cc"
                | "cxx"
                | "hpp"
                | "hxx"
                | "hh"
                | "json"
                | "toml"
                | "yml"
                | "yaml"
                | "sh"
                | "bash"
                | "html"
                | "htm"
                | "css"
                | "scss"
                | "md"
                | "markdown"
        )
    }
}

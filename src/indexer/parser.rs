use std::collections::HashMap;

use anyhow::Result;
use tree_sitter::{Language, Parser, Tree};

pub struct MultiParser {
    parsers: HashMap<String, Language>,
}

impl Default for MultiParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiParser {
    pub fn new() -> Self {
        let mut parsers = HashMap::new();
        parsers.insert("rust".into(), tree_sitter_rust::LANGUAGE.into());
        parsers.insert("python".into(), tree_sitter_python::LANGUAGE.into());
        parsers.insert("javascript".into(), tree_sitter_javascript::LANGUAGE.into());
        parsers.insert("typescript".into(), tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into());
        parsers.insert("tsx".into(), tree_sitter_typescript::LANGUAGE_TSX.into());
        parsers.insert("go".into(), tree_sitter_go::LANGUAGE.into());
        parsers.insert("java".into(), tree_sitter_java::LANGUAGE.into());
        parsers.insert("c".into(), tree_sitter_c::LANGUAGE.into());
        parsers.insert("cpp".into(), tree_sitter_cpp::LANGUAGE.into());
        parsers.insert("json".into(), tree_sitter_json::LANGUAGE.into());
        parsers.insert("toml".into(), tree_sitter_toml_ng::LANGUAGE.into());
        parsers.insert("yaml".into(), tree_sitter_yaml::LANGUAGE.into());
        parsers.insert("bash".into(), tree_sitter_bash::LANGUAGE.into());
        parsers.insert("html".into(), tree_sitter_html::LANGUAGE.into());
        parsers.insert("css".into(), tree_sitter_css::LANGUAGE.into());
        // markdown uses old tree-sitter API, skip for now

        Self { parsers }
    }

    pub fn parse(&self, source: &str, language: &str) -> Result<Tree> {
        let lang = self
            .parsers
            .get(language)
            .ok_or_else(|| anyhow::anyhow!("unsupported language: {language}"))?;
        let mut parser = Parser::new();
        parser.set_language(lang)?;
        parser
            .parse(source.as_bytes(), None)
            .ok_or_else(|| anyhow::anyhow!("failed to parse source"))
    }

    pub fn supports(&self, language: &str) -> bool {
        self.parsers.contains_key(language)
    }

    pub fn supported_languages(&self) -> Vec<&str> {
        self.parsers.keys().map(|s| s.as_str()).collect()
    }
}

pub fn detect_language(path: &str) -> Option<String> {
    let ext = path.rsplit('.').next()?;
    match ext {
        "rs" => Some("rust".into()),
        "py" | "pyi" => Some("python".into()),
        "js" | "mjs" | "cjs" => Some("javascript".into()),
        "ts" | "mts" | "cts" => Some("typescript".into()),
        "tsx" => Some("tsx".into()),
        "jsx" => Some("javascript".into()),
        "go" => Some("go".into()),
        "java" => Some("java".into()),
        "c" | "h" => Some("c".into()),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Some("cpp".into()),
        "json" => Some("json".into()),
        "toml" => Some("toml".into()),
        "yml" | "yaml" => Some("yaml".into()),
        "sh" | "bash" | "zsh" => Some("bash".into()),
        "html" | "htm" => Some("html".into()),
        "css" | "scss" => Some("css".into()),
        "md" | "markdown" => None, // markdown parser not wired yet
        _ => None,
    }
}

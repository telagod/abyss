use std::path::{Path, PathBuf};
use tree_sitter::Tree;

#[derive(Debug, Clone)]
pub struct RawReference {
    pub line: u32,
    pub source_symbol: Option<String>,
    pub target_name: String,
    pub target_qualifier: Option<String>,
    pub kind: RefKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RefKind {
    Call,
    TypeRef,
    Import,
    Inherit,
    FieldAccess,
}

impl RefKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RefKind::Call => "call",
            RefKind::TypeRef => "type_ref",
            RefKind::Import => "import",
            RefKind::Inherit => "inherit",
            RefKind::FieldAccess => "field_access",
        }
    }
}

pub trait LanguageRefExtractor: Send + Sync {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference>;
    fn is_test_file(&self, path: &str) -> bool;
    fn resolve_import(&self, import_path: &str, workspace: &Path) -> Option<PathBuf>;
    fn language_name(&self) -> &'static str;
}

use std::path::{Path, PathBuf};
use tree_sitter::Tree;

#[derive(Debug, Clone)]
pub struct RawReference {
    pub line: u32,
    pub source_symbol: Option<String>,
    pub target_name: String,
    pub target_qualifier: Option<String>,
    /// Inferred static type of the call receiver (`x.M()` where `x: T` → `T`).
    /// Lite inference: parameters, method receivers, local declarations with
    /// literal/constructor initializers. No data-flow, no interfaces.
    pub receiver_type: Option<String>,
    pub kind: RefKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RefKind {
    Call,
    TypeRef,
    Import,
    /// Named import/re-export binding: `import { a, b as c } from './x'`,
    /// `export { y } from './z'`. `target_name` = the LOCAL bound name,
    /// `target_qualifier` = the module path. Resolved to a file before the
    /// confidence tiers run; bare calls matching a binding resolve through it.
    ImportBinding,
    Inherit,
    FieldAccess,
}

impl RefKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RefKind::Call => "call",
            RefKind::TypeRef => "type_ref",
            RefKind::Import => "import",
            RefKind::ImportBinding => "import_binding",
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

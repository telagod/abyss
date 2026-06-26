use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

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

/// Lite per-scope variable-to-type map shared across language extractors.
/// Receivers, parameters, and locals with literal/constructor initializers.
/// No data-flow, no interfaces.
pub type VarTypes = HashMap<String, String>;

/// Build a per-line scope map: for each source line, the enclosing scope name
/// (function, method, class, impl, etc.) or `None` for top-level code.
///
/// All language extractors share this algorithm; they differ only in which AST
/// node kinds define scopes and how the scope name is extracted.
///
/// - `scope_node_kinds`: tree-sitter node kinds that open a new scope
///   (e.g. `&["function_declaration", "method_declaration"]`).
/// - `name_extractor`: given a scope node and source bytes, returns the scope
///   name. Most languages use `child_by_field_name("name")`, but some (e.g.
///   C/C++) need a custom lookup through declarator chains.
pub fn build_scope_map(
    root: &Node,
    source: &str,
    scope_node_kinds: &[&str],
    name_extractor: impl Fn(&Node, &str) -> Option<String> + Copy,
) -> Vec<Option<String>> {
    let line_count = source.lines().count();
    let mut map: Vec<Option<String>> = vec![None; line_count + 1];

    fn walk(
        node: &Node,
        source: &str,
        current: &Option<String>,
        map: &mut Vec<Option<String>>,
        scope_node_kinds: &[&str],
        name_extractor: impl Fn(&Node, &str) -> Option<String> + Copy,
    ) {
        let name = if scope_node_kinds.contains(&node.kind()) {
            name_extractor(node, source)
        } else {
            None
        };

        let active = name.as_ref().or(current.as_ref());
        if let Some(n) = active {
            let start = node.start_position().row;
            let end = node.end_position().row;
            for line in start..=end.min(map.len() - 1) {
                if map[line].is_none() {
                    map[line] = Some(n.clone());
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            walk(
                &child,
                source,
                &name.clone().or_else(|| current.clone()),
                map,
                scope_node_kinds,
                name_extractor,
            );
        }
    }

    walk(
        root,
        source,
        &None,
        &mut map,
        scope_node_kinds,
        name_extractor,
    );
    map
}

/// Default name extractor for most languages: reads `child_by_field_name("name")`
/// and returns the text of that node.
pub fn default_scope_name(node: &Node, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .map(|n| source[n.start_byte()..n.end_byte()].to_string())
}

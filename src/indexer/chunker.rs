use tree_sitter::{Node, Tree};

#[derive(Debug, Clone)]
pub struct CodeChunk {
    pub content: String,
    pub kind: ChunkKind,
    pub start_line: u32,
    pub end_line: u32,
    pub scope: Option<String>,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: u32,
    /// Owner type for methods (Go receiver / enclosing class). Falls back to
    /// the chunk scope at insert time when None.
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChunkKind {
    Function,
    Class,
    Module,
    Block,
    Import,
    Comment,
}

impl ChunkKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChunkKind::Function => "function",
            ChunkKind::Class => "class",
            ChunkKind::Module => "module",
            ChunkKind::Block => "block",
            ChunkKind::Import => "import",
            ChunkKind::Comment => "comment",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SymbolKind {
    Function,
    Class,
    Struct,
    Enum,
    Const,
    Variable,
    Import,
    Interface,
    Type,
    Method,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Const => "const",
            SymbolKind::Variable => "variable",
            SymbolKind::Import => "import",
            SymbolKind::Interface => "interface",
            SymbolKind::Type => "type",
            SymbolKind::Method => "method",
        }
    }
}

pub struct Chunker {
    max_lines: u32,
    #[allow(dead_code)] // reserved for min-chunk merging (see DESIGN-v0.2 chunking rules)
    min_lines: u32,
    target_lines: u32, // merge small chunks until reaching this
}

impl Chunker {
    pub fn new(max_lines: u32, min_lines: u32) -> Self {
        Self {
            max_lines,
            min_lines,
            target_lines: 40, // aim for ~40 line chunks when merging
        }
    }

    pub fn chunk(&self, source: &str, tree: &Tree, language: &str) -> Vec<CodeChunk> {
        let root = tree.root_node();
        let lines: Vec<&str> = source.lines().collect();
        let mut chunks = Vec::new();

        self.collect_chunks(
            &root,
            source,
            &lines,
            language,
            &mut Vec::new(),
            &mut chunks,
        );

        if chunks.is_empty() && !source.is_empty() {
            chunks.push(CodeChunk {
                content: source.to_string(),
                kind: ChunkKind::Module,
                start_line: 0,
                end_line: lines.len().saturating_sub(1) as u32,
                scope: None,
                symbols: Vec::new(),
            });
        }

        // Aggressive merging: merge adjacent small chunks
        self.merge_adjacent_chunks(&mut chunks);

        chunks
    }

    #[allow(clippy::only_used_in_recursion)]
    fn collect_chunks(
        &self,
        node: &Node,
        source: &str,
        lines: &[&str],
        language: &str,
        scope_stack: &mut Vec<String>,
        chunks: &mut Vec<CodeChunk>,
    ) {
        let kind = node.kind();

        if self.is_chunk_boundary(kind, language) {
            let start = node.start_position().row as u32;
            let end = node.end_position().row as u32;
            let content = self.extract_text(node, source);
            let chunk_kind = self.classify_node(kind, language);
            let symbols = self.extract_symbols(node, source, language);
            let scope = if scope_stack.is_empty() {
                None
            } else {
                Some(scope_stack.join("::"))
            };

            if end - start > self.max_lines {
                // Descending into an oversized node must not lose ITS symbol:
                // a >max_lines function used to vanish from the symbols table
                // entirely (only chunk boundaries among its children emit
                // anything, and a plain body block is not one).
                if let Some(name) = self.extract_node_name(node, source) {
                    let header = lines
                        .get(start as usize)
                        .copied()
                        .unwrap_or_default()
                        .to_string();
                    chunks.push(CodeChunk {
                        content: header,
                        kind: chunk_kind,
                        start_line: start,
                        end_line: start,
                        scope: scope.clone(),
                        symbols: vec![Symbol {
                            name: name.clone(),
                            kind: match chunk_kind {
                                ChunkKind::Class => SymbolKind::Class,
                                _ if scope.is_some() => SymbolKind::Method,
                                _ => SymbolKind::Function,
                            },
                            line: start,
                            scope: scope.clone(),
                        }],
                    });
                    scope_stack.push(name);
                }
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.collect_chunks(&child, source, lines, language, scope_stack, chunks);
                }
                if self.extract_node_name(node, source).is_some() {
                    scope_stack.pop();
                }
                return;
            }

            chunks.push(CodeChunk {
                content,
                kind: chunk_kind,
                start_line: start,
                end_line: end,
                scope,
                symbols,
            });
        } else {
            if let Some(name) = self.extract_node_name(node, source)
                && self.is_scope_node(kind, language)
            {
                scope_stack.push(name);
            }

            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                self.collect_chunks(&child, source, lines, language, scope_stack, chunks);
            }

            if self.is_scope_node(kind, language) && self.extract_node_name(node, source).is_some()
            {
                scope_stack.pop();
            }
        }
    }

    /// Merge adjacent small chunks until they reach target_lines.
    /// Imports always merge together. Small functions in the same scope merge.
    fn merge_adjacent_chunks(&self, chunks: &mut Vec<CodeChunk>) {
        if chunks.len() <= 1 {
            return;
        }

        let mut merged: Vec<CodeChunk> = Vec::new();

        let mut i = 0;
        while i < chunks.len() {
            let mut current = chunks[i].clone();
            let current_lines = current.end_line - current.start_line + 1;

            // If current chunk is already big enough, keep it
            if current_lines >= self.target_lines {
                merged.push(current);
                i += 1;
                continue;
            }

            // Try to merge with subsequent small chunks
            while i + 1 < chunks.len() {
                let next = &chunks[i + 1];
                let next_lines = next.end_line - next.start_line + 1;
                let merged_lines = (current.end_line.max(next.end_line)) - current.start_line + 1;

                // Stop merging if result would be too large
                if merged_lines > self.max_lines {
                    break;
                }

                // Merge imports together always
                let both_imports =
                    current.kind == ChunkKind::Import && next.kind == ChunkKind::Import;

                // Merge small chunks in same scope
                let same_scope = current.scope == next.scope;
                let both_small =
                    current_lines < self.target_lines && next_lines < self.target_lines;

                if both_imports || (same_scope && both_small) {
                    current.content.push('\n');
                    current.content.push_str(&next.content);
                    current.end_line = next.end_line;
                    current.symbols.extend(next.symbols.clone());
                    // Promote kind: function > block > import
                    if next.kind == ChunkKind::Function || next.kind == ChunkKind::Class {
                        current.kind = next.kind;
                    }
                    i += 1;
                } else {
                    break;
                }
            }

            merged.push(current);
            i += 1;
        }

        *chunks = merged;
    }

    fn is_chunk_boundary(&self, kind: &str, _language: &str) -> bool {
        matches!(
            kind,
            "function_definition"
                | "function_declaration"
                | "function_item"
                | "method_definition"
                | "method_declaration"
                | "public_field_definition"
                | "field_definition"
                | "class_definition"
                | "class_declaration"
                | "struct_item"
                | "enum_item"
                | "impl_item"
                | "trait_item"
                | "interface_declaration"
                | "type_alias_declaration"
                | "const_item"
                | "static_item"
                | "export_statement"
                | "import_declaration"
                | "import_statement"
                | "use_declaration"
                | "module_definition"
                | "mod_item"
                | "decorated_definition"
        )
    }

    fn is_scope_node(&self, kind: &str, _language: &str) -> bool {
        matches!(
            kind,
            "class_definition"
                | "class_declaration"
                | "struct_item"
                | "impl_item"
                | "trait_item"
                | "module_definition"
                | "mod_item"
                | "interface_declaration"
        )
    }

    fn classify_node(&self, kind: &str, _language: &str) -> ChunkKind {
        match kind {
            "function_definition"
            | "function_declaration"
            | "function_item"
            | "method_definition"
            | "method_declaration" => ChunkKind::Function,
            "class_definition"
            | "class_declaration"
            | "struct_item"
            | "enum_item"
            | "impl_item"
            | "trait_item"
            | "interface_declaration"
            | "type_alias_declaration" => ChunkKind::Class,
            "module_definition" | "mod_item" => ChunkKind::Module,
            "import_declaration" | "import_statement" | "use_declaration" | "export_statement" => {
                ChunkKind::Import
            }
            _ => ChunkKind::Block,
        }
    }

    fn extract_symbols(&self, node: &Node, source: &str, language: &str) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        self.collect_symbols_recursive(node, source, language, None, &mut symbols);
        symbols
    }

    fn collect_symbols_recursive(
        &self,
        node: &Node,
        source: &str,
        language: &str,
        owner: Option<&str>,
        symbols: &mut Vec<Symbol>,
    ) {
        let kind = node.kind();

        let symbol_kind = match kind {
            // A function nested in a class-like owner is a method: Python and
            // Rust have no dedicated method node kind, so without this the
            // owner scope is lost for small classes that fit in one chunk
            // (the chunk-scope backfill only fires when the class is split).
            "function_definition" | "function_declaration" | "function_item" => {
                Some(if owner.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                })
            }
            "method_definition" | "method_declaration" => Some(SymbolKind::Method),
            // TS/JS method-as-class-field: `text = (...) => {...}`. Data fields
            // are not symbols — only function-valued ones.
            "public_field_definition" | "field_definition"
                if node.child_by_field_name("value").is_some_and(|v| {
                    matches!(
                        v.kind(),
                        "arrow_function" | "function_expression" | "function"
                    )
                }) =>
            {
                Some(SymbolKind::Method)
            }
            // TS/JS module-level function consts: `export const useState = (...) => ...`
            "variable_declarator"
                if node.child_by_field_name("value").is_some_and(|v| {
                    matches!(
                        v.kind(),
                        "arrow_function" | "function_expression" | "function"
                    )
                }) =>
            {
                Some(SymbolKind::Function)
            }
            "class_definition" | "class_declaration" => Some(SymbolKind::Class),
            "struct_item" => Some(SymbolKind::Struct),
            "enum_item" => Some(SymbolKind::Enum),
            "const_item" | "static_item" => Some(SymbolKind::Const),
            "interface_declaration" => Some(SymbolKind::Interface),
            "type_alias_declaration" => Some(SymbolKind::Type),
            _ => None,
        };

        if let Some(sk) = symbol_kind
            && let Some(name) = self.extract_node_name(node, source)
        {
            // Method owner: Go encodes it in the receiver; class languages in
            // the enclosing class node we're recursing through.
            let scope = if sk == SymbolKind::Method {
                if language == "go" {
                    crate::graph::languages::go::go_method_receiver_type(node, source)
                } else {
                    owner.map(String::from)
                }
            } else {
                None
            };
            symbols.push(Symbol {
                name,
                kind: sk,
                line: node.start_position().row as u32,
                scope,
            });
        }

        // Entering a class-like node makes it the owner for nested methods
        let next_owner: Option<String> = if matches!(
            kind,
            "class_definition" | "class_declaration" | "interface_declaration" | "impl_item"
        ) {
            self.extract_node_name(node, source)
        } else {
            None
        };

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_symbols_recursive(
                &child,
                source,
                language,
                next_owner.as_deref().or(owner),
                symbols,
            );
        }
    }

    fn extract_node_name(&self, node: &Node, source: &str) -> Option<String> {
        node.child_by_field_name("name")
            .or_else(|| {
                let mut cursor = node.walk();
                node.children(&mut cursor)
                    .find(|c| c.kind() == "identifier" || c.kind() == "type_identifier")
            })
            .map(|n| self.extract_text(&n, source))
    }

    fn extract_text(&self, node: &Node, source: &str) -> String {
        let start = node.start_byte();
        let end = node.end_byte();
        source[start..end].to_string()
    }
}

use crate::graph::extractor::{RawReference, RefKind};
use crate::storage::Repository;

pub struct ResolvedRef {
    pub target_file_id: Option<i64>,
    pub target_symbol_id: Option<i64>,
    pub confidence: f64,
}

pub struct SymbolResolver<'a> {
    repo: &'a Repository,
}

impl<'a> SymbolResolver<'a> {
    pub fn new(repo: &'a Repository) -> Self {
        Self { repo }
    }

    pub fn resolve(&self, raw: &RawReference, source_file_id: i64) -> ResolvedRef {
        // Skip import refs — they don't resolve to symbols
        if raw.kind == RefKind::Import {
            return ResolvedRef {
                target_file_id: None,
                target_symbol_id: None,
                confidence: 0.0,
            };
        }

        // 1. Same-file resolution (confidence = 1.0)
        if let Ok(Some(sym)) = self
            .repo
            .find_symbol_by_name_in_file(source_file_id, &raw.target_name)
        {
            return ResolvedRef {
                target_file_id: Some(source_file_id),
                target_symbol_id: Some(sym.id),
                confidence: 1.0,
            };
        }

        // 2. Same-package resolution (confidence = 0.95)
        if let Some(resolved) = self.resolve_same_package(source_file_id, &raw.target_name) {
            return resolved;
        }

        // 3. Qualifier-based resolution (confidence = 0.9)
        if let Some(qualifier) = &raw.target_qualifier
            && let Some(resolved) =
                self.resolve_via_qualifier(source_file_id, qualifier, &raw.target_name)
        {
            return resolved;
        }

        // 4. Global unique resolution (confidence = 0.8)
        if let Ok(candidates) = self.repo.find_symbol_global(&raw.target_name) {
            // Filter to non-test definitions for type refs
            let filtered: Vec<_> = if raw.kind == RefKind::TypeRef {
                candidates
                    .iter()
                    .filter(|s| {
                        s.kind == "struct"
                            || s.kind == "interface"
                            || s.kind == "type"
                            || s.kind == "enum"
                    })
                    .collect()
            } else {
                candidates
                    .iter()
                    .filter(|s| s.kind == "function" || s.kind == "method")
                    .collect()
            };

            if filtered.len() == 1 {
                return ResolvedRef {
                    target_file_id: Some(filtered[0].file_id),
                    target_symbol_id: Some(filtered[0].id),
                    confidence: 0.8,
                };
            }

            // Ambiguous but exists
            if !filtered.is_empty() {
                return ResolvedRef {
                    target_file_id: Some(filtered[0].file_id),
                    target_symbol_id: Some(filtered[0].id),
                    confidence: 0.5,
                };
            }
        }

        // 5. Unresolved
        ResolvedRef {
            target_file_id: None,
            target_symbol_id: None,
            confidence: 0.0,
        }
    }

    fn resolve_same_package(&self, source_file_id: i64, name: &str) -> Option<ResolvedRef> {
        // Get source file's directory
        let source_path = self.repo.get_file_path(source_file_id).ok()??;
        let dir = parent_dir(&source_path)?;

        // Find all files in the same directory
        let sibling_ids = self.repo.files_in_directory(&dir).ok()?;

        for fid in sibling_ids {
            if fid == source_file_id {
                continue;
            }
            if let Ok(Some(sym)) = self.repo.find_symbol_by_name_in_file(fid, name) {
                return Some(ResolvedRef {
                    target_file_id: Some(fid),
                    target_symbol_id: Some(sym.id),
                    confidence: 0.95,
                });
            }
        }
        None
    }

    fn resolve_via_qualifier(
        &self,
        source_file_id: i64,
        qualifier: &str,
        name: &str,
    ) -> Option<ResolvedRef> {
        // Find import refs from this file that match the qualifier
        let refs = self.repo.find_refs_from_file(source_file_id).ok()?;
        let import_refs: Vec<_> = refs.iter().filter(|r| r.kind == "import").collect();

        // Try to match qualifier to an import's last path segment
        for import_ref in &import_refs {
            let import_path = &import_ref.target_name;
            let last_segment = import_path.rsplit('/').next().unwrap_or(import_path);
            if last_segment == qualifier {
                // Found the package — now find the symbol in files under that path
                let candidates = self.repo.find_symbol_global(name).ok()?;
                for c in &candidates {
                    let cpath = self.repo.get_file_path(c.file_id).ok()??;
                    if cpath.contains(import_path) {
                        return Some(ResolvedRef {
                            target_file_id: Some(c.file_id),
                            target_symbol_id: Some(c.id),
                            confidence: 0.9,
                        });
                    }
                }
            }
        }

        None
    }
}

fn parent_dir(path: &str) -> Option<String> {
    let p = std::path::Path::new(path);
    p.parent().map(|p| {
        let s = p.to_string_lossy().to_string();
        if s.is_empty() {
            ".".to_string()
        } else {
            format!("{s}/")
        }
    })
}

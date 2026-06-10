pub mod go;
pub mod typescript;
pub mod rust_lang;
pub mod python;

use super::LanguageRefExtractor;

pub fn get_extractor(language: &str) -> Option<Box<dyn LanguageRefExtractor>> {
    match language {
        "go" => Some(Box::new(go::GoExtractor)),
        "typescript" | "tsx" | "javascript" => Some(Box::new(typescript::TypeScriptExtractor)),
        "rust" => Some(Box::new(rust_lang::RustExtractor)),
        "python" => Some(Box::new(python::PythonExtractor)),
        _ => None,
    }
}

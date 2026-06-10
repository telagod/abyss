pub mod go;
pub mod java;
pub mod python;
pub mod rust_lang;
pub mod typescript;

use super::LanguageRefExtractor;

pub fn get_extractor(language: &str) -> Option<Box<dyn LanguageRefExtractor>> {
    match language {
        "go" => Some(Box::new(go::GoExtractor)),
        "java" => Some(Box::new(java::JavaExtractor)),
        "typescript" | "tsx" | "javascript" => Some(Box::new(typescript::TypeScriptExtractor)),
        "rust" => Some(Box::new(rust_lang::RustExtractor)),
        "python" => Some(Box::new(python::PythonExtractor)),
        _ => None,
    }
}

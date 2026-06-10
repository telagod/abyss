#[cfg(feature = "semantic")]
pub mod model;

#[cfg(feature = "semantic")]
pub use model::Embedder;

/// Stub for builds without the `semantic` feature.
///
/// Cannot be constructed (private field, no public constructor), so every
/// `Option<Embedder>` in a slim build is `None` and the methods below are
/// statically unreachable. This keeps all call-site signatures identical
/// across feature combinations without scattering `cfg` attributes.
#[cfg(not(feature = "semantic"))]
pub struct Embedder(#[allow(dead_code)] ());

#[cfg(not(feature = "semantic"))]
impl Embedder {
    pub fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        unreachable!("Embedder stub: built without the `semantic` feature")
    }

    pub fn embed_batch(&self, _texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        unreachable!("Embedder stub: built without the `semantic` feature")
    }

    pub fn dimensions(&self) -> usize {
        unreachable!("Embedder stub: built without the `semantic` feature")
    }
}

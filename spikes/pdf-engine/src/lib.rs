//! PDFX M0→M1 technical spike library.
//!
//! Probes the production-critical paths required by ticket 03:
//! rendering and text extraction, native annotations, sequential incremental
//! saves (PDFium then lopdf), and CJK Font Subset embedding.

pub mod annotations;
pub mod cjk_font;
pub mod engine;
pub mod raw;
pub mod save_pipeline;

/// Convenience re-exports for probe code and tests.
pub mod prelude {
    pub use crate::annotations::*;
    pub use crate::engine::*;
    pub use pdfium_render::prelude::*;
}

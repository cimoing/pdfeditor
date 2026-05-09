//! Core layer for the PDF editor MVP.
//!
//! This crate intentionally has no UI dependency and no browser runtime dependency.
//! Real PDF backends such as PDFium or MuPDF can be plugged in behind [`PdfEngine`].

mod cache;
mod document;
mod edit;
mod engine;
mod error;
mod lopdf_backend;
mod resource;
mod types;

pub use cache::{CacheStats, PageBitmapCache};
pub use document::{DocumentSession, OpenOptions, SaveOptions};
pub use edit::{EditCommand, EditQueue, ObjectSnapshot, TextStyle};
pub use engine::{EngineDocument, MockPdfEngine, PdfEngine};
pub use error::{CoreError, CoreResult};
pub use lopdf_backend::LopdfEngine;
pub use resource::ResourceBudget;
pub use types::{
    Color, ImageObject, ImageObjectId, PageIndex, PageInfo, PdfObjectId, Point, Rect, RenderedPage,
    Size, TextObject, TextObjectId, TextRun,
};

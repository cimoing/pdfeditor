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
mod page_writer;
mod resource;
mod types;
#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
mod wasm_api;

pub use cache::{CacheStats, PageBitmapCache};
pub use document::{DocumentSession, OpenOptions, SaveOptions};
pub use edit::{EditCommand, EditQueue, ObjectSnapshot, TextStyle};
pub use engine::{EngineDocument, MockPdfEngine, PdfEngine};
pub use error::{CoreError, CoreResult};
pub use lopdf_backend::{
    open_lopdf_document_from_bytes, page_background_png_from_pdf_bytes,
    page_font_assets_from_pdf_bytes, page_image_png_from_pdf_bytes, page_structure_from_pdf_bytes,
    save_pdf_document_to_bytes, write_pdf_background_png, write_pdf_page_images,
    BackgroundBitmapReport, BackgroundRenderOptions, LopdfEngine, PageFontAsset,
    PageImageBytesExport, PageImageExport,
};
pub use page_writer::write_page_structure_pdf;
pub use resource::ResourceBudget;
pub use types::{
    BookmarkItem, Color, ImageObject, ImageObjectId, PageIndex, PageInfo, PageStructure,
    PdfObjectId, Point, Rect, RenderedPage, Size, StructuredAnnotation, StructuredImageObject,
    StructuredTextObject, StructuredWatermark, TextObject, TextObjectId, TextRun,
};

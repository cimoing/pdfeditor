use crate::lopdf_backend::{open_lopdf_document_from_bytes_unindexed, LopdfDocument};
use crate::types::{Color, Rect, TextRun};
use crate::{
    save_pdf_document_to_bytes, BackgroundRenderOptions, CoreError, EngineDocument, PageIndex,
    PdfObjectId, Point, TextObjectId, TextTypography,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

thread_local! {
    static DOCUMENT_STORE: RefCell<DocumentStore> = RefCell::new(DocumentStore::default());
}

#[derive(Debug, Default)]
struct DocumentStore {
    next_handle: u32,
    documents: HashMap<u32, LopdfDocument>,
}

impl DocumentStore {
    fn insert(&mut self, document: LopdfDocument) -> u32 {
        self.next_handle = self.next_handle.wrapping_add(1).max(1);
        while self.documents.contains_key(&self.next_handle) {
            self.next_handle = self.next_handle.wrapping_add(1).max(1);
        }
        let handle = self.next_handle;
        self.documents.insert(handle, document);
        handle
    }
}

#[wasm_bindgen]
pub fn pdf_open_document(pdf_bytes: &[u8]) -> Result<u32, JsValue> {
    let document = open_lopdf_document_from_bytes_unindexed(pdf_bytes).map_err(core_error_to_js)?;
    Ok(DOCUMENT_STORE.with(|store| store.borrow_mut().insert(document)))
}

#[wasm_bindgen]
pub fn pdf_close_document(handle: u32) -> Result<(), JsValue> {
    DOCUMENT_STORE.with(|store| {
        store
            .borrow_mut()
            .documents
            .remove(&handle)
            .map(|_| ())
            .ok_or_else(|| invalid_handle_error(handle))
    })
}

#[wasm_bindgen]
pub fn pdf_page_bundle(handle: u32, page_number: u32) -> Result<Vec<u8>, JsValue> {
    let page = wasm_page_index(page_number)?;
    with_document_mut(handle, |document| {
        document
            .ensure_text_index_for_page(page)
            .map_err(core_error_to_js)?;
        let bundle = document
            .page_load_bundle(page, BackgroundRenderOptions::default())
            .map_err(core_error_to_js)?;
        encode_page_bundle(bundle)
    })
}

/// Return the page structure (text objects, images, fonts) as JSON without
/// rendering the background PNG. Much faster than `pdf_page_bundle` for
/// post-edit refreshes where the background image hasn't changed.
#[wasm_bindgen]
pub fn pdf_page_structure(handle: u32, page_number: u32) -> Result<String, JsValue> {
    let page = wasm_page_index(page_number)?;
    with_document_mut(handle, |document| {
        document
            .ensure_text_index_for_page(page)
            .map_err(core_error_to_js)?;
        let structure = document.page_structure(page).map_err(core_error_to_js)?;
        serde_json::to_string(&structure).map_err(|error| {
            JsValue::from_str(&format!("failed to serialize page structure JSON: {error}"))
        })
    })
}

#[wasm_bindgen]
pub fn pdf_hit_test(
    handle: u32,
    page_number: u32,
    pdf_x: f64,
    pdf_y: f64,
) -> Result<String, JsValue> {
    let page = wasm_page_index(page_number)?;
    with_document_mut(handle, |document| {
        document
            .ensure_text_index_for_page(page)
            .map_err(core_error_to_js)?;
        let result = document
            .hit_test(page, Point::new(pdf_x as f32, pdf_y as f32))
            .map_err(core_error_to_js)?;
        serde_json::to_string(&result).map_err(|error| {
            JsValue::from_str(&format!("failed to serialize hit-test JSON: {error}"))
        })
    })
}

#[wasm_bindgen]
pub fn pdf_start_text_edit(handle: u32, object_id: u64) -> Result<String, JsValue> {
    with_document_mut(handle, |document| {
        document
            .ensure_text_index_for_page(page_from_text_object_id(object_id))
            .map_err(core_error_to_js)?;
        let result = document
            .start_text_edit(TextObjectId(PdfObjectId(object_id)))
            .map_err(core_error_to_js)?;
        serde_json::to_string(&result).map_err(|error| {
            JsValue::from_str(&format!(
                "failed to serialize text edit session JSON: {error}"
            ))
        })
    })
}

#[wasm_bindgen]
pub fn pdf_preview_text_layout(
    handle: u32,
    object_id: u64,
    text: &str,
) -> Result<String, JsValue> {
    with_document_mut(handle, |document| {
        document
            .ensure_text_index_for_page(page_from_text_object_id(object_id))
            .map_err(core_error_to_js)?;
        let result = document
            .preview_text_layout(TextObjectId(PdfObjectId(object_id)), text.to_string())
            .map_err(core_error_to_js)?;
        serde_json::to_string(&result)
            .map_err(|error| JsValue::from_str(&format!("failed to serialize preview JSON: {error}")))
    })
}

/// Apply text edits to the in-memory document. Use `pdf_get_bytes` when PDF
/// bytes are needed for download or export.
#[wasm_bindgen]
pub fn pdf_apply_text_edits(handle: u32, edits_json: &str) -> Result<(), JsValue> {
    let request: TextEditRequest = serde_json::from_str(edits_json)
        .map_err(|error| JsValue::from_str(&format!("invalid edits JSON: {error}")))?;
    with_document_mut(handle, |document| {
        for edit in request.edits {
            apply_text_edit(document, edit).map_err(core_error_to_js)?;
        }
        Ok(())
    })
}

/// Serialize the in-memory document to PDF bytes.
#[wasm_bindgen]
pub fn pdf_get_bytes(handle: u32) -> Result<Vec<u8>, JsValue> {
    with_document(handle, |document| {
        save_pdf_document_to_bytes(document).map_err(core_error_to_js)
    })
}

/// Load an embedded CJK fallback font from WOFF1 bytes.
///
/// Once set, subsequent edits that need a CJK fallback will embed this TrueType font
/// (Identity-H encoding) instead of the unembedded STSong-Light standard font.
#[wasm_bindgen]
pub fn pdf_set_cjk_font(handle: u32, woff_bytes: &[u8]) -> Result<bool, JsValue> {
    with_document_mut(handle, |document| {
        if let Some(sfnt) = crate::font_embed::woff1_to_sfnt(woff_bytes) {
            if let Some(data) = crate::font_embed::parse_cjk_font(sfnt) {
                document.set_cjk_font(data);
                return Ok(true);
            }
        }
        Ok(false)
    })
}

/// Register a local TrueType/SFNT font for later embedding.
///
/// `font_key` is the frontend resource key after the `__localfont__:` prefix.
/// Only TrueType-flavoured SFNT/WOFF1/TTC fonts are accepted for now.
#[wasm_bindgen]
pub fn pdf_set_local_font(
    handle: u32,
    font_key: &str,
    font_bytes: &[u8],
) -> Result<bool, JsValue> {
    let Some(sfnt) = crate::font_embed::font_bytes_to_truetype_sfnt(font_bytes) else {
        return Ok(false);
    };
    let Some(data) = crate::font_embed::parse_cjk_font(sfnt) else {
        return Ok(false);
    };

    with_document_mut(handle, |document| {
        document.set_local_font(font_key.to_string(), data);
        Ok(true)
    })
}

fn with_document<T>(
    handle: u32,
    f: impl FnOnce(&LopdfDocument) -> Result<T, JsValue>,
) -> Result<T, JsValue> {
    DOCUMENT_STORE.with(|store| {
        let store = store.borrow();
        let document = store
            .documents
            .get(&handle)
            .ok_or_else(|| invalid_handle_error(handle))?;
        f(document)
    })
}

fn with_document_mut<T>(
    handle: u32,
    f: impl FnOnce(&mut LopdfDocument) -> Result<T, JsValue>,
) -> Result<T, JsValue> {
    DOCUMENT_STORE.with(|store| {
        let mut store = store.borrow_mut();
        let document = store
            .documents
            .get_mut(&handle)
            .ok_or_else(|| invalid_handle_error(handle))?;
        f(document)
    })
}

fn encode_page_bundle(bundle: crate::PageLoadBundle) -> Result<Vec<u8>, JsValue> {
    let mut payload = Vec::new();
    let background_png = BinaryAssetInfo::append(
        "background.png",
        "image/png",
        &bundle.background_png,
        &mut payload,
    );
    let images = bundle
        .images
        .into_iter()
        .map(|image| ImageAssetInfo {
            id: (image.id.0).0,
            file_name: image.file_name.clone(),
            width_px: image.width_px,
            height_px: image.height_px,
            asset: BinaryAssetInfo::append(&image.file_name, "image/png", &image.png, &mut payload),
        })
        .collect::<Vec<_>>();
    let fonts = bundle
        .fonts
        .into_iter()
        .map(|font| FontAssetBundleInfo {
            resource_name: font.resource_name,
            family_name: font.family_name,
            font_weight: font.font_weight,
            is_bold: font.is_bold,
            file_name: font.file_name.clone(),
            mime_type: font.mime_type.clone(),
            format: font.format,
            asset: BinaryAssetInfo::append(
                &font.file_name,
                &font.mime_type,
                &font.bytes,
                &mut payload,
            ),
        })
        .collect::<Vec<_>>();
    let metadata = PageBundleInfo {
        structure: bundle.structure,
        background_png,
        images,
        fonts,
    };
    let json = serde_json::to_vec(&metadata)
        .map_err(|error| JsValue::from_str(&format!("failed to serialize page bundle: {error}")))?;
    if json.len() > u32::MAX as usize {
        return Err(JsValue::from_str("page bundle metadata is too large"));
    }

    let mut output = Vec::with_capacity(4 + json.len() + payload.len());
    output.extend_from_slice(&(json.len() as u32).to_be_bytes());
    output.extend_from_slice(&json);
    output.extend_from_slice(&payload);
    Ok(output)
}

fn page_from_text_object_id(object_id: u64) -> PageIndex {
    PageIndex((object_id >> 32) as u32)
}

fn page_from_text_edit(edit: &TextEdit) -> crate::CoreResult<PageIndex> {
    if let Some(page_index) = edit.page_index {
        return Ok(PageIndex(page_index));
    }
    if let Some(page_number) = edit.page_number {
        if page_number == 0 {
            return Err(crate::CoreError::InvalidOperation(
                "page_number is 1-based and must be greater than 0".to_string(),
            ));
        }
        return Ok(PageIndex(page_number - 1));
    }
    Ok(page_from_text_object_id(edit.id))
}

fn apply_text_edit(document: &mut LopdfDocument, edit: TextEdit) -> crate::CoreResult<()> {
    let page = page_from_text_edit(&edit)?;
    let id = TextObjectId(PdfObjectId(edit.id));
    document.ensure_text_index_for_page(page)?;
    match edit.kind.as_str() {
        "replace_text" | "update_text" => {
            document.update_text_object(id, edit.content, None)?;
        }
        "replace_runs" | "replace_text_runs" => {
            let runs = text_runs_from_input(edit.runs);
            document.replace_text_object_with_runs(
                id,
                runs,
                Point::new(edit.origin_dx, edit.origin_dy),
                edit.clip_bounds,
                edit.typography.unwrap_or_default(),
            )?;
        }
        kind => {
            return Err(crate::CoreError::InvalidOperation(format!(
                "unsupported edit operation: {kind}"
            )));
        }
    }
    Ok(())
}

fn text_runs_from_input(inputs: Vec<TextRunInput>) -> Vec<TextRun> {
    inputs
        .into_iter()
        .map(|r| {
            TextRun::new(
                r.content,
                r.font_name,
                r.font_size,
                Color {
                    r: r.color[0],
                    g: r.color[1],
                    b: r.color[2],
                    a: r.color[3],
                },
            )
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct TextEditRequest {
    edits: Vec<TextEdit>,
}

#[derive(Debug, Deserialize)]
struct TextEdit {
    #[serde(rename = "type")]
    kind: String,
    id: u64,
    #[serde(default)]
    page_number: Option<u32>,
    #[serde(default)]
    page_index: Option<u32>,
    #[serde(default)]
    content: String,
    #[serde(default)]
    runs: Vec<TextRunInput>,
    #[serde(default)]
    origin_dx: f32,
    #[serde(default)]
    origin_dy: f32,
    #[serde(default)]
    clip_bounds: Option<Rect>,
    #[serde(default)]
    typography: Option<TextTypography>,
}

#[derive(Debug, Deserialize)]
struct TextRunInput {
    content: String,
    font_name: Option<String>,
    font_size: f32,
    color: [u8; 4],
}

#[derive(Debug, Serialize)]
struct PageBundleInfo {
    structure: crate::PageStructure,
    background_png: BinaryAssetInfo,
    images: Vec<ImageAssetInfo>,
    fonts: Vec<FontAssetBundleInfo>,
}

#[derive(Debug, Serialize)]
struct ImageAssetInfo {
    id: u64,
    file_name: String,
    width_px: u32,
    height_px: u32,
    asset: BinaryAssetInfo,
}

#[derive(Debug, Serialize)]
struct FontAssetBundleInfo {
    resource_name: String,
    family_name: String,
    font_weight: u16,
    is_bold: bool,
    file_name: String,
    mime_type: String,
    format: String,
    asset: BinaryAssetInfo,
}

#[derive(Debug, Serialize)]
struct BinaryAssetInfo {
    file_name: String,
    mime_type: String,
    offset: usize,
    length: usize,
}

impl BinaryAssetInfo {
    fn append(file_name: &str, mime_type: &str, bytes: &[u8], payload: &mut Vec<u8>) -> Self {
        let offset = payload.len();
        payload.extend_from_slice(bytes);
        Self {
            file_name: file_name.to_string(),
            mime_type: mime_type.to_string(),
            offset,
            length: bytes.len(),
        }
    }
}

fn wasm_page_index(page_number: u32) -> Result<PageIndex, JsValue> {
    if page_number == 0 {
        return Err(JsValue::from_str(
            "page_number is 1-based and must be greater than 0",
        ));
    }
    Ok(PageIndex(page_number - 1))
}

fn invalid_handle_error(handle: u32) -> JsValue {
    JsValue::from_str(&format!("invalid PDF document handle: {handle}"))
}

fn core_error_to_js(error: CoreError) -> JsValue {
    JsValue::from_str(&error.to_string())
}

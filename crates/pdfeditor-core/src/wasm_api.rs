use crate::lopdf_backend::LopdfDocument;
use crate::{
    open_lopdf_document_from_bytes, page_background_png_from_pdf_bytes,
    page_font_assets_from_pdf_bytes, page_image_png_from_pdf_bytes,
    page_load_bundle_from_pdf_bytes, page_structure_from_pdf_bytes, save_pdf_document_to_bytes,
    BackgroundRenderOptions, CoreError, EngineDocument, ImageObjectId, PageIndex, PdfObjectId,
    Point, TextObjectId,
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
pub fn pdf_page_to_json(pdf_bytes: &[u8], page_number: u32) -> Result<String, JsValue> {
    let page = wasm_page_index(page_number)?;
    let structure = page_structure_from_pdf_bytes(pdf_bytes, page).map_err(core_error_to_js)?;
    serde_json::to_string_pretty(&structure)
        .map_err(|error| JsValue::from_str(&format!("failed to serialize page JSON: {error}")))
}

#[wasm_bindgen]
pub fn pdf_page_bundle(pdf_bytes: &[u8], page_number: u32) -> Result<Vec<u8>, JsValue> {
    let page = wasm_page_index(page_number)?;
    let bundle =
        page_load_bundle_from_pdf_bytes(pdf_bytes, page, BackgroundRenderOptions::default())
            .map_err(core_error_to_js)?;
    encode_page_bundle(bundle)
}

#[wasm_bindgen]
pub fn pdf_open_document(pdf_bytes: &[u8]) -> Result<u32, JsValue> {
    let document = open_lopdf_document_from_bytes(pdf_bytes).map_err(core_error_to_js)?;
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
            .ok_or_else(|| JsValue::from_str(&format!("invalid PDF document handle: {handle}")))
    })
}

#[wasm_bindgen]
pub fn pdf_page_bundle_by_handle(handle: u32, page_number: u32) -> Result<Vec<u8>, JsValue> {
    let page = wasm_page_index(page_number)?;
    DOCUMENT_STORE.with(|store| {
        let store = store.borrow();
        let document = store
            .documents
            .get(&handle)
            .ok_or_else(|| JsValue::from_str(&format!("invalid PDF document handle: {handle}")))?;
        let bundle = document
            .page_load_bundle(page, BackgroundRenderOptions::default())
            .map_err(core_error_to_js)?;
        encode_page_bundle(bundle)
    })
}

#[wasm_bindgen]
pub fn pdf_hit_test(
    pdf_bytes: &[u8],
    page_number: u32,
    pdf_x: f64,
    pdf_y: f64,
) -> Result<String, JsValue> {
    let page = wasm_page_index(page_number)?;
    let document = open_lopdf_document_from_bytes(pdf_bytes).map_err(core_error_to_js)?;
    let result = document
        .hit_test(page, Point::new(pdf_x as f32, pdf_y as f32))
        .map_err(core_error_to_js)?;
    serde_json::to_string(&result)
        .map_err(|error| JsValue::from_str(&format!("failed to serialize hit-test JSON: {error}")))
}

#[wasm_bindgen]
pub fn pdf_hit_test_by_handle(
    handle: u32,
    page_number: u32,
    pdf_x: f64,
    pdf_y: f64,
) -> Result<String, JsValue> {
    let page = wasm_page_index(page_number)?;
    DOCUMENT_STORE.with(|store| {
        let store = store.borrow();
        let document = store
            .documents
            .get(&handle)
            .ok_or_else(|| JsValue::from_str(&format!("invalid PDF document handle: {handle}")))?;
        let result = document
            .hit_test(page, Point::new(pdf_x as f32, pdf_y as f32))
            .map_err(core_error_to_js)?;
        serde_json::to_string(&result).map_err(|error| {
            JsValue::from_str(&format!("failed to serialize hit-test JSON: {error}"))
        })
    })
}

#[wasm_bindgen]
pub fn pdf_start_text_edit(pdf_bytes: &[u8], object_id: u64) -> Result<String, JsValue> {
    let document = open_lopdf_document_from_bytes(pdf_bytes).map_err(core_error_to_js)?;
    let result = document
        .start_text_edit(TextObjectId(PdfObjectId(object_id)))
        .map_err(core_error_to_js)?;
    serde_json::to_string(&result).map_err(|error| {
        JsValue::from_str(&format!(
            "failed to serialize text edit session JSON: {error}"
        ))
    })
}

#[wasm_bindgen]
pub fn pdf_start_text_edit_by_handle(handle: u32, object_id: u64) -> Result<String, JsValue> {
    DOCUMENT_STORE.with(|store| {
        let store = store.borrow();
        let document = store
            .documents
            .get(&handle)
            .ok_or_else(|| JsValue::from_str(&format!("invalid PDF document handle: {handle}")))?;
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
    pdf_bytes: &[u8],
    object_id: u64,
    text: &str,
) -> Result<String, JsValue> {
    let document = open_lopdf_document_from_bytes(pdf_bytes).map_err(core_error_to_js)?;
    let result = document
        .preview_text_layout(TextObjectId(PdfObjectId(object_id)), text.to_string())
        .map_err(core_error_to_js)?;
    serde_json::to_string(&result)
        .map_err(|error| JsValue::from_str(&format!("failed to serialize preview JSON: {error}")))
}

#[wasm_bindgen]
pub fn pdf_preview_text_layout_by_handle(
    handle: u32,
    object_id: u64,
    text: &str,
) -> Result<String, JsValue> {
    DOCUMENT_STORE.with(|store| {
        let store = store.borrow();
        let document = store
            .documents
            .get(&handle)
            .ok_or_else(|| JsValue::from_str(&format!("invalid PDF document handle: {handle}")))?;
        let result = document
            .preview_text_layout(TextObjectId(PdfObjectId(object_id)), text.to_string())
            .map_err(core_error_to_js)?;
        serde_json::to_string(&result).map_err(|error| {
            JsValue::from_str(&format!("failed to serialize preview JSON: {error}"))
        })
    })
}

#[wasm_bindgen]
pub fn pdf_commit_text_edit(
    pdf_bytes: &[u8],
    object_id: u64,
    text: &str,
) -> Result<Vec<u8>, JsValue> {
    let mut document = open_lopdf_document_from_bytes(pdf_bytes).map_err(core_error_to_js)?;
    document
        .update_text_object(TextObjectId(PdfObjectId(object_id)), text.to_string(), None)
        .map_err(core_error_to_js)?;
    save_pdf_document_to_bytes(&document).map_err(core_error_to_js)
}

#[wasm_bindgen]
pub fn pdf_commit_text_edit_by_handle(
    handle: u32,
    object_id: u64,
    text: &str,
) -> Result<Vec<u8>, JsValue> {
    DOCUMENT_STORE.with(|store| {
        let mut store = store.borrow_mut();
        let document = store
            .documents
            .get_mut(&handle)
            .ok_or_else(|| JsValue::from_str(&format!("invalid PDF document handle: {handle}")))?;
        document
            .update_text_object(TextObjectId(PdfObjectId(object_id)), text.to_string(), None)
            .map_err(core_error_to_js)?;
        save_pdf_document_to_bytes(document).map_err(core_error_to_js)
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

#[wasm_bindgen]
pub fn pdf_page_background_png(pdf_bytes: &[u8], page_number: u32) -> Result<Vec<u8>, JsValue> {
    let page = wasm_page_index(page_number)?;
    page_background_png_from_pdf_bytes(pdf_bytes, page, BackgroundRenderOptions::default())
        .map_err(core_error_to_js)
}

#[wasm_bindgen]
pub fn pdf_image_object_png(
    pdf_bytes: &[u8],
    page_number: u32,
    image_object_id: u64,
) -> Result<Vec<u8>, JsValue> {
    let page = wasm_page_index(page_number)?;
    page_image_png_from_pdf_bytes(pdf_bytes, page, ImageObjectId(PdfObjectId(image_object_id)))
        .map_err(core_error_to_js)
}

#[wasm_bindgen]
pub fn pdf_page_fonts_to_json(pdf_bytes: &[u8], page_number: u32) -> Result<String, JsValue> {
    let page = wasm_page_index(page_number)?;
    let fonts = page_font_assets_from_pdf_bytes(pdf_bytes, page).map_err(core_error_to_js)?;
    let metadata = fonts
        .into_iter()
        .map(|font| FontAssetInfo {
            resource_name: font.resource_name,
            family_name: font.family_name,
            file_name: font.file_name,
            mime_type: font.mime_type,
            format: font.format,
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&metadata)
        .map_err(|error| JsValue::from_str(&format!("failed to serialize font JSON: {error}")))
}

#[wasm_bindgen]
pub fn pdf_font_asset(
    pdf_bytes: &[u8],
    page_number: u32,
    resource_name: &str,
) -> Result<Vec<u8>, JsValue> {
    let page = wasm_page_index(page_number)?;
    let fonts = page_font_assets_from_pdf_bytes(pdf_bytes, page).map_err(core_error_to_js)?;
    fonts
        .into_iter()
        .find(|font| font.resource_name == resource_name)
        .map(|font| font.bytes)
        .ok_or_else(|| JsValue::from_str(&format!("font resource not found: {resource_name}")))
}

#[wasm_bindgen]
pub fn pdf_apply_text_edits(pdf_bytes: &[u8], edits_json: &str) -> Result<Vec<u8>, JsValue> {
    let request: TextEditRequest = serde_json::from_str(edits_json)
        .map_err(|error| JsValue::from_str(&format!("invalid edits JSON: {error}")))?;
    let mut document = open_lopdf_document_from_bytes(pdf_bytes).map_err(core_error_to_js)?;

    for edit in request.edits {
        match edit.kind.as_str() {
            "replace_text" | "update_text" => {
                document
                    .update_text_object(TextObjectId(PdfObjectId(edit.id)), edit.content, None)
                    .map_err(core_error_to_js)?;
            }
            kind => {
                return Err(JsValue::from_str(&format!(
                    "unsupported edit operation: {kind}"
                )));
            }
        }
    }

    save_pdf_document_to_bytes(&document).map_err(core_error_to_js)
}

#[wasm_bindgen]
pub fn pdf_apply_text_edits_by_handle(handle: u32, edits_json: &str) -> Result<Vec<u8>, JsValue> {
    let request: TextEditRequest = serde_json::from_str(edits_json)
        .map_err(|error| JsValue::from_str(&format!("invalid edits JSON: {error}")))?;
    DOCUMENT_STORE.with(|store| {
        let mut store = store.borrow_mut();
        let document = store
            .documents
            .get_mut(&handle)
            .ok_or_else(|| JsValue::from_str(&format!("invalid PDF document handle: {handle}")))?;

        for edit in request.edits {
            match edit.kind.as_str() {
                "replace_text" | "update_text" => {
                    document
                        .update_text_object(TextObjectId(PdfObjectId(edit.id)), edit.content, None)
                        .map_err(core_error_to_js)?;
                }
                kind => {
                    return Err(JsValue::from_str(&format!(
                        "unsupported edit operation: {kind}"
                    )));
                }
            }
        }

        save_pdf_document_to_bytes(document).map_err(core_error_to_js)
    })
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
    content: String,
}

#[derive(Debug, Serialize)]
struct FontAssetInfo {
    resource_name: String,
    family_name: String,
    file_name: String,
    mime_type: String,
    format: String,
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

fn core_error_to_js(error: CoreError) -> JsValue {
    JsValue::from_str(&error.to_string())
}

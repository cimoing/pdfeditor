use crate::{
    open_lopdf_document_from_bytes, page_background_png_from_pdf_bytes,
    page_font_assets_from_pdf_bytes, page_image_png_from_pdf_bytes, page_structure_from_pdf_bytes,
    save_pdf_document_to_bytes, BackgroundRenderOptions, CoreError, EngineDocument, ImageObjectId,
    PageIndex, PdfObjectId, TextObjectId,
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn pdf_page_to_json(pdf_bytes: &[u8], page_number: u32) -> Result<String, JsValue> {
    let page = wasm_page_index(page_number)?;
    let structure = page_structure_from_pdf_bytes(pdf_bytes, page).map_err(core_error_to_js)?;
    serde_json::to_string_pretty(&structure)
        .map_err(|error| JsValue::from_str(&format!("failed to serialize page JSON: {error}")))
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

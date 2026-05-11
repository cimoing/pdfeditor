use crate::{
    BookmarkItem, Color, CoreError, CoreResult, EngineDocument, ImageObject, ImageObjectId,
    PageIndex, PageInfo, PageStructure, PdfEngine, PdfObjectId, Rect, RenderedPage, Size,
    StructuredAnnotation, StructuredImageObject, StructuredTextObject, StructuredWatermark,
    TextObject, TextObjectId, TextRun, TextStyle,
};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use tiny_skia::{Color as SkiaColor, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

#[derive(Debug, Clone, Default)]
pub struct LopdfEngine;

#[derive(Debug, Clone, Copy)]
pub struct BackgroundRenderOptions {
    pub scale: f32,
    pub max_pixels: u64,
}

impl Default for BackgroundRenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            max_pixels: 16_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundBitmapReport {
    pub width_px: u32,
    pub height_px: u32,
    pub drawn_operations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImageExport {
    pub id: ImageObjectId,
    pub file_name: String,
    pub width_px: u32,
    pub height_px: u32,
}

pub fn write_pdf_background_png(
    input: impl AsRef<Path>,
    page: PageIndex,
    output: impl AsRef<Path>,
    options: BackgroundRenderOptions,
) -> CoreResult<BackgroundBitmapReport> {
    let engine = LopdfEngine;
    let document = engine.open(input.as_ref())?;
    document.write_background_png(page, output.as_ref(), options)
}

pub fn write_pdf_page_images(
    input: impl AsRef<Path>,
    page: PageIndex,
    output_dir: impl AsRef<Path>,
) -> CoreResult<Vec<PageImageExport>> {
    let engine = LopdfEngine;
    let document = engine.open(input.as_ref())?;
    document.write_page_images(page, output_dir.as_ref())
}

#[derive(Debug, Clone)]
pub struct LopdfDocument {
    document: Document,
    pages: Vec<ObjectId>,
    text_objects: HashMap<PageIndex, Vec<TextObject>>,
    text_refs: HashMap<TextObjectId, TextObjectRef>,
}

#[derive(Debug, Clone)]
struct TextObjectRef {
    page: PageIndex,
    operation_index: usize,
    font_name: Option<String>,
}

impl PdfEngine for LopdfEngine {
    type Document = LopdfDocument;

    fn open(&self, path: &Path) -> CoreResult<Self::Document> {
        let document = Document::load(path)
            .map_err(|err| CoreError::InvalidPdf(format!("failed to load PDF: {err}")))?;
        let page_labels = document.get_pages();
        let pages = page_labels.values().copied().collect::<Vec<_>>();
        let mut result = LopdfDocument {
            document,
            pages,
            text_objects: HashMap::new(),
            text_refs: HashMap::new(),
        };
        result.extract_text_objects()?;
        Ok(result)
    }
}

impl EngineDocument for LopdfDocument {
    fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    fn page_info(&self, page: PageIndex) -> CoreResult<PageInfo> {
        let page_id = self.page_id(page)?;
        let size = self.page_size(page_id).unwrap_or(Size::new(595.0, 842.0));
        Ok(PageInfo { index: page, size })
    }

    fn text_objects(&self, page: PageIndex) -> CoreResult<Vec<TextObject>> {
        self.ensure_page(page)?;
        Ok(self.text_objects.get(&page).cloned().unwrap_or_default())
    }

    fn image_objects(&self, page: PageIndex) -> CoreResult<Vec<ImageObject>> {
        self.ensure_page(page)?;
        Ok(self
            .structured_images(page)?
            .into_iter()
            .map(|image| ImageObject {
                id: image.id,
                page,
                bounds: image.bounds,
                format: image.filters.first().cloned().unwrap_or_default(),
                byte_len: image.byte_len,
            })
            .collect())
    }

    fn bookmarks(&self) -> CoreResult<Vec<BookmarkItem>> {
        Ok(self.extract_bookmarks())
    }

    fn page_structure(&self, page: PageIndex) -> CoreResult<PageStructure> {
        self.ensure_page(page)?;
        let page_info = self.page_info(page)?;
        let text = self.structured_text(page)?;
        let images = self.structured_images(page)?;
        let mut watermarks = text
            .iter()
            .filter(|object| looks_like_watermark(&object.content))
            .map(|object| StructuredWatermark {
                kind: "text".to_string(),
                object_id: object.id.0,
                bounds: object.bounds,
                content: Some(object.content.clone()),
                source: "marked-content-or-text-heuristic".to_string(),
            })
            .collect::<Vec<_>>();
        watermarks.extend(
            images
                .iter()
                .filter(|object| {
                    object
                        .name
                        .as_deref()
                        .map(looks_like_watermark)
                        .unwrap_or(false)
                })
                .map(|object| StructuredWatermark {
                    kind: "image".to_string(),
                    object_id: object.id.0,
                    bounds: object.bounds,
                    content: object.name.clone(),
                    source: "xobject-name-heuristic".to_string(),
                }),
        );
        Ok(PageStructure {
            page: page_info,
            text,
            images,
            watermarks,
            annotations: self.structured_annotations(page)?,
            bookmarks: self
                .extract_bookmarks()
                .into_iter()
                .filter(|bookmark| bookmark.page == Some(page))
                .collect(),
        })
    }

    fn render_page(
        &self,
        page: PageIndex,
        scale: f32,
        max_pixels: u64,
    ) -> CoreResult<RenderedPage> {
        let info = self.page_info(page)?;
        let width_px = (info.size.width * scale).ceil().max(1.0) as u32;
        let height_px = (info.size.height * scale).ceil().max(1.0) as u32;
        let pixels = u64::from(width_px) * u64::from(height_px);
        if pixels > max_pixels {
            return Err(CoreError::Unsupported(format!(
                "render target has {pixels} pixels, above limit {max_pixels}"
            )));
        }

        Ok(RenderedPage {
            page,
            width_px,
            height_px,
            scale,
            rgba: vec![255; pixels as usize * 4],
        })
    }

    fn add_text_object(
        &mut self,
        _page: PageIndex,
        _bounds: Rect,
        _content: String,
        _style: TextStyle,
    ) -> CoreResult<TextObject> {
        Err(CoreError::Unsupported(
            "lopdf backend MVP only updates existing text objects".to_string(),
        ))
    }

    fn update_text_object(
        &mut self,
        id: TextObjectId,
        content: String,
        style: Option<TextStyle>,
    ) -> CoreResult<TextObject> {
        let current = self.find_text_object(id)?;
        let style = style.unwrap_or_else(|| TextStyle {
            font_name: current.font_name.clone(),
            font_size: current.font_size,
            color: current.color,
        });
        let runs = vec![TextRun::new(
            content,
            style.font_name,
            style.font_size,
            style.color,
        )];
        self.update_text_object_runs(id, runs)
    }

    fn update_text_object_runs(
        &mut self,
        id: TextObjectId,
        runs: Vec<TextRun>,
    ) -> CoreResult<TextObject> {
        let text_ref = self
            .text_refs
            .get(&id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("text object {}", (id.0).0)))?;
        let page_id = self.page_id(text_ref.page)?;
        let font_maps = self.page_font_maps(page_id);
        let content_bytes = self.document.get_page_content(page_id).map_err(|err| {
            CoreError::Engine(format!("failed to read page content stream: {err}"))
        })?;
        let mut content = Content::decode(&content_bytes)
            .map_err(|err| CoreError::Engine(format!("failed to decode content stream: {err}")))?;
        let replacement = runs
            .iter()
            .map(|run| run.content.as_str())
            .collect::<String>();

        let operation = content
            .operations
            .get_mut(text_ref.operation_index)
            .ok_or_else(|| CoreError::NotFound("text drawing operation".to_string()))?;
        let font_map = text_ref
            .font_name
            .as_ref()
            .and_then(|name| font_maps.get(name));
        replace_operation_text(operation, replacement, font_map)?;

        let encoded = content
            .encode()
            .map_err(|err| CoreError::Engine(format!("failed to encode content stream: {err}")))?;
        self.document
            .change_page_content(page_id, encoded)
            .map_err(|err| CoreError::Engine(format!("failed to write page content: {err}")))?;

        self.extract_text_objects()?;
        self.find_text_object(id)
    }

    fn update_text_object_bounds(
        &mut self,
        id: TextObjectId,
        bounds: Rect,
    ) -> CoreResult<TextObject> {
        for objects in self.text_objects.values_mut() {
            if let Some(object) = objects.iter_mut().find(|object| object.id == id) {
                object.bounds = bounds;
                return Ok(object.clone());
            }
        }

        Err(CoreError::NotFound(format!("text object {}", (id.0).0)))
    }

    fn save_to(&self, path: &Path) -> CoreResult<()> {
        let mut document = self.document.clone();
        document
            .save(path)
            .map_err(|err| CoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err)))?;
        Ok(())
    }
}

impl LopdfDocument {
    fn extract_text_objects(&mut self) -> CoreResult<()> {
        self.text_objects.clear();
        self.text_refs.clear();

        for page_index in 0..self.pages.len() {
            let page = PageIndex(page_index as u32);
            let page_id = self.page_id(page)?;
            let content_bytes = self.document.get_page_content(page_id).map_err(|err| {
                CoreError::Engine(format!("failed to read page content stream: {err}"))
            })?;
            let content = Content::decode(&content_bytes).map_err(|err| {
                CoreError::Engine(format!("failed to decode content stream: {err}"))
            })?;
            let font_maps = self.page_font_maps(page_id);
            let mut state = TextParseState::default();
            let mut objects = Vec::new();

            for (operation_index, operation) in content.operations.iter().enumerate() {
                update_text_state(&mut state, operation);
                let font_map = state
                    .font_name
                    .as_ref()
                    .and_then(|name| font_maps.get(name));
                if let Some(text) = operation_text(operation, font_map) {
                    let object_id = TextObjectId(PdfObjectId(encode_text_object_id(
                        page.0,
                        operation_index as u32,
                    )));
                    let bounds = Rect::new(
                        state.x,
                        state.y,
                        estimate_text_width(&text, state.font_size),
                        state.font_size * 1.2,
                    );
                    let run = TextRun::new(
                        text.clone(),
                        state.font_name.clone(),
                        state.font_size,
                        state.color,
                    );
                    objects.push(TextObject {
                        id: object_id,
                        page,
                        bounds,
                        content: text,
                        font_name: state.font_name.clone(),
                        font_size: state.font_size,
                        color: state.color,
                        runs: vec![run],
                    });
                    self.text_refs.insert(
                        object_id,
                        TextObjectRef {
                            page,
                            operation_index,
                            font_name: state.font_name.clone(),
                        },
                    );
                }
            }

            self.text_objects.insert(page, objects);
        }

        Ok(())
    }

    fn page_id(&self, page: PageIndex) -> CoreResult<ObjectId> {
        self.pages
            .get(page.0 as usize)
            .copied()
            .ok_or_else(|| CoreError::NotFound(format!("page {}", page.0)))
    }

    fn ensure_page(&self, page: PageIndex) -> CoreResult<()> {
        self.page_id(page).map(|_| ())
    }

    fn find_text_object(&self, id: TextObjectId) -> CoreResult<TextObject> {
        for objects in self.text_objects.values() {
            if let Some(object) = objects.iter().find(|object| object.id == id) {
                return Ok(object.clone());
            }
        }
        Err(CoreError::NotFound(format!("text object {}", (id.0).0)))
    }

    fn page_size(&self, page_id: ObjectId) -> Option<Size> {
        let page = self.document.get_object(page_id).ok()?.as_dict().ok()?;
        let media_box = page.get(b"MediaBox").ok()?.as_array().ok()?;
        if media_box.len() != 4 {
            return None;
        }

        let x0 = object_to_f32(&media_box[0])?;
        let y0 = object_to_f32(&media_box[1])?;
        let x1 = object_to_f32(&media_box[2])?;
        let y1 = object_to_f32(&media_box[3])?;
        Some(Size::new((x1 - x0).abs(), (y1 - y0).abs()))
    }

    fn page_font_maps(&self, page_id: ObjectId) -> HashMap<String, ToUnicodeMap> {
        let Ok(fonts) = self.document.get_page_fonts(page_id) else {
            return HashMap::new();
        };

        let mut maps = HashMap::new();
        for (name, font) in fonts {
            if let Some(map) = parse_font_to_unicode(&self.document, font) {
                maps.insert(String::from_utf8_lossy(&name).into_owned(), map);
            }
        }
        maps
    }

    fn structured_text(&self, page: PageIndex) -> CoreResult<Vec<StructuredTextObject>> {
        let page_id = self.page_id(page)?;
        let content = self.decoded_page_content(page_id)?;
        let font_maps = self.page_font_maps(page_id);
        let mut state = PageParseState::default();
        let mut objects = Vec::new();

        for (operation_index, operation) in content.operations.iter().enumerate() {
            update_page_state(&mut state, operation);
            let font_map = state
                .text
                .font_name
                .as_ref()
                .and_then(|name| font_maps.get(name));
            if let Some(text) = operation_text(operation, font_map) {
                let object_id = TextObjectId(PdfObjectId(encode_text_object_id(
                    page.0,
                    operation_index as u32,
                )));
                let transform = text_render_transform(&state);
                let bounds = bounds_for_text(&text, state.text.font_size, transform);
                objects.push(StructuredTextObject {
                    id: object_id,
                    bounds,
                    content: text.clone(),
                    font_name: state.text.font_name.clone(),
                    font_size: state.text.font_size,
                    color: state.text.color,
                    transform,
                    angle_degrees: matrix_angle_degrees(transform),
                    z_index: operation_index,
                    runs: vec![TextRun::new(
                        text,
                        state.text.font_name.clone(),
                        state.text.font_size,
                        state.text.color,
                    )],
                });
            }
        }

        Ok(objects)
    }

    fn structured_images(&self, page: PageIndex) -> CoreResult<Vec<StructuredImageObject>> {
        let page_id = self.page_id(page)?;
        let content = self.decoded_page_content(page_id)?;
        let xobjects = self.page_xobjects(page_id);
        let mut state = PageParseState::default();
        let mut images = Vec::new();

        for (operation_index, operation) in content.operations.iter().enumerate() {
            update_page_state(&mut state, operation);
            if operation.operator != "Do" {
                continue;
            }
            let Some(name) = operation.operands.first().and_then(object_name) else {
                continue;
            };
            let Some((object_id, stream)) = xobjects.get(&name) else {
                continue;
            };
            if stream
                .dict
                .get(b"Subtype")
                .ok()
                .and_then(object_name_bytes)
                .as_deref()
                != Some("Image")
            {
                continue;
            }
            let width_px = stream
                .dict
                .get(b"Width")
                .ok()
                .and_then(object_to_i64)
                .map(|value| value.max(0) as u32);
            let height_px = stream
                .dict
                .get(b"Height")
                .ok()
                .and_then(object_to_i64)
                .map(|value| value.max(0) as u32);
            let filters = stream
                .dict
                .get(b"Filter")
                .ok()
                .map(object_filter_names)
                .unwrap_or_default();
            images.push(StructuredImageObject {
                id: ImageObjectId(PdfObjectId(encode_indirect_object_id(*object_id))),
                name: Some(name),
                source_file: None,
                bounds: unit_bounds_after_transform(state.ctm),
                transform: state.ctm,
                angle_degrees: matrix_angle_degrees(state.ctm),
                width_px,
                height_px,
                color_space: stream
                    .dict
                    .get(b"ColorSpace")
                    .ok()
                    .and_then(object_color_space),
                bits_per_component: stream
                    .dict
                    .get(b"BitsPerComponent")
                    .ok()
                    .and_then(object_to_i64)
                    .map(|value| value.max(0) as u8),
                filters,
                byte_len: stream.content.len(),
                z_index: operation_index,
            });
        }

        Ok(images)
    }

    fn structured_annotations(&self, page: PageIndex) -> CoreResult<Vec<StructuredAnnotation>> {
        let page_id = self.page_id(page)?;
        Ok(self
            .page_annotation_entries(page_id)
            .into_iter()
            .map(|(id, annotation)| StructuredAnnotation {
                id: id.map(encode_indirect_object_id).map(PdfObjectId),
                subtype: annotation.get(b"Subtype").ok().and_then(object_name_bytes),
                bounds: annotation.get(b"Rect").ok().and_then(object_rect),
                contents: annotation.get(b"Contents").ok().and_then(object_plain_text),
                name: annotation.get(b"NM").ok().and_then(object_plain_text),
                flags: annotation.get(b"F").ok().and_then(object_to_i64),
            })
            .collect())
    }

    fn decoded_page_content(&self, page_id: ObjectId) -> CoreResult<Content> {
        let content_bytes = self.document.get_page_content(page_id).map_err(|err| {
            CoreError::Engine(format!("failed to read page content stream: {err}"))
        })?;
        Content::decode(&content_bytes)
            .map_err(|err| CoreError::Engine(format!("failed to decode content stream: {err}")))
    }

    fn page_xobjects(&self, page_id: ObjectId) -> HashMap<String, (ObjectId, lopdf::Stream)> {
        let mut result = HashMap::new();
        let Ok((resource_dict, resource_ids)) = self.document.get_page_resources(page_id) else {
            return result;
        };
        if let Some(resources) = resource_dict {
            collect_xobjects(&self.document, resources, &mut result);
        }
        for resource_id in resource_ids {
            if let Ok(resources) = self.document.get_dictionary(resource_id) {
                collect_xobjects(&self.document, resources, &mut result);
            }
        }
        result
    }

    fn page_annotation_entries(&self, page_id: ObjectId) -> Vec<(Option<ObjectId>, Dictionary)> {
        let mut annotations = Vec::new();
        let Ok(page) = self.document.get_dictionary(page_id) else {
            return annotations;
        };
        let Ok(annots) = page.get(b"Annots") else {
            return annotations;
        };
        let annot_objects = match annots {
            Object::Reference(id) => self
                .document
                .get_object(*id)
                .ok()
                .and_then(|object| object.as_array().ok())
                .cloned()
                .unwrap_or_default(),
            Object::Array(array) => array.clone(),
            _ => Vec::new(),
        };

        for object in annot_objects {
            match object {
                Object::Reference(id) => {
                    if let Ok(dict) = self.document.get_dictionary(id) {
                        annotations.push((Some(id), dict.clone()));
                    }
                }
                Object::Dictionary(dict) => annotations.push((None, dict)),
                _ => {}
            }
        }
        annotations
    }

    fn write_background_png(
        &self,
        page: PageIndex,
        output: &Path,
        options: BackgroundRenderOptions,
    ) -> CoreResult<BackgroundBitmapReport> {
        self.ensure_page(page)?;
        let page_id = self.page_id(page)?;
        let page_info = self.page_info(page)?;
        let scale = options.scale.max(0.1);
        let width_px = (page_info.size.width * scale).round().max(1.0) as u32;
        let height_px = (page_info.size.height * scale).round().max(1.0) as u32;
        let pixels = u64::from(width_px) * u64::from(height_px);
        if pixels > options.max_pixels {
            return Err(CoreError::Unsupported(format!(
                "background bitmap has {pixels} pixels, above limit {}",
                options.max_pixels
            )));
        }

        let mut pixmap = Pixmap::new(width_px, height_px)
            .ok_or_else(|| CoreError::Engine("failed to allocate background bitmap".to_string()))?;
        pixmap.fill(SkiaColor::from_rgba8(255, 255, 255, 255));

        let content = self.decoded_page_content(page_id)?;
        let mut state = GraphicsParseState::default();
        let mut path = PdfPath::default();
        let mut drawn_operations = 0usize;

        for operation in &content.operations {
            match operation.operator.as_str() {
                "q" => state.stack.push(state.snapshot()),
                "Q" => {
                    if let Some(snapshot) = state.stack.pop() {
                        state.restore(snapshot);
                    }
                }
                "cm" => {
                    if let Some(matrix) = operation_matrix(operation) {
                        state.ctm = multiply_matrix(state.ctm, matrix);
                    }
                }
                "w" => {
                    if let Some(width) = operation.operands.first().and_then(object_to_f32) {
                        state.line_width = width.max(0.0);
                    }
                }
                "RG" => {
                    if let Some(color) = rgb_color(operation) {
                        state.stroke_color = color;
                    }
                }
                "rg" => {
                    if let Some(color) = rgb_color(operation) {
                        state.fill_color = color;
                    }
                }
                "G" => {
                    if let Some(color) = gray_color(operation) {
                        state.stroke_color = color;
                    }
                }
                "g" => {
                    if let Some(color) = gray_color(operation) {
                        state.fill_color = color;
                    }
                }
                "m" => {
                    if let Some((x, y)) = operation_point(operation, 0) {
                        let (x, y) = transform_point(state.ctm, x, y);
                        path.move_to(x, y);
                    }
                }
                "l" => {
                    if let Some((x, y)) = operation_point(operation, 0) {
                        let (x, y) = transform_point(state.ctm, x, y);
                        path.line_to(x, y);
                    }
                }
                "c" => {
                    if let (Some(p1), Some(p2), Some(p3)) = (
                        operation_point(operation, 0),
                        operation_point(operation, 2),
                        operation_point(operation, 4),
                    ) {
                        let p1 = transform_point(state.ctm, p1.0, p1.1);
                        let p2 = transform_point(state.ctm, p2.0, p2.1);
                        let p3 = transform_point(state.ctm, p3.0, p3.1);
                        path.curve_to(p1, p2, p3);
                    }
                }
                "v" => {
                    if let (Some(p2), Some(p3)) =
                        (operation_point(operation, 0), operation_point(operation, 2))
                    {
                        let current = path.current_point().unwrap_or((0.0, 0.0));
                        let p2 = transform_point(state.ctm, p2.0, p2.1);
                        let p3 = transform_point(state.ctm, p3.0, p3.1);
                        path.curve_to(current, p2, p3);
                    }
                }
                "y" => {
                    if let (Some(p1), Some(p3)) =
                        (operation_point(operation, 0), operation_point(operation, 2))
                    {
                        let p1 = transform_point(state.ctm, p1.0, p1.1);
                        let p3 = transform_point(state.ctm, p3.0, p3.1);
                        path.curve_to(p1, p3, p3);
                    }
                }
                "re" => {
                    if let (Some(x), Some(y), Some(w), Some(h)) = (
                        operation.operands.first().and_then(object_to_f32),
                        operation.operands.get(1).and_then(object_to_f32),
                        operation.operands.get(2).and_then(object_to_f32),
                        operation.operands.get(3).and_then(object_to_f32),
                    ) {
                        let p0 = transform_point(state.ctm, x, y);
                        let p1 = transform_point(state.ctm, x + w, y);
                        let p2 = transform_point(state.ctm, x + w, y + h);
                        let p3 = transform_point(state.ctm, x, y + h);
                        path.move_to(p0.0, p0.1);
                        path.line_to(p1.0, p1.1);
                        path.line_to(p2.0, p2.1);
                        path.line_to(p3.0, p3.1);
                        path.close();
                    }
                }
                "h" => path.close(),
                "n" => path.clear(),
                "S" => {
                    if stroke_pdf_path(&mut pixmap, &path, &state, page_info.size.height, scale) {
                        drawn_operations += 1;
                    }
                    path.clear();
                }
                "s" => {
                    path.close();
                    if stroke_pdf_path(&mut pixmap, &path, &state, page_info.size.height, scale) {
                        drawn_operations += 1;
                    }
                    path.clear();
                }
                "f" | "F" | "f*" => {
                    if fill_pdf_path(&mut pixmap, &path, &state, page_info.size.height, scale) {
                        drawn_operations += 1;
                    }
                    path.clear();
                }
                "B" | "B*" => {
                    let filled =
                        fill_pdf_path(&mut pixmap, &path, &state, page_info.size.height, scale);
                    let stroked =
                        stroke_pdf_path(&mut pixmap, &path, &state, page_info.size.height, scale);
                    if filled || stroked {
                        drawn_operations += 1;
                    }
                    path.clear();
                }
                "b" | "b*" => {
                    path.close();
                    let filled =
                        fill_pdf_path(&mut pixmap, &path, &state, page_info.size.height, scale);
                    let stroked =
                        stroke_pdf_path(&mut pixmap, &path, &state, page_info.size.height, scale);
                    if filled || stroked {
                        drawn_operations += 1;
                    }
                    path.clear();
                }
                "Do" => {}
                _ => {}
            }
        }

        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        pixmap.save_png(output).map_err(|error| {
            CoreError::Engine(format!("failed to save background PNG: {error}"))
        })?;
        Ok(BackgroundBitmapReport {
            width_px,
            height_px,
            drawn_operations,
        })
    }

    fn write_page_images(
        &self,
        page: PageIndex,
        output_dir: &Path,
    ) -> CoreResult<Vec<PageImageExport>> {
        self.ensure_page(page)?;
        std::fs::create_dir_all(output_dir)?;
        let page_id = self.page_id(page)?;
        let content = self.decoded_page_content(page_id)?;
        let xobjects = self.page_xobjects(page_id);
        let mut state = PageParseState::default();
        let mut exported = Vec::new();

        for operation in &content.operations {
            update_page_state(&mut state, operation);
            if operation.operator != "Do" {
                continue;
            }
            let Some(name) = operation.operands.first().and_then(object_name) else {
                continue;
            };
            let Some((object_id, stream)) = xobjects.get(&name) else {
                continue;
            };
            if stream
                .dict
                .get(b"Subtype")
                .ok()
                .and_then(object_name_bytes)
                .as_deref()
                != Some("Image")
            {
                continue;
            }
            let Some(image) = decode_basic_image_xobject(&self.document, stream) else {
                continue;
            };

            let id = ImageObjectId(PdfObjectId(encode_indirect_object_id(*object_id)));
            let file_name = format!("{}.image.png", (id.0).0);
            let output = output_dir.join(&file_name);
            if !output.exists() {
                let mut pixmap = Pixmap::new(image.width, image.height).ok_or_else(|| {
                    CoreError::Engine("failed to allocate image export bitmap".to_string())
                })?;
                pixmap.data_mut().copy_from_slice(&image.premultiplied_rgba);
                pixmap.save_png(&output).map_err(|error| {
                    CoreError::Engine(format!("failed to save image object PNG: {error}"))
                })?;
            }

            if !exported.iter().any(|item: &PageImageExport| item.id == id) {
                exported.push(PageImageExport {
                    id,
                    file_name,
                    width_px: image.width,
                    height_px: image.height,
                });
            }
        }

        Ok(exported)
    }

    fn extract_bookmarks(&self) -> Vec<BookmarkItem> {
        let Ok(toc) = self.document.get_toc() else {
            return Vec::new();
        };
        toc.toc
            .into_iter()
            .map(|item| BookmarkItem {
                title: item.title,
                page: item.page.checked_sub(1).map(|page| PageIndex(page as u32)),
                level: item.level,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct GraphicsParseState {
    ctm: [f32; 6],
    stroke_color: Color,
    fill_color: Color,
    line_width: f32,
    stack: Vec<GraphicsStateSnapshot>,
}

#[derive(Debug, Clone)]
struct GraphicsStateSnapshot {
    ctm: [f32; 6],
    stroke_color: Color,
    fill_color: Color,
    line_width: f32,
}

impl Default for GraphicsParseState {
    fn default() -> Self {
        Self {
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            stroke_color: Color::BLACK,
            fill_color: Color::BLACK,
            line_width: 1.0,
            stack: Vec::new(),
        }
    }
}

impl GraphicsParseState {
    fn snapshot(&self) -> GraphicsStateSnapshot {
        GraphicsStateSnapshot {
            ctm: self.ctm,
            stroke_color: self.stroke_color,
            fill_color: self.fill_color,
            line_width: self.line_width,
        }
    }

    fn restore(&mut self, snapshot: GraphicsStateSnapshot) {
        self.ctm = snapshot.ctm;
        self.stroke_color = snapshot.stroke_color;
        self.fill_color = snapshot.fill_color;
        self.line_width = snapshot.line_width;
    }
}

#[derive(Debug, Clone, Default)]
struct PdfPath {
    segments: Vec<PathSegment>,
    current: Option<(f32, f32)>,
}

#[derive(Debug, Clone, Copy)]
enum PathSegment {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    CurveTo((f32, f32), (f32, f32), (f32, f32)),
    Close,
}

impl PdfPath {
    fn move_to(&mut self, x: f32, y: f32) {
        self.segments.push(PathSegment::MoveTo(x, y));
        self.current = Some((x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.segments.push(PathSegment::LineTo(x, y));
        self.current = Some((x, y));
    }

    fn curve_to(&mut self, p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)) {
        self.segments.push(PathSegment::CurveTo(p1, p2, p3));
        self.current = Some(p3);
    }

    fn close(&mut self) {
        self.segments.push(PathSegment::Close);
    }

    fn clear(&mut self) {
        self.segments.clear();
        self.current = None;
    }

    fn current_point(&self) -> Option<(f32, f32)> {
        self.current
    }

    fn to_skia_path(&self, page_height: f32, scale: f32) -> Option<tiny_skia::Path> {
        if self.segments.is_empty() {
            return None;
        }

        let mut builder = PathBuilder::new();
        for segment in &self.segments {
            match *segment {
                PathSegment::MoveTo(x, y) => {
                    let (x, y) = pdf_point_to_pixel(x, y, page_height, scale);
                    builder.move_to(x, y);
                }
                PathSegment::LineTo(x, y) => {
                    let (x, y) = pdf_point_to_pixel(x, y, page_height, scale);
                    builder.line_to(x, y);
                }
                PathSegment::CurveTo(p1, p2, p3) => {
                    let p1 = pdf_point_to_pixel(p1.0, p1.1, page_height, scale);
                    let p2 = pdf_point_to_pixel(p2.0, p2.1, page_height, scale);
                    let p3 = pdf_point_to_pixel(p3.0, p3.1, page_height, scale);
                    builder.cubic_to(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1);
                }
                PathSegment::Close => builder.close(),
            }
        }
        builder.finish()
    }
}

fn stroke_pdf_path(
    pixmap: &mut Pixmap,
    path: &PdfPath,
    state: &GraphicsParseState,
    page_height: f32,
    scale: f32,
) -> bool {
    let Some(path) = path.to_skia_path(page_height, scale) else {
        return false;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(
        state.stroke_color.r,
        state.stroke_color.g,
        state.stroke_color.b,
        state.stroke_color.a,
    );
    let stroke = Stroke {
        width: (state.line_width * scale).max(0.5),
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    true
}

fn fill_pdf_path(
    pixmap: &mut Pixmap,
    path: &PdfPath,
    state: &GraphicsParseState,
    page_height: f32,
    scale: f32,
) -> bool {
    let Some(path) = path.to_skia_path(page_height, scale) else {
        return false;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(
        state.fill_color.r,
        state.fill_color.g,
        state.fill_color.b,
        state.fill_color.a,
    );
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    true
}

#[derive(Debug, Clone)]
struct BasicImage {
    width: u32,
    height: u32,
    premultiplied_rgba: Vec<u8>,
}

fn decode_basic_image_xobject(document: &Document, stream: &lopdf::Stream) -> Option<BasicImage> {
    let mut image = if stream
        .dict
        .get(b"Filter")
        .ok()
        .map(object_filter_names)
        .unwrap_or_default()
        .iter()
        .any(|filter| filter == "DCTDecode")
    {
        decode_dct_image_xobject(stream)?
    } else {
        decode_raw_image_xobject(stream)?
    };

    apply_soft_mask(document, stream, &mut image);
    premultiply_image_alpha(&mut image.rgba);
    Some(BasicImage {
        width: image.width,
        height: image.height,
        premultiplied_rgba: image.rgba,
    })
}

#[derive(Debug, Clone)]
struct StraightImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn decode_raw_image_xobject(stream: &lopdf::Stream) -> Option<StraightImage> {
    let width = stream
        .dict
        .get(b"Width")
        .ok()
        .and_then(object_to_i64)
        .filter(|value| *value > 0)? as u32;
    let height = stream
        .dict
        .get(b"Height")
        .ok()
        .and_then(object_to_i64)
        .filter(|value| *value > 0)? as u32;
    let bits_per_component = stream
        .dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(object_to_i64)
        .unwrap_or(8);
    if bits_per_component != 8 {
        return None;
    }

    let color_space = stream
        .dict
        .get(b"ColorSpace")
        .ok()
        .and_then(object_color_space)
        .unwrap_or_else(|| "DeviceGray".to_string());
    let components = match color_space.as_str() {
        "DeviceRGB" => 3usize,
        "DeviceGray" => 1usize,
        _ => return None,
    };

    let bytes = stream
        .decompressed_content()
        .unwrap_or_else(|_| stream.content.clone());
    let expected_len = width as usize * height as usize * components;
    if bytes.len() < expected_len {
        return None;
    }

    let mut rgba = vec![0; width as usize * height as usize * 4];
    for pixel_index in 0..(width as usize * height as usize) {
        let source = pixel_index * components;
        let target = pixel_index * 4;
        match components {
            3 => {
                rgba[target] = bytes[source];
                rgba[target + 1] = bytes[source + 1];
                rgba[target + 2] = bytes[source + 2];
            }
            1 => {
                let gray = bytes[source];
                rgba[target] = gray;
                rgba[target + 1] = gray;
                rgba[target + 2] = gray;
            }
            _ => unreachable!(),
        }
        rgba[target + 3] = 255;
    }

    Some(StraightImage {
        width,
        height,
        rgba,
    })
}

fn decode_dct_image_xobject(stream: &lopdf::Stream) -> Option<StraightImage> {
    let image = image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg)
        .ok()?
        .to_rgba8();
    let (width, height) = image.dimensions();
    Some(StraightImage {
        width,
        height,
        rgba: image.into_raw(),
    })
}

fn apply_soft_mask(document: &Document, stream: &lopdf::Stream, image: &mut StraightImage) {
    let Ok(mask_object) = stream.dict.get(b"SMask") else {
        return;
    };
    let mask_stream = match mask_object {
        Object::Reference(id) => document
            .get_object(*id)
            .ok()
            .and_then(|object| object.as_stream().ok()),
        Object::Stream(stream) => Some(stream),
        _ => None,
    };
    let Some(mask_stream) = mask_stream else {
        return;
    };
    let Some(alpha) = decode_soft_mask_alpha(mask_stream, image.width, image.height) else {
        return;
    };
    let matte = mask_stream
        .dict
        .get(b"Matte")
        .ok()
        .and_then(object_rgb_array);

    for (pixel, alpha) in image.rgba.chunks_exact_mut(4).zip(alpha) {
        if let Some(matte) = matte {
            unmatte_pixel(pixel, alpha, matte);
        }
        pixel[3] = alpha;
    }
}

fn decode_soft_mask_alpha(stream: &lopdf::Stream, width: u32, height: u32) -> Option<Vec<u8>> {
    let mask_width = stream
        .dict
        .get(b"Width")
        .ok()
        .and_then(object_to_i64)
        .filter(|value| *value > 0)? as u32;
    let mask_height = stream
        .dict
        .get(b"Height")
        .ok()
        .and_then(object_to_i64)
        .filter(|value| *value > 0)? as u32;
    if mask_width != width || mask_height != height {
        return None;
    }
    let bits_per_component = stream
        .dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(object_to_i64)
        .unwrap_or(8);
    if bits_per_component != 8 {
        return None;
    }
    let color_space = stream
        .dict
        .get(b"ColorSpace")
        .ok()
        .and_then(object_color_space)
        .unwrap_or_else(|| "DeviceGray".to_string());
    if color_space != "DeviceGray" {
        return None;
    }
    let bytes = stream_content_bytes(stream)?;
    let expected_len = width as usize * height as usize;
    (bytes.len() >= expected_len).then(|| bytes[..expected_len].to_vec())
}

fn stream_content_bytes(stream: &lopdf::Stream) -> Option<Vec<u8>> {
    if stream
        .dict
        .get(b"Filter")
        .ok()
        .map(object_filter_names)
        .unwrap_or_default()
        .iter()
        .any(|filter| filter == "FlateDecode")
    {
        let mut decoder = flate2::read::ZlibDecoder::new(stream.content.as_slice());
        let mut output = Vec::new();
        decoder.read_to_end(&mut output).ok()?;
        return Some(output);
    }

    Some(
        stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone()),
    )
}

fn object_rgb_array(object: &Object) -> Option<[f32; 3]> {
    let array = object.as_array().ok()?;
    Some([
        array.first().and_then(object_to_f32)?.clamp(0.0, 1.0),
        array.get(1).and_then(object_to_f32)?.clamp(0.0, 1.0),
        array.get(2).and_then(object_to_f32)?.clamp(0.0, 1.0),
    ])
}

fn unmatte_pixel(pixel: &mut [u8], alpha: u8, matte: [f32; 3]) {
    if alpha == 0 {
        pixel[0] = 0;
        pixel[1] = 0;
        pixel[2] = 0;
        return;
    }
    let alpha = f32::from(alpha) / 255.0;
    for channel in 0..3 {
        let matte = matte[channel];
        let color = f32::from(pixel[channel]) / 255.0;
        let unpremultiplied = ((color - matte * (1.0 - alpha)) / alpha).clamp(0.0, 1.0);
        pixel[channel] = (unpremultiplied * 255.0).round() as u8;
    }
}

fn premultiply_image_alpha(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        pixel[0] = ((u16::from(pixel[0]) * alpha + 127) / 255) as u8;
        pixel[1] = ((u16::from(pixel[1]) * alpha + 127) / 255) as u8;
        pixel[2] = ((u16::from(pixel[2]) * alpha + 127) / 255) as u8;
    }
}

fn pdf_point_to_pixel(x: f32, y: f32, page_height: f32, scale: f32) -> (f32, f32) {
    (x * scale, (page_height - y) * scale)
}

fn operation_point(operation: &Operation, offset: usize) -> Option<(f32, f32)> {
    Some((
        operation.operands.get(offset).and_then(object_to_f32)?,
        operation.operands.get(offset + 1).and_then(object_to_f32)?,
    ))
}

fn rgb_color(operation: &Operation) -> Option<Color> {
    let r = operation.operands.first().and_then(object_to_f32)?;
    let g = operation.operands.get(1).and_then(object_to_f32)?;
    let b = operation.operands.get(2).and_then(object_to_f32)?;
    Some(Color::rgba(
        normalized_color_channel(r),
        normalized_color_channel(g),
        normalized_color_channel(b),
        255,
    ))
}

fn gray_color(operation: &Operation) -> Option<Color> {
    let value = normalized_color_channel(operation.operands.first().and_then(object_to_f32)?);
    Some(Color::rgba(value, value, value, 255))
}

#[derive(Debug, Clone)]
struct TextParseState {
    x: f32,
    y: f32,
    font_name: Option<String>,
    font_size: f32,
    color: Color,
}

#[derive(Debug, Clone)]
struct PageParseState {
    ctm: [f32; 6],
    stack: Vec<([f32; 6], TextParseState)>,
    text: TextParseState,
    text_matrix: [f32; 6],
}

impl Default for PageParseState {
    fn default() -> Self {
        Self {
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            stack: Vec::new(),
            text: TextParseState::default(),
            text_matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}

impl Default for TextParseState {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            font_name: None,
            font_size: 12.0,
            color: Color::BLACK,
        }
    }
}

fn update_page_state(state: &mut PageParseState, operation: &Operation) {
    match operation.operator.as_str() {
        "q" => state.stack.push((state.ctm, state.text.clone())),
        "Q" => {
            if let Some((ctm, text)) = state.stack.pop() {
                state.ctm = ctm;
                state.text = text;
            }
        }
        "cm" => {
            if let Some(matrix) = operation_matrix(operation) {
                state.ctm = multiply_matrix(state.ctm, matrix);
            }
        }
        "BT" => {
            state.text_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        }
        "Tf" => {
            if let Some(name) = operation.operands.first().and_then(object_name) {
                state.text.font_name = Some(name);
            }
            if let Some(size) = operation.operands.get(1).and_then(object_to_f32) {
                state.text.font_size = size;
            }
        }
        "Td" | "TD" => {
            if let (Some(x), Some(y)) = (
                operation.operands.first().and_then(object_to_f32),
                operation.operands.get(1).and_then(object_to_f32),
            ) {
                let translate = [1.0, 0.0, 0.0, 1.0, x, y];
                state.text_matrix = multiply_matrix(state.text_matrix, translate);
                state.text.x = state.text_matrix[4];
                state.text.y = state.text_matrix[5];
            }
        }
        "Tm" => {
            if let Some(matrix) = operation_matrix(operation) {
                state.text_matrix = matrix;
                state.text.x = matrix[4];
                state.text.y = matrix[5];
            }
        }
        "rg" => {
            if let (Some(r), Some(g), Some(b)) = (
                operation.operands.first().and_then(object_to_f32),
                operation.operands.get(1).and_then(object_to_f32),
                operation.operands.get(2).and_then(object_to_f32),
            ) {
                state.text.color = Color::rgba(
                    normalized_color_channel(r),
                    normalized_color_channel(g),
                    normalized_color_channel(b),
                    255,
                );
            }
        }
        _ => {}
    }
}

fn update_text_state(state: &mut TextParseState, operation: &Operation) {
    match operation.operator.as_str() {
        "Tf" => {
            if let Some(name) = operation.operands.first().and_then(object_name) {
                state.font_name = Some(name);
            }
            if let Some(size) = operation.operands.get(1).and_then(object_to_f32) {
                state.font_size = size;
            }
        }
        "Td" | "TD" => {
            if let (Some(x), Some(y)) = (
                operation.operands.first().and_then(object_to_f32),
                operation.operands.get(1).and_then(object_to_f32),
            ) {
                state.x += x;
                state.y += y;
            }
        }
        "Tm" => {
            if let (Some(x), Some(y)) = (
                operation.operands.get(4).and_then(object_to_f32),
                operation.operands.get(5).and_then(object_to_f32),
            ) {
                state.x = x;
                state.y = y;
            }
        }
        "rg" => {
            if let (Some(r), Some(g), Some(b)) = (
                operation.operands.first().and_then(object_to_f32),
                operation.operands.get(1).and_then(object_to_f32),
                operation.operands.get(2).and_then(object_to_f32),
            ) {
                state.color = Color::rgba(
                    normalized_color_channel(r),
                    normalized_color_channel(g),
                    normalized_color_channel(b),
                    255,
                );
            }
        }
        _ => {}
    }
}

fn operation_matrix(operation: &Operation) -> Option<[f32; 6]> {
    Some([
        operation.operands.first().and_then(object_to_f32)?,
        operation.operands.get(1).and_then(object_to_f32)?,
        operation.operands.get(2).and_then(object_to_f32)?,
        operation.operands.get(3).and_then(object_to_f32)?,
        operation.operands.get(4).and_then(object_to_f32)?,
        operation.operands.get(5).and_then(object_to_f32)?,
    ])
}

fn multiply_matrix(left: [f32; 6], right: [f32; 6]) -> [f32; 6] {
    [
        left[0] * right[0] + left[2] * right[1],
        left[1] * right[0] + left[3] * right[1],
        left[0] * right[2] + left[2] * right[3],
        left[1] * right[2] + left[3] * right[3],
        left[0] * right[4] + left[2] * right[5] + left[4],
        left[1] * right[4] + left[3] * right[5] + left[5],
    ]
}

fn text_render_transform(state: &PageParseState) -> [f32; 6] {
    multiply_matrix(
        state.ctm,
        multiply_matrix(
            state.text_matrix,
            [
                state.text.font_size,
                0.0,
                0.0,
                state.text.font_size,
                0.0,
                0.0,
            ],
        ),
    )
}

fn bounds_for_text(content: &str, font_size: f32, transform: [f32; 6]) -> Rect {
    let width = estimate_text_width(content, font_size) / font_size.max(1.0);
    transformed_rect_bounds(transform, width, 1.2)
}

fn unit_bounds_after_transform(transform: [f32; 6]) -> Rect {
    transformed_rect_bounds(transform, 1.0, 1.0)
}

fn transformed_rect_bounds(transform: [f32; 6], width: f32, height: f32) -> Rect {
    let points = [
        transform_point(transform, 0.0, 0.0),
        transform_point(transform, width, 0.0),
        transform_point(transform, 0.0, height),
        transform_point(transform, width, height),
    ];
    let min_x = points
        .iter()
        .map(|point| point.0)
        .fold(f32::INFINITY, f32::min);
    let max_x = points
        .iter()
        .map(|point| point.0)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = points
        .iter()
        .map(|point| point.1)
        .fold(f32::INFINITY, f32::min);
    let max_y = points
        .iter()
        .map(|point| point.1)
        .fold(f32::NEG_INFINITY, f32::max);
    Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
}

fn transform_point(transform: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (
        transform[0] * x + transform[2] * y + transform[4],
        transform[1] * x + transform[3] * y + transform[5],
    )
}

fn matrix_angle_degrees(transform: [f32; 6]) -> f32 {
    transform[1].atan2(transform[0]).to_degrees()
}

fn operation_text(operation: &Operation, font_map: Option<&ToUnicodeMap>) -> Option<String> {
    match operation.operator.as_str() {
        "Tj" | "'" | "\"" => operation
            .operands
            .last()
            .and_then(|object| object_text(object, font_map)),
        "TJ" => {
            let array = operation.operands.first()?.as_array().ok()?;
            let mut text = String::new();
            for item in array {
                if let Some(part) = object_text(item, font_map) {
                    text.push_str(&part);
                }
            }
            Some(text)
        }
        _ => None,
    }
}

fn replace_operation_text(
    operation: &mut Operation,
    replacement: String,
    font_map: Option<&ToUnicodeMap>,
) -> CoreResult<()> {
    let encoded_replacement = font_map.and_then(|map| map.encode(&replacement));
    match operation.operator.as_str() {
        "Tj" | "'" | "\"" => {
            let operand = operation.operands.last_mut().ok_or_else(|| {
                CoreError::InvalidPdf("text operation has no operand".to_string())
            })?;
            *operand = encoded_text_object(replacement, encoded_replacement);
            Ok(())
        }
        "TJ" => {
            let operand = operation
                .operands
                .first_mut()
                .ok_or_else(|| CoreError::InvalidPdf("TJ operation has no operand".to_string()))?;
            *operand = Object::Array(vec![encoded_text_object(replacement, encoded_replacement)]);
            Ok(())
        }
        operator => Err(CoreError::Unsupported(format!(
            "unsupported text operation {operator}"
        ))),
    }
}

fn encoded_text_object(replacement: String, encoded_replacement: Option<Vec<u8>>) -> Object {
    match encoded_replacement {
        Some(bytes) => Object::String(bytes, StringFormat::Hexadecimal),
        None => Object::string_literal(replacement),
    }
}

fn object_to_f32(object: &Object) -> Option<f32> {
    match object {
        Object::Integer(value) => Some(*value as f32),
        Object::Real(value) => Some(*value),
        _ => None,
    }
}

fn object_to_i64(object: &Object) -> Option<i64> {
    match object {
        Object::Integer(value) => Some(*value),
        _ => None,
    }
}

fn object_name(object: &Object) -> Option<String> {
    match object {
        Object::Name(value) => Some(String::from_utf8_lossy(value).into_owned()),
        _ => None,
    }
}

fn object_name_bytes(object: &Object) -> Option<String> {
    object
        .as_name()
        .ok()
        .map(|value| String::from_utf8_lossy(value).into_owned())
}

fn object_plain_text(object: &Object) -> Option<String> {
    match object {
        Object::String(value, _) => Some(String::from_utf8_lossy(value).into_owned()),
        Object::Name(value) => Some(String::from_utf8_lossy(value).into_owned()),
        _ => None,
    }
}

fn object_rect(object: &Object) -> Option<Rect> {
    let values = object.as_array().ok()?;
    if values.len() != 4 {
        return None;
    }
    let x0 = object_to_f32(&values[0])?;
    let y0 = object_to_f32(&values[1])?;
    let x1 = object_to_f32(&values[2])?;
    let y1 = object_to_f32(&values[3])?;
    Some(Rect::new(
        x0.min(x1),
        y0.min(y1),
        (x1 - x0).abs(),
        (y1 - y0).abs(),
    ))
}

fn object_filter_names(object: &Object) -> Vec<String> {
    match object {
        Object::Name(name) => vec![String::from_utf8_lossy(name).into_owned()],
        Object::Array(array) => array.iter().filter_map(object_name_bytes).collect(),
        _ => Vec::new(),
    }
}

fn object_color_space(object: &Object) -> Option<String> {
    match object {
        Object::Name(name) => Some(String::from_utf8_lossy(name).into_owned()),
        Object::Array(array) => array.first().and_then(object_name_bytes),
        _ => None,
    }
}

fn object_text(object: &Object, font_map: Option<&ToUnicodeMap>) -> Option<String> {
    match object {
        Object::String(value, _) => Some(
            font_map
                .map(|map| map.decode(value))
                .unwrap_or_else(|| String::from_utf8_lossy(value).into_owned()),
        ),
        _ => None,
    }
}

fn normalized_color_channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn estimate_text_width(content: &str, font_size: f32) -> f32 {
    content.chars().count() as f32 * font_size.max(1.0) * 0.6
}

fn encode_text_object_id(page: u32, operation_index: u32) -> u64 {
    (u64::from(page) << 32) | u64::from(operation_index)
}

fn encode_indirect_object_id(id: ObjectId) -> u64 {
    (u64::from(id.0) << 16) | u64::from(id.1)
}

fn collect_xobjects(
    document: &Document,
    resources: &Dictionary,
    result: &mut HashMap<String, (ObjectId, lopdf::Stream)>,
) {
    let Ok(xobjects) = resources.get(b"XObject") else {
        return;
    };
    let xobjects = match xobjects {
        Object::Reference(id) => document.get_dictionary(*id).ok(),
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    };
    let Some(xobjects) = xobjects else {
        return;
    };
    for (name, object) in xobjects.iter() {
        let Ok(id) = object.as_reference() else {
            continue;
        };
        let Ok(stream) = document.get_object(id).and_then(Object::as_stream) else {
            continue;
        };
        result
            .entry(String::from_utf8_lossy(name).into_owned())
            .or_insert_with(|| (id, stream.clone()));
    }
}

fn looks_like_watermark(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("watermark") || lower.contains("draft") || lower.contains("confidential")
}

#[derive(Debug, Clone, Default)]
struct ToUnicodeMap {
    forward: HashMap<Vec<u8>, String>,
    reverse: HashMap<String, Vec<u8>>,
    max_code_len: usize,
}

impl ToUnicodeMap {
    fn insert(&mut self, source: Vec<u8>, target: String) {
        if source.is_empty() || target.is_empty() {
            return;
        }
        self.max_code_len = self.max_code_len.max(source.len());
        self.reverse
            .entry(target.clone())
            .or_insert_with(|| source.clone());
        self.forward.insert(source, target);
    }

    fn decode(&self, bytes: &[u8]) -> String {
        let mut output = String::new();
        let mut index = 0usize;
        while index < bytes.len() {
            let max_len = self.max_code_len.min(bytes.len() - index).max(1);
            let mut matched = None;
            for len in (1..=max_len).rev() {
                if let Some(value) = self.forward.get(&bytes[index..index + len]) {
                    matched = Some((len, value));
                    break;
                }
            }

            if let Some((len, value)) = matched {
                output.push_str(value);
                index += len;
            } else {
                output.push(char::REPLACEMENT_CHARACTER);
                index += 1;
            }
        }
        output
    }

    fn encode(&self, text: &str) -> Option<Vec<u8>> {
        let mut output = Vec::new();
        for character in text.chars() {
            let key = character.to_string();
            let encoded = self.reverse.get(&key)?;
            output.extend_from_slice(encoded);
        }
        Some(output)
    }
}

fn parse_font_to_unicode(document: &Document, font: &Dictionary) -> Option<ToUnicodeMap> {
    let to_unicode = font.get(b"ToUnicode").ok()?;
    let stream = match to_unicode {
        Object::Reference(id) => document.get_object(*id).ok()?.as_stream().ok()?,
        Object::Stream(stream) => stream,
        _ => return None,
    };
    let content = stream.decompressed_content().ok()?;
    Some(parse_to_unicode_cmap(&content))
}

fn parse_to_unicode_cmap(content: &[u8]) -> ToUnicodeMap {
    let text = String::from_utf8_lossy(content);
    let mut map = ToUnicodeMap::default();
    let lines = text.lines().collect::<Vec<_>>();
    let mut index = 0usize;

    while index < lines.len() {
        let line = strip_cmap_comment(lines[index]).trim();
        if line.ends_with("beginbfchar") {
            index += 1;
            while index < lines.len() {
                let entry = strip_cmap_comment(lines[index]).trim();
                if entry.starts_with("endbfchar") {
                    break;
                }
                let hexes = extract_hex_strings(entry);
                if hexes.len() >= 2 {
                    map.insert(hexes[0].clone(), utf16be_to_string(&hexes[1]));
                }
                index += 1;
            }
        } else if line.ends_with("beginbfrange") {
            index += 1;
            while index < lines.len() {
                let entry = strip_cmap_comment(lines[index]).trim();
                if entry.starts_with("endbfrange") {
                    break;
                }
                parse_bfrange_entry(entry, &mut map);
                index += 1;
            }
        }
        index += 1;
    }

    map
}

fn parse_bfrange_entry(entry: &str, map: &mut ToUnicodeMap) {
    let hexes = extract_hex_strings(entry);
    if hexes.len() < 3 {
        return;
    }

    let Some(start) = bytes_to_u32(&hexes[0]) else {
        return;
    };
    let Some(end) = bytes_to_u32(&hexes[1]) else {
        return;
    };
    if end < start || end - start > 4096 {
        return;
    }

    let source_len = hexes[0].len();
    if entry.contains('[') {
        for (offset, target) in hexes.iter().skip(2).enumerate() {
            let code = start + offset as u32;
            if code > end {
                break;
            }
            map.insert(u32_to_bytes(code, source_len), utf16be_to_string(target));
        }
    } else {
        let mut target_units = bytes_to_u16_units(&hexes[2]);
        for code in start..=end {
            map.insert(
                u32_to_bytes(code, source_len),
                String::from_utf16_lossy(&target_units),
            );
            if let Some(last) = target_units.last_mut() {
                *last = last.wrapping_add(1);
            }
        }
    }
}

fn strip_cmap_comment(line: &str) -> &str {
    line.split_once('%').map(|(head, _)| head).unwrap_or(line)
}

fn extract_hex_strings(line: &str) -> Vec<Vec<u8>> {
    let mut result = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('<') {
        let after_start = &rest[start + 1..];
        if after_start.starts_with('<') {
            rest = &after_start[1..];
            continue;
        }
        let Some(end) = after_start.find('>') else {
            break;
        };
        let hex = &after_start[..end];
        if let Some(bytes) = hex_to_bytes(hex) {
            result.push(bytes);
        }
        rest = &after_start[end + 1..];
    }
    result
}

fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    let compact = hex
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    if compact.is_empty() {
        return Some(Vec::new());
    }

    let mut padded = compact;
    if padded.len() % 2 == 1 {
        padded.push('0');
    }

    let mut bytes = Vec::with_capacity(padded.len() / 2);
    for pair in padded.as_bytes().chunks_exact(2) {
        let pair = std::str::from_utf8(pair).ok()?;
        bytes.push(u8::from_str_radix(pair, 16).ok()?);
    }
    Some(bytes)
}

fn utf16be_to_string(bytes: &[u8]) -> String {
    String::from_utf16_lossy(&bytes_to_u16_units(bytes))
}

fn bytes_to_u16_units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks(2)
        .map(|chunk| {
            let high = u16::from(chunk[0]);
            let low = u16::from(*chunk.get(1).unwrap_or(&0));
            (high << 8) | low
        })
        .collect()
}

fn bytes_to_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.len() > 4 {
        return None;
    }
    let mut value = 0u32;
    for byte in bytes {
        value = (value << 8) | u32::from(*byte);
    }
    Some(value)
}

fn u32_to_bytes(value: u32, len: usize) -> Vec<u8> {
    (0..len)
        .rev()
        .map(|shift| ((value >> (shift * 8)) & 0xff) as u8)
        .collect()
}

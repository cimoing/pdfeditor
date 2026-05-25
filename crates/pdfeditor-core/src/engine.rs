use crate::{
    BookmarkItem, Color, CoreError, CoreResult, ImageObject, ImageObjectId, PageIndex, PageInfo,
    PageStructure, PdfObjectId, Rect, RenderedPage, Size, StructuredImageObject,
    StructuredTextObject, TextObject, TextObjectId, TextRun, TextStyle,
};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub trait PdfEngine {
    type Document: EngineDocument;

    fn open(&self, path: &Path) -> CoreResult<Self::Document>;
}

pub trait EngineDocument {
    fn page_count(&self) -> u32;
    fn page_info(&self, page: PageIndex) -> CoreResult<PageInfo>;
    fn text_objects(&self, page: PageIndex) -> CoreResult<Vec<TextObject>>;
    fn image_objects(&self, page: PageIndex) -> CoreResult<Vec<ImageObject>>;
    fn bookmarks(&self) -> CoreResult<Vec<BookmarkItem>> {
        Ok(Vec::new())
    }
    fn page_structure(&self, page: PageIndex) -> CoreResult<PageStructure> {
        let page_info = self.page_info(page)?;
        let text = self
            .text_objects(page)?
            .into_iter()
            .enumerate()
            .map(|(z_index, object)| StructuredTextObject {
                id: object.id,
                bounds: object.bounds,
                content: object.content,
                font_name: object.font_name,
                font_size: object.font_size,
                color: object.color,
                stroke_color: object.color,
                stroke_width: 0.0,
                rendering_mode: 0,
                char_spacing: 0.0,
                word_spacing: 0.0,
                horizontal_scaling: 100.0,
                transform: [
                    object.font_size,
                    0.0,
                    0.0,
                    object.font_size,
                    object.bounds.origin.x,
                    object.bounds.origin.y,
                ],
                angle_degrees: 0.0,
                z_index,
                glyphs: Vec::new(),
                punct_width_squeeze: false,
                font_features: Vec::new(),
                runs: object.runs,
            })
            .collect();
        let images = self
            .image_objects(page)?
            .into_iter()
            .enumerate()
            .map(|(z_index, object)| StructuredImageObject {
                id: object.id,
                name: None,
                source_file: None,
                bounds: object.bounds,
                transform: [
                    object.bounds.size.width,
                    0.0,
                    0.0,
                    object.bounds.size.height,
                    object.bounds.origin.x,
                    object.bounds.origin.y,
                ],
                angle_degrees: 0.0,
                width_px: None,
                height_px: None,
                color_space: None,
                bits_per_component: None,
                filters: vec![object.format],
                byte_len: object.byte_len,
                z_index,
            })
            .collect();
        Ok(PageStructure {
            page: page_info,
            text,
            visual_text: Vec::new(),
            images,
            watermarks: Vec::new(),
            annotations: Vec::new(),
            bookmarks: self
                .bookmarks()?
                .into_iter()
                .filter(|bookmark| bookmark.page == Some(page))
                .collect(),
        })
    }
    fn render_page(&self, page: PageIndex, scale: f32, max_pixels: u64)
        -> CoreResult<RenderedPage>;
    fn add_text_object(
        &mut self,
        page: PageIndex,
        bounds: Rect,
        content: String,
        style: TextStyle,
    ) -> CoreResult<TextObject>;
    fn update_text_object(
        &mut self,
        id: TextObjectId,
        content: String,
        style: Option<TextStyle>,
    ) -> CoreResult<TextObject>;
    fn update_text_object_runs(
        &mut self,
        id: TextObjectId,
        runs: Vec<TextRun>,
    ) -> CoreResult<TextObject>;
    fn update_text_object_bounds(
        &mut self,
        id: TextObjectId,
        bounds: Rect,
    ) -> CoreResult<TextObject>;
    fn save_to(&self, path: &Path) -> CoreResult<()>;
}

#[derive(Debug, Clone, Default)]
pub struct MockPdfEngine;

#[derive(Debug, Clone)]
pub struct MockEngineDocument {
    source: Vec<u8>,
    pages: Vec<PageInfo>,
    text_objects: HashMap<PageIndex, Vec<TextObject>>,
    image_objects: HashMap<PageIndex, Vec<ImageObject>>,
    next_object_id: u64,
}

impl PdfEngine for MockPdfEngine {
    type Document = MockEngineDocument;

    fn open(&self, path: &Path) -> CoreResult<Self::Document> {
        let source = fs::read(path)?;
        if !source.starts_with(b"%PDF-") {
            return Err(CoreError::InvalidPdf(
                "file does not start with a PDF header".to_string(),
            ));
        }

        let page_count = estimate_page_count(&source).max(1);
        let mut pages = Vec::with_capacity(page_count as usize);
        let mut text_objects = HashMap::new();
        let mut image_objects = HashMap::new();

        for index in 0..page_count {
            let page = PageIndex(index);
            pages.push(PageInfo {
                index: page,
                size: Size::new(595.0, 842.0),
                rotation: 0,
            });
            text_objects.insert(page, sample_text(page));
            image_objects.insert(page, sample_images(page));
        }

        Ok(MockEngineDocument {
            source,
            pages,
            text_objects,
            image_objects,
            next_object_id: 10_000,
        })
    }
}

impl EngineDocument for MockEngineDocument {
    fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    fn page_info(&self, page: PageIndex) -> CoreResult<PageInfo> {
        self.pages
            .get(page.0 as usize)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("page {}", page.0)))
    }

    fn text_objects(&self, page: PageIndex) -> CoreResult<Vec<TextObject>> {
        self.ensure_page(page)?;
        Ok(self.text_objects.get(&page).cloned().unwrap_or_default())
    }

    fn image_objects(&self, page: PageIndex) -> CoreResult<Vec<ImageObject>> {
        self.ensure_page(page)?;
        Ok(self.image_objects.get(&page).cloned().unwrap_or_default())
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

        let mut rgba = vec![255; pixels as usize * 4];
        for chunk in rgba.chunks_exact_mut(4) {
            chunk[0] = 248;
            chunk[1] = 248;
            chunk[2] = 245;
            chunk[3] = 255;
        }

        Ok(RenderedPage {
            page,
            width_px,
            height_px,
            scale,
            rgba,
        })
    }

    fn add_text_object(
        &mut self,
        page: PageIndex,
        bounds: Rect,
        content: String,
        style: TextStyle,
    ) -> CoreResult<TextObject> {
        self.ensure_page(page)?;
        let object = TextObject {
            id: TextObjectId(PdfObjectId(self.allocate_object_id())),
            page,
            bounds,
            content: content.clone(),
            font_name: style.font_name.clone(),
            font_size: style.font_size,
            color: style.color,
            runs: vec![TextRun::new(
                content,
                style.font_name,
                style.font_size,
                style.color,
            )],
        };
        self.text_objects
            .entry(page)
            .or_default()
            .push(object.clone());
        Ok(object)
    }

    fn update_text_object(
        &mut self,
        id: TextObjectId,
        content: String,
        style: Option<TextStyle>,
    ) -> CoreResult<TextObject> {
        for objects in self.text_objects.values_mut() {
            if let Some(object) = objects.iter_mut().find(|object| object.id == id) {
                object.content = content;
                if let Some(style) = style {
                    object.font_name = style.font_name.clone();
                    object.font_size = style.font_size;
                    object.color = style.color;
                    object.runs = vec![TextRun::new(
                        object.content.clone(),
                        style.font_name,
                        style.font_size,
                        style.color,
                    )];
                } else {
                    object.runs = vec![TextRun::new(
                        object.content.clone(),
                        object.font_name.clone(),
                        object.font_size,
                        object.color,
                    )];
                }
                return Ok(object.clone());
            }
        }

        Err(CoreError::NotFound(format!("text object {}", (id.0).0)))
    }

    fn update_text_object_runs(
        &mut self,
        id: TextObjectId,
        runs: Vec<TextRun>,
    ) -> CoreResult<TextObject> {
        for objects in self.text_objects.values_mut() {
            if let Some(object) = objects.iter_mut().find(|object| object.id == id) {
                object.content = runs
                    .iter()
                    .map(|run| run.content.as_str())
                    .collect::<String>();
                if let Some(first_run) = runs.first() {
                    object.font_name = first_run.font_name.clone();
                    object.font_size = first_run.font_size;
                    object.color = first_run.color;
                }
                object.runs = runs;
                return Ok(object.clone());
            }
        }

        Err(CoreError::NotFound(format!("text object {}", (id.0).0)))
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
        let mut output = self.source.clone();
        output.extend_from_slice(b"\n% pdfeditor-core mock text objects\n");
        for page in &self.pages {
            if let Some(objects) = self.text_objects.get(&page.index) {
                for object in objects {
                    output.extend_from_slice(
                        format!(
                            "% page={} text_id={} content={} runs={}\n",
                            page.index.0,
                            (object.id.0).0,
                            object.content.replace('\n', "\\n"),
                            object.runs.len()
                        )
                        .as_bytes(),
                    );
                    for (index, run) in object.runs.iter().enumerate() {
                        output.extend_from_slice(
                            format!(
                                "% run={} font={} size={:.1} rgba={},{},{},{} content={}\n",
                                index,
                                run.font_name.as_deref().unwrap_or(""),
                                run.font_size,
                                run.color.r,
                                run.color.g,
                                run.color.b,
                                run.color.a,
                                run.content.replace('\n', "\\n")
                            )
                            .as_bytes(),
                        );
                    }
                }
            }
        }
        fs::write(path, output)?;
        Ok(())
    }
}

impl MockEngineDocument {
    fn ensure_page(&self, page: PageIndex) -> CoreResult<()> {
        if page.0 < self.page_count() {
            Ok(())
        } else {
            Err(CoreError::NotFound(format!("page {}", page.0)))
        }
    }

    fn allocate_object_id(&mut self) -> u64 {
        let id = self.next_object_id;
        self.next_object_id += 1;
        id
    }
}

fn estimate_page_count(source: &[u8]) -> u32 {
    let text = String::from_utf8_lossy(source);
    let count = text.matches("/Type /Page").count();
    count.saturating_sub(text.matches("/Type /Pages").count()) as u32
}

fn sample_text(page: PageIndex) -> Vec<TextObject> {
    let content = format!("Page {}", page.0 + 1);
    vec![TextObject {
        id: TextObjectId(PdfObjectId(1 + u64::from(page.0) * 10)),
        page,
        bounds: Rect::new(72.0, 72.0, 240.0, 28.0),
        content: content.clone(),
        font_name: Some("Helvetica".to_string()),
        font_size: 12.0,
        color: Color::BLACK,
        runs: vec![TextRun::new(
            content,
            Some("Helvetica".to_string()),
            12.0,
            Color::BLACK,
        )],
    }]
}

fn sample_images(page: PageIndex) -> Vec<ImageObject> {
    vec![ImageObject {
        id: ImageObjectId(PdfObjectId(2 + u64::from(page.0) * 10)),
        page,
        bounds: Rect::new(72.0, 120.0, 96.0, 72.0),
        format: "placeholder".to_string(),
        byte_len: 0,
    }]
}

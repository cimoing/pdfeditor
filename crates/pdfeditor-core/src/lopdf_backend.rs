use crate::{
    BookmarkItem, Color, CoreError, CoreResult, EngineDocument, HitTestResult, ImageObject,
    ImageObjectId, LayoutGlyph, PageIndex, PageInfo, PageStructure, PdfEngine, PdfObjectId, Point,
    Rect, RenderedPage, Size, StructuredAnnotation, StructuredImageObject, StructuredTextObject,
    StructuredVisualTextObject, StructuredWatermark, TextEditSessionInfo, TextLayoutPreview,
    TextObject, TextObjectId, TextRun, TextStyle,
};
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, StringFormat};
use std::collections::{BTreeMap, HashMap, HashSet};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImageBytesExport {
    pub id: ImageObjectId,
    pub file_name: String,
    pub width_px: u32,
    pub height_px: u32,
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageFontAsset {
    pub resource_name: String,
    pub family_name: String,
    pub font_weight: u16,
    pub is_bold: bool,
    pub file_name: String,
    pub mime_type: String,
    pub format: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PageLoadBundle {
    pub structure: PageStructure,
    pub background_png: Vec<u8>,
    pub images: Vec<PageImageBytesExport>,
    pub fonts: Vec<PageFontAsset>,
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

pub fn page_structure_from_pdf_bytes(bytes: &[u8], page: PageIndex) -> CoreResult<PageStructure> {
    let document = open_lopdf_document_from_bytes(bytes)?;
    document.page_structure(page)
}

pub fn page_background_png_from_pdf_bytes(
    bytes: &[u8],
    page: PageIndex,
    options: BackgroundRenderOptions,
) -> CoreResult<Vec<u8>> {
    let document = open_lopdf_document_from_bytes(bytes)?;
    document
        .background_png_bytes(page, options)
        .map(|(png, _)| png)
}

pub fn page_image_png_from_pdf_bytes(
    bytes: &[u8],
    page: PageIndex,
    image_id: ImageObjectId,
) -> CoreResult<Vec<u8>> {
    let document = open_lopdf_document_from_bytes(bytes)?;
    let images = document.page_image_png_bytes(page)?;
    images
        .into_iter()
        .find(|image| image.id == image_id)
        .map(|image| image.png)
        .ok_or_else(|| CoreError::NotFound(format!("image object {}", (image_id.0).0)))
}

pub fn page_font_assets_from_pdf_bytes(
    bytes: &[u8],
    page: PageIndex,
) -> CoreResult<Vec<PageFontAsset>> {
    let document = open_lopdf_document_from_bytes(bytes)?;
    document.page_font_assets(page)
}

pub fn page_load_bundle_from_pdf_bytes(
    bytes: &[u8],
    page: PageIndex,
    options: BackgroundRenderOptions,
) -> CoreResult<PageLoadBundle> {
    let document = open_lopdf_document_from_bytes(bytes)?;
    document.page_load_bundle(page, options)
}

pub fn save_pdf_document_to_bytes(document: &LopdfDocument) -> CoreResult<Vec<u8>> {
    document.save_to_bytes()
}

pub fn open_lopdf_document_from_bytes(bytes: &[u8]) -> CoreResult<LopdfDocument> {
    let document = Document::load_mem(bytes)
        .map_err(|err| CoreError::InvalidPdf(format!("failed to load PDF bytes: {err}")))?;
    let page_labels = document.get_pages();
    let pages = page_labels.values().copied().collect::<Vec<_>>();
    let mut result = LopdfDocument {
        document,
        pages,
        text_objects: HashMap::new(),
        text_refs: HashMap::new(),
        text_edit_groups: HashMap::new(),
    };
    result.extract_text_objects()?;
    Ok(result)
}

#[derive(Debug, Clone)]
pub struct LopdfDocument {
    document: Document,
    pages: Vec<ObjectId>,
    text_objects: HashMap<PageIndex, Vec<TextObject>>,
    text_refs: HashMap<TextObjectId, TextObjectRef>,
    text_edit_groups: HashMap<TextObjectId, TextEditGroup>,
}

#[derive(Debug, Clone)]
struct TextObjectRef {
    page: PageIndex,
    operation_index: usize,
    font_name: Option<String>,
}

#[derive(Debug, Clone)]
struct TextEditGroup {
    page: PageIndex,
    member_ids: Vec<TextObjectId>,
    bounds: Rect,
    matrix: [f32; 6],
    font_name: Option<String>,
    font_size: f32,
}

#[derive(Debug, Clone)]
struct GroupMemberPlan {
    id: TextObjectId,
    original_content: String,
    original_char_count: usize,
    font_name: Option<String>,
    font_size: f32,
    font_map: Option<ToUnicodeMap>,
    metrics: Option<FontMetrics>,
    template: Object,
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
            text_edit_groups: HashMap::new(),
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
        let rotation = self.page_rotation(page_id);
        Ok(PageInfo {
            index: page,
            size,
            rotation,
        })
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
        let raw_text = self.structured_text(page)?;
        // Merge scatter-group members into a single object per logical group so
        // the page structure presents one entry per editable text unit.
        let text = self.merge_structured_text_groups(raw_text);
        let visual_text = self.structured_visual_text(page)?;
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
            visual_text,
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
        let edit_group = self.text_edit_group(id)?;
        let page_id = self.page_id(edit_group.page)?;
        let font_maps = self.page_font_maps(page_id);
        let font_metrics = self.page_font_metrics(page_id);
        let content_bytes = self.document.get_page_content(page_id).map_err(|err| {
            CoreError::Engine(format!("failed to read page content stream: {err}"))
        })?;
        let mut content = Content::decode(&content_bytes)
            .map_err(|err| CoreError::Engine(format!("failed to decode content stream: {err}")))?;
        let replacement = runs
            .iter()
            .map(|run| run.content.as_str())
            .collect::<String>();
        let current_objects = self
            .text_objects
            .get(&edit_group.page)
            .cloned()
            .unwrap_or_default();
        let member_plans = edit_group
            .member_ids
            .iter()
            .map(|member_id| {
                let text_ref = self
                    .text_refs
                    .get(member_id)
                    .cloned()
                    .ok_or_else(|| CoreError::NotFound(format!("text object {}", ((member_id.0).0))))?;
                let object = current_objects
                    .iter()
                    .find(|object| object.id == *member_id)
                    .cloned()
                    .ok_or_else(|| CoreError::NotFound(format!("text object {}", ((member_id.0).0))))?;
                let operation = content
                    .operations
                    .get(text_ref.operation_index)
                    .ok_or_else(|| CoreError::NotFound("text drawing operation".to_string()))?;
                let font_map = text_ref
                    .font_name
                    .as_ref()
                    .and_then(|name| font_maps.get(name))
                    .cloned();
                let metrics = text_ref
                    .font_name
                    .as_ref()
                    .and_then(|name| font_metrics.get(name))
                    .cloned();
                Ok(GroupMemberPlan {
                    id: *member_id,
                    original_content: object.content.clone(),
                    original_char_count: object.content.chars().count(),
                    font_name: text_ref.font_name.clone(),
                    font_size: object.font_size,
                    font_map,
                    metrics,
                    template: operation
                        .operands
                        .first()
                        .cloned()
                        .unwrap_or(Object::Null),
                })
            })
            .collect::<CoreResult<Vec<_>>>()?;
        let original_group_text = member_plans
            .iter()
            .map(|plan| plan.original_content.as_str())
            .collect::<String>();
        let segments = repartition_group_text(&original_group_text, &replacement, &member_plans)
            .unwrap_or_else(|_| proportional_split(&replacement, &member_plans));
        // Once a text edit group needs CJK fallback, keep the whole edited line on the
        // fallback font. Mixing fallback and the original font in one line makes PDF viewers
        // use different glyph metrics than the SVG preview.
        let use_group_fallback = member_plans
            .iter()
            .zip(segments.iter())
            .any(|(member, segment)| needs_cjk_fallback_font(member, segment));
        let needs_char_fallback =
            !use_group_fallback && member_plans.iter().zip(segments.iter()).any(|(member, segment)| {
                segment.chars().any(|ch| {
                    replacement_text_object(
                        &member.template,
                        ch.to_string(),
                        member.font_map.as_ref(),
                    )
                    .is_err()
                })
            });
        let fallback_font_name = if use_group_fallback || needs_char_fallback {
            Some(self.ensure_page_cjk_fallback_font(page_id)?)
        } else {
            None
        };
        let segment_map = member_plans
            .iter()
            .zip(segments)
            .map(|(member, replacement)| (member.id, replacement))
            .collect::<HashMap<_, _>>();
        let member_map = member_plans
            .iter()
            .map(|member| (member.id, member))
            .collect::<HashMap<_, _>>();
        let targeted_operation_indexes = edit_group
            .member_ids
            .iter()
            .filter_map(|member_id| {
                self.text_refs
                    .get(member_id)
                    .map(|text_ref| (text_ref.operation_index, *member_id))
            })
            .collect::<BTreeMap<_, _>>();

        // Locate the BT..ET block that wraps the targeted operations so we can
        // insert a clipping path around it.  PDF path operators (re, W, n) are
        // not valid inside a BT block, so the clip must be placed outside.
        // Search backwards from the first targeted op for the opening BT, and
        // forwards from the last targeted op for the closing ET.
        //
        // IMPORTANT: only apply the clip when the BT..ET block contains *only*
        // the targeted text-drawing operations.  If other Tj/TJ ops are present
        // in the same BT..ET block (a common PDF structure where many objects
        // share a single text block), wrapping the whole block with a narrow clip
        // rect would make every non-targeted text object invisible.
        //
        // Use the most-accurate clip bounds: if the text already has a `clip_bounds`
        // from a prior overflow save, preserve that (it represents the truly-original
        // text width); otherwise fall back to the current accurate per-metrics bounds.
        let layout_context = self.text_layout_context(id)?;
        let clip_bounds = layout_context
            .object
            .clip_bounds
            .unwrap_or(layout_context.object.bounds);
        let first_targeted_op = targeted_operation_indexes.keys().copied().next();
        let last_targeted_op = targeted_operation_indexes.keys().copied().last();
        let clip_open_before_bt = first_targeted_op.and_then(|first| {
            content.operations[..first]
                .iter()
                .rposition(|op| op.operator == "BT")
        });
        let clip_close_after_et = last_targeted_op.and_then(|last| {
            content.operations[last..]
                .iter()
                .position(|op| op.operator == "ET")
                .map(|rel| rel + last)
        });
        // Only clip when both markers are present, in the correct order, AND the
        // BT..ET block contains no non-targeted text-drawing operations.
        let text_drawing_ops: HashSet<&str> =
            ["Tj", "TJ", "'", "\""].iter().copied().collect();
        let (clip_open_before_bt, clip_close_after_et) =
            match (clip_open_before_bt, clip_close_after_et) {
                (Some(bt), Some(et)) if bt < et => {
                    // Check: are there any Tj/TJ/etc. operations between bt..=et
                    // that are NOT in the targeted set?
                    let has_non_targeted_text = content.operations[bt..=et]
                        .iter()
                        .enumerate()
                        .any(|(rel_idx, op)| {
                            text_drawing_ops.contains(op.operator.as_str())
                                && !targeted_operation_indexes.contains_key(&(bt + rel_idx))
                        });
                    if has_non_targeted_text {
                        (None, None)
                    } else {
                        (Some(bt), Some(et))
                    }
                }
                _ => (None, None),
            };

        // Pre-pass: collect text_matrix and text state at each targeted operation.
        // State is recorded BEFORE the operation executes so we capture the matrix
        // that was active when the original Tj/TJ was rendered.
        // Also compute the TJ compression factor per operation so the scatter can
        // reproduce the same horizontal tightening in the replacement text.
        let mut pre_pass_state = PageParseState::default();
        let mut operation_states: HashMap<usize, ([f32; 6], TextParseState)> = HashMap::new();
        let mut tj_compression_by_op: HashMap<usize, f32> = HashMap::new();
        for (op_index, operation) in content.operations.iter().enumerate() {
            if targeted_operation_indexes.contains_key(&op_index) {
                operation_states.insert(op_index, (pre_pass_state.text_matrix, pre_pass_state.text.clone()));
                let op_font_map = pre_pass_state.text.font_name.as_ref()
                    .and_then(|n| font_maps.get(n));
                let op_metrics = pre_pass_state.text.font_name.as_ref()
                    .and_then(|n| font_metrics.get(n));
                let factor = tj_compression_factor(
                    operation,
                    op_font_map,
                    op_metrics,
                    &pre_pass_state.text,
                );
                tj_compression_by_op.insert(op_index, factor);
            }
            update_page_state(&mut pre_pass_state, operation);
            let metrics_for_advance = pre_pass_state.text.font_name.as_ref()
                .and_then(|name| font_metrics.get(name));
            advance_page_text_state(&mut pre_pass_state, operation, metrics_for_advance);
        }

        let first_member_id = edit_group.member_ids.first().copied();

        // Anchor for the entire group's scatter: always use the first member's text matrix so
        // that, regardless of which member repartition assigns chars to (e.g. when earlier
        // members can't encode the replacement), all characters are placed starting at the
        // original beginning of the group rather than wherever the capable member happens to sit.
        let anchor_text_matrix = first_member_id
            .and_then(|id| self.text_refs.get(&id))
            .and_then(|text_ref| operation_states.get(&text_ref.operation_index))
            .map(|(tm, _)| *tm)
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

        // Each member emits up to 2*N+2 operations (Tm+Tj per char, plus optional Tf).
        let estimated_capacity = content.operations.len()
            + member_plans.iter().map(|p| p.original_char_count * 2 + 4).sum::<usize>();
        let mut rebuilt_operations: Vec<Operation> = Vec::with_capacity(estimated_capacity);
        let mut updated_member_ids = HashMap::new();

        // Running x offset (in PDF user-space points) shared across all group members so that
        // chars placed by later members continue seamlessly after earlier members' chars.
        let mut global_scatter_pts = 0.0f32;

        for (operation_index, operation) in content.operations.into_iter().enumerate() {
            if let Some(member_id) = targeted_operation_indexes.get(&operation_index) {
                let member_plan = member_map.get(member_id).ok_or_else(|| {
                    CoreError::NotFound(format!("text object {}", (((*member_id).0).0)))
                })?;
                let replacement_segment = segment_map.get(member_id).cloned().ok_or_else(|| {
                    CoreError::Engine("missing replacement segment for grouped text".to_string())
                })?;
                let (_text_matrix, text_state) = operation_states
                    .get(&operation_index)
                    .cloned()
                    .unwrap_or_else(|| ([1.0, 0.0, 0.0, 1.0, 0.0, 0.0], TextParseState::default()));

                if use_group_fallback {
                    if Some(*member_id) == first_member_id {
                        let fallback = fallback_font_name.as_deref().ok_or_else(|| {
                            CoreError::Engine("missing fallback font resource".to_string())
                        })?;
                        rebuilt_operations.push(font_set_operation(fallback, member_plan.font_size));
                        rebuilt_operations.push(Operation::new("Tc", vec![Object::Integer(0)]));
                        rebuilt_operations.push(Operation::new("Tw", vec![Object::Integer(0)]));
                        rebuilt_operations.push(Operation::new(
                            "Tm",
                            vec![
                                Object::Real(anchor_text_matrix[0]),
                                Object::Real(anchor_text_matrix[1]),
                                Object::Real(anchor_text_matrix[2]),
                                Object::Real(anchor_text_matrix[3]),
                                Object::Real(anchor_text_matrix[4]),
                                Object::Real(anchor_text_matrix[5]),
                            ],
                        ));
                        let tj_index = rebuilt_operations.len();
                        rebuilt_operations.push(Operation::new(
                            "Tj",
                            vec![Object::String(
                                utf16be_bytes(&replacement),
                                StringFormat::Hexadecimal,
                            )],
                        ));
                        if text_state.char_spacing != 0.0 {
                            rebuilt_operations.push(Operation::new(
                                "Tc",
                                vec![Object::Real(text_state.char_spacing)],
                            ));
                        }
                        if text_state.word_spacing != 0.0 {
                            rebuilt_operations.push(Operation::new(
                                "Tw",
                                vec![Object::Real(text_state.word_spacing)],
                            ));
                        }
                        if let Some(font_name) = member_plan.font_name.as_deref() {
                            rebuilt_operations.push(font_set_operation(font_name, member_plan.font_size));
                        }
                        let updated_id = TextObjectId(PdfObjectId(encode_text_object_id(
                            edit_group.page.0,
                            tj_index as u32,
                        )));
                        for group_member_id in &edit_group.member_ids {
                            updated_member_ids.insert(*group_member_id, updated_id);
                        }
                    }
                    continue;
                }

                // Scatter: emit one Tm+Tj per character.
                // All characters use `anchor_text_matrix` (first member's matrix) as the base
                // so chars are always placed starting from the group's original x position,
                // with `global_scatter_pts` advancing continuously across members.
                //
                // If the original operation was a TJ with positive spacing adjustments
                // (i.e. characters were packed tighter than font metrics alone), we reproduce
                // the same proportional compression on the replacement text so the saved PDF
                // matches the visual density of the original.
                let tj_compression = tj_compression_by_op.get(&operation_index).copied().unwrap_or(1.0);
                let chars: Vec<char> = replacement_segment.chars().collect();
                let mut first_tj_index: Option<usize> = None;
                // Tracks whether we have switched to the fallback font for the current char.
                let mut char_font_is_fallback = false;
                for (char_idx, ch) in chars.iter().copied().enumerate() {
                    let char_str = ch.to_string();

                    // Determine encoding before emitting Tm so we can insert Tf first.
                    let (char_obj, char_needs_fallback) = if use_group_fallback {
                        (
                            Object::String(utf16be_bytes(&char_str), StringFormat::Hexadecimal),
                            true,
                        )
                    } else {
                        match replacement_text_object(
                            &member_plan.template,
                            char_str.clone(),
                            member_plan.font_map.as_ref(),
                        ) {
                            Ok(obj) => (obj, false),
                            Err(_) => (
                                Object::String(utf16be_bytes(&char_str), StringFormat::Hexadecimal),
                                true,
                            ),
                        }
                    };

                    // Per-char font switch: emit Tf when the required font changes.
                    if char_needs_fallback && !char_font_is_fallback {
                        let fallback = fallback_font_name.as_deref().ok_or_else(|| {
                            CoreError::Engine("missing fallback font resource".to_string())
                        })?;
                        rebuilt_operations.push(font_set_operation(fallback, member_plan.font_size));
                        char_font_is_fallback = true;
                    } else if !char_needs_fallback && char_font_is_fallback {
                        if let Some(font_name) = member_plan.font_name.as_deref() {
                            rebuilt_operations.push(font_set_operation(font_name, member_plan.font_size));
                        }
                        char_font_is_fallback = false;
                    }

                    // Text matrix for this character: offset from the group anchor by the
                    // accumulated advance across all previously placed chars.
                    let char_tm = multiply_matrix(
                        anchor_text_matrix,
                        [1.0, 0.0, 0.0, 1.0, global_scatter_pts, 0.0],
                    );
                    rebuilt_operations.push(Operation::new(
                        "Tm",
                        vec![
                            Object::Real(char_tm[0]),
                            Object::Real(char_tm[1]),
                            Object::Real(char_tm[2]),
                            Object::Real(char_tm[3]),
                            Object::Real(char_tm[4]),
                            Object::Real(char_tm[5]),
                        ],
                    ));

                    let tj_index = rebuilt_operations.len();
                    if first_tj_index.is_none() {
                        first_tj_index = Some(tj_index);
                    }

                    rebuilt_operations.push(Operation::new("Tj", vec![char_obj]));

                    // Advance the global cursor by this glyph's width in user-space points.
                    let glyph_advance = if char_needs_fallback {
                        fallback_char_advance(ch) * (text_state.horizontal_scaling / 100.0)
                    } else {
                        let encoded = member_plan.font_map.as_ref().and_then(|m| m.encode(&char_str));
                        encoded
                            .as_deref()
                            .and_then(|bytes| {
                                member_plan.metrics.as_ref().map(|m| m.text_advance(bytes, &text_state))
                            })
                            .unwrap_or_else(|| {
                                estimate_text_width(&char_str, member_plan.font_size)
                                    / member_plan.font_size.max(1.0)
                            })
                    };
                    // Apply TJ compression to glyph advance (char_spacing is a separate
                    // PDF state and is NOT compressed — it is already accounted for
                    // independently of the TJ displacement mechanism).
                    global_scatter_pts += glyph_advance * member_plan.font_size * tj_compression;
                    // Add inter-character spacing in points (between chars, not after the last).
                    if char_idx + 1 < chars.len() {
                        global_scatter_pts +=
                            text_state.char_spacing * (text_state.horizontal_scaling / 100.0);
                    }
                }

                // If per-char fallback ended in fallback mode, restore the original font.
                if char_font_is_fallback {
                    if let Some(font_name) = member_plan.font_name.as_deref() {
                        rebuilt_operations.push(font_set_operation(font_name, member_plan.font_size));
                    }
                }

                // Record the new ID pointing to the first Tj for this member.
                let id_index = first_tj_index.unwrap_or(rebuilt_operations.len());
                updated_member_ids.insert(
                    *member_id,
                    TextObjectId(PdfObjectId(encode_text_object_id(
                        edit_group.page.0,
                        id_index as u32,
                    ))),
                );
            } else {
                // Emit `q re W n` immediately before the BT that opens the targeted block.
                if Some(operation_index) == clip_open_before_bt {
                    rebuilt_operations.push(Operation::new("q", vec![]));
                    rebuilt_operations.push(Operation::new(
                        "re",
                        vec![
                            Object::Real(clip_bounds.origin.x),
                            Object::Real(clip_bounds.origin.y),
                            Object::Real(clip_bounds.size.width),
                            Object::Real(clip_bounds.size.height),
                        ],
                    ));
                    rebuilt_operations.push(Operation::new("W", vec![]));
                    rebuilt_operations.push(Operation::new("n", vec![]));
                }
                rebuilt_operations.push(operation);
                // Emit `Q` immediately after the ET that closes the targeted block.
                if Some(operation_index) == clip_close_after_et {
                    rebuilt_operations.push(Operation::new("Q", vec![]));
                }
            }
        }
        content.operations = rebuilt_operations;

        let encoded = content
            .encode()
            .map_err(|err| CoreError::Engine(format!("failed to encode content stream: {err}")))?;
        self.document
            .change_page_content(page_id, encoded)
            .map_err(|err| CoreError::Engine(format!("failed to write page content: {err}")))?;

        self.extract_text_objects()?;
        self.group_text_object(updated_member_ids.get(&id).copied().unwrap_or(id))
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
        prepare_document_for_full_save(&mut document);
        document
            .save(path)
            .map_err(|err| CoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err)))?;
        Ok(())
    }
}

impl LopdfDocument {
    pub fn save_to_bytes(&self) -> CoreResult<Vec<u8>> {
        let mut document = self.document.clone();
        prepare_document_for_full_save(&mut document);
        let mut output = Vec::new();
        document
            .save_to(&mut output)
            .map_err(|err| CoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err)))?;
        Ok(output)
    }

    fn ensure_page_cjk_fallback_font(&mut self, page_id: ObjectId) -> CoreResult<String> {
        const FALLBACK_FONT_NAME: &str = "PdfEditorFallbackCjk";
        let cid_font_id = self.document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "CIDFontType0",
            "BaseFont" => "STSong-Light",
            "DW" => 1000,
            "W" => fallback_cjk_widths(),
            "CIDSystemInfo" => dictionary! {
                "Registry" => Object::string_literal("Adobe"),
                "Ordering" => Object::string_literal("GB1"),
                "Supplement" => 2,
            },
        });
        let cjk_font_id = self.document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type0",
            "BaseFont" => "STSong-Light",
            "Encoding" => "UniGB-UCS2-H",
            "DescendantFonts" => vec![Object::Reference(cid_font_id)],
        });
        let page_dict = self
            .document
            .get_dictionary(page_id)
            .map_err(|err| CoreError::Engine(format!("failed to access page dictionary: {err}")))?;
        let mut resources = page_dict
            .get(b"Resources")
            .ok()
            .and_then(|object| cloned_dictionary_from_object(&self.document, object))
            .unwrap_or_default();
        let mut fonts = resources
            .get(b"Font")
            .ok()
            .and_then(|object| cloned_dictionary_from_object(&self.document, object))
            .unwrap_or_default();
        fonts.set(FALLBACK_FONT_NAME, Object::Reference(cjk_font_id));
        resources.set("Font", Object::Dictionary(fonts));

        let page = self
            .document
            .get_object_mut(page_id)
            .map_err(|err| CoreError::Engine(format!("failed to access page object: {err}")))?;
        let page_dict = page
            .as_dict_mut()
            .map_err(|err| CoreError::InvalidPdf(format!("page object is not a dictionary: {err}")))?;
        page_dict.set("Resources", Object::Dictionary(resources));

        Ok(FALLBACK_FONT_NAME.to_string())
    }

    pub fn page_load_bundle(
        &self,
        page: PageIndex,
        options: BackgroundRenderOptions,
    ) -> CoreResult<PageLoadBundle> {
        let structure = self.page_structure(page)?;
        let background_png = self.background_png_bytes(page, options)?.0;
        let images = self.page_image_png_bytes(page)?;
        let fonts = self.page_font_assets(page)?;
        Ok(PageLoadBundle {
            structure,
            background_png,
            images,
            fonts,
        })
    }

    pub fn hit_test(&self, page: PageIndex, point: Point) -> CoreResult<Option<HitTestResult>> {
        self.ensure_page(page)?;
        let mut text = self.structured_text(page)?;
        text.sort_by_key(|object| object.z_index);
        for object in text.into_iter().rev() {
            if !object.bounds.contains(point) {
                continue;
            }
            return Ok(Some(HitTestResult {
                object_id: object.id.0,
                object_type: "text".to_string(),
                page,
                local_position: inverse_transform_point(object.transform, point),
                text_run_index: Some(0),
                glyph_index: None,
                bbox: object.bounds,
                matrix: object.transform,
            }));
        }

        let mut images = self.structured_images(page)?;
        images.sort_by_key(|object| object.z_index);
        for object in images.into_iter().rev() {
            if !object.bounds.contains(point) {
                continue;
            }
            return Ok(Some(HitTestResult {
                object_id: object.id.0,
                object_type: "image".to_string(),
                page,
                local_position: inverse_transform_point(object.transform, point),
                text_run_index: None,
                glyph_index: None,
                bbox: object.bounds,
                matrix: object.transform,
            }));
        }

        Ok(None)
    }

    pub fn start_text_edit(&self, id: TextObjectId) -> CoreResult<TextEditSessionInfo> {
        let edit_group = self.text_edit_group(id)?;
        let context = self.text_layout_context(id)?;
        let (glyphs, _) = self.layout_preview_glyphs(id, &context.object.content)?;
        Ok(TextEditSessionInfo {
            object_id: id,
            page: context.page,
            original_text: context.object.content,
            group_object_ids: edit_group.member_ids,
            bbox: context.object.clip_bounds.unwrap_or(context.object.bounds),
            matrix: context.object.transform,
            font_id: context.object.font_name,
            font_size: context.object.font_size,
            writing_mode: Some("horizontal".to_string()),
            glyphs,
        })
    }

    pub fn preview_text_layout(
        &self,
        id: TextObjectId,
        text: String,
    ) -> CoreResult<TextLayoutPreview> {
        let edit_group = self.text_edit_group(id)?;
        let context = self.text_layout_context(id)?;
        let (glyphs, bbox) = self.layout_preview_glyphs(id, &text)?;
        let ref_bounds = context.object.clip_bounds.unwrap_or(context.object.bounds);
        let overflow = bbox.size.width > ref_bounds.size.width
            || bbox.size.height > ref_bounds.size.height;

        Ok(TextLayoutPreview {
            object_id: id,
            text,
            group_object_ids: edit_group.member_ids,
            glyphs,
            bbox,
            overflow,
        })
    }

    fn layout_preview_glyphs(&self, id: TextObjectId, text: &str) -> CoreResult<(Vec<LayoutGlyph>, Rect)> {
        let edit_group = self.text_edit_group(id)?;
        if edit_group.member_ids.len() <= 1 {
            let context = self.text_layout_context(id)?;
            let (mut glyphs, width) = layout_glyphs(text, &context);
            let factor = context.tj_compression;
            if factor < 0.999 {
                // Re-position every glyph with the compressed cursor so the preview
                // bbox width and per-glyph highlights match the compressed PDF layout.
                let mut cursor = 0.0f32;
                for glyph in &mut glyphs {
                    let adv = glyph.advance * factor;
                    let (x, y) = transform_point(context.object.transform, cursor, 0.0);
                    let glyph_transform = multiply_matrix(
                        context.object.transform,
                        [1.0, 0.0, 0.0, 1.0, cursor, 0.0],
                    );
                    let bbox = transformed_rect_bounds(glyph_transform, adv.max(0.0), 1.2);
                    glyph.x = x;
                    glyph.y = y;
                    glyph.advance = adv;
                    glyph.width = bbox.size.width;
                    glyph.bbox = bbox;
                    cursor += adv;
                }
                return Ok((
                    glyphs,
                    bounds_for_text_width(width * factor, context.object.transform),
                ));
            }
            return Ok((glyphs, bounds_for_text_width(width, context.object.transform)));
        }

        let member_contexts = edit_group
            .member_ids
            .iter()
            .map(|member_id| self.single_text_layout_context(*member_id))
            .collect::<CoreResult<Vec<_>>>()?;
        let original_text = member_contexts
            .iter()
            .map(|context| context.object.content.as_str())
            .collect::<String>();
        let members = member_contexts
            .iter()
            .map(|context| GroupMemberPlan {
                id: context.object.id,
                original_content: context.object.content.clone(),
                original_char_count: context.object.content.chars().count(),
                font_name: context.object.font_name.clone(),
                font_size: context.object.font_size,
                font_map: context.font_map.clone(),
                metrics: context.metrics.clone(),
                template: Object::String(Vec::new(), StringFormat::Hexadecimal),
            })
            .collect::<Vec<_>>();
        let segments = repartition_group_text(&original_text, text, &members)
            .unwrap_or_else(|_| proportional_split(text, &members));
        let mut glyphs = Vec::new();
        for (context, segment) in member_contexts.iter().zip(segments.iter()) {
            let (mut segment_glyphs, _) = layout_glyphs(segment, context);
            glyphs.append(&mut segment_glyphs);
        }
        let bbox = glyph_bounds(&glyphs).unwrap_or(edit_group.bounds);
        Ok((glyphs, bbox))
    }

    fn extract_text_objects(&mut self) -> CoreResult<()> {
        self.text_objects.clear();
        self.text_refs.clear();
        self.text_edit_groups.clear();

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
                    if text.is_empty() {
                        continue;
                    }
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
            let structured = self.structured_text(page)?;
            self.register_text_edit_groups(page, &structured);
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

    fn text_edit_group(&self, id: TextObjectId) -> CoreResult<TextEditGroup> {
        if let Some(group) = self.text_edit_groups.get(&id) {
            return Ok(group.clone());
        }

        let object = self.find_text_object(id)?;
        let matrix = [
            object.font_size,
            0.0,
            0.0,
            object.font_size,
            object.bounds.origin.x,
            object.bounds.origin.y,
        ];
        Ok(TextEditGroup {
            page: object.page,
            member_ids: vec![id],
            bounds: object.bounds,
            matrix,
            font_name: object.font_name,
            font_size: object.font_size,
        })
    }

    fn group_text_object(&self, id: TextObjectId) -> CoreResult<TextObject> {
        let group = self.text_edit_group(id)?;
        let objects = self
            .text_objects
            .get(&group.page)
            .cloned()
            .unwrap_or_default();
        let members = group
            .member_ids
            .iter()
            .filter_map(|member_id| objects.iter().find(|object| object.id == *member_id).cloned())
            .collect::<Vec<_>>();
        if members.is_empty() {
            return self.find_text_object(id);
        }

        let first = members.first().cloned().unwrap();
        Ok(TextObject {
            id,
            page: group.page,
            bounds: group.bounds,
            content: members
                .iter()
                .map(|member| member.content.as_str())
                .collect::<String>(),
            font_name: group.font_name.clone().or(first.font_name),
            font_size: group.font_size,
            color: first.color,
            runs: vec![TextRun::new(
                members
                    .iter()
                    .map(|member| member.content.as_str())
                    .collect::<String>(),
                group.font_name,
                group.font_size,
                first.color,
            )],
        })
    }

    fn text_layout_context(&self, id: TextObjectId) -> CoreResult<TextLayoutContext> {
        let edit_group = self.text_edit_group(id)?;
        let primary_id = edit_group
            .member_ids
            .first()
            .copied()
            .unwrap_or(id);
        let text_ref = self
            .text_refs
            .get(&primary_id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("text object {}", (primary_id.0).0)))?;
        let structured = self.structured_text(text_ref.page)?;
        let members = edit_group
            .member_ids
            .iter()
            .filter_map(|member_id| structured.iter().find(|object| object.id == *member_id).cloned())
            .collect::<Vec<_>>();
        let object = if members.is_empty() {
            structured
                .into_iter()
                .find(|object| object.id == primary_id)
                .ok_or_else(|| CoreError::NotFound(format!("text object {}", (primary_id.0).0)))?
        } else {
            merge_text_group_objects(&edit_group, &members)
        };
        let page_id = self.page_id(text_ref.page)?;
        let content = self.decoded_page_content(page_id)?;
        let mut page_state = PageParseState::default();
        for (index, operation) in content.operations.iter().enumerate() {
            update_page_state(&mut page_state, operation);
            if index == text_ref.operation_index {
                break;
            }
        }
        let font_maps = self.page_font_maps(page_id);
        let font_metrics = self.page_font_metrics(page_id);
        let font_map = page_state
            .text
            .font_name
            .as_ref()
            .and_then(|name| font_maps.get(name))
            .cloned();
        let metrics = page_state
            .text
            .font_name
            .as_ref()
            .and_then(|name| font_metrics.get(name))
            .cloned();

        let tj_compression = content
            .operations
            .get(text_ref.operation_index)
            .map(|op| {
                tj_compression_factor(op, font_map.as_ref(), metrics.as_ref(), &page_state.text)
            })
            .unwrap_or(1.0);

        Ok(TextLayoutContext {
            page: text_ref.page,
            object,
            state: page_state.text,
            font_map,
            metrics,
            tj_compression,
        })
    }

    fn single_text_layout_context(&self, id: TextObjectId) -> CoreResult<TextLayoutContext> {
        let text_ref = self
            .text_refs
            .get(&id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("text object {}", (id.0).0)))?;
        let structured = self.structured_text(text_ref.page)?;
        let object = structured
            .into_iter()
            .find(|object| object.id == id)
            .ok_or_else(|| CoreError::NotFound(format!("text object {}", (id.0).0)))?;
        let page_id = self.page_id(text_ref.page)?;
        let content = self.decoded_page_content(page_id)?;
        let mut page_state = PageParseState::default();
        for (index, operation) in content.operations.iter().enumerate() {
            update_page_state(&mut page_state, operation);
            if index == text_ref.operation_index {
                break;
            }
        }
        let font_maps = self.page_font_maps(page_id);
        let font_metrics = self.page_font_metrics(page_id);
        let font_map = page_state
            .text
            .font_name
            .as_ref()
            .and_then(|name| font_maps.get(name))
            .cloned();
        let metrics = page_state
            .text
            .font_name
            .as_ref()
            .and_then(|name| font_metrics.get(name))
            .cloned();

        let tj_compression = content
            .operations
            .get(text_ref.operation_index)
            .map(|op| {
                tj_compression_factor(op, font_map.as_ref(), metrics.as_ref(), &page_state.text)
            })
            .unwrap_or(1.0);

        Ok(TextLayoutContext {
            page: text_ref.page,
            object,
            state: page_state.text,
            font_map,
            metrics,
            tj_compression,
        })
    }

    fn register_text_edit_groups(&mut self, page: PageIndex, objects: &[StructuredTextObject]) {
        for group in detect_text_edit_groups(page, objects) {
            for member_id in &group.member_ids {
                self.text_edit_groups.insert(*member_id, group.clone());
            }
        }
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

    fn page_rotation(&self, page_id: ObjectId) -> i32 {
        let rotation = self
            .document
            .get_object(page_id)
            .ok()
            .and_then(|object| object.as_dict().ok())
            .and_then(|page| page.get(b"Rotate").ok())
            .and_then(object_to_i64)
            .unwrap_or(0);
        match rotation.rem_euclid(360) {
            90 => 90,
            180 => 180,
            270 => 270,
            _ => 0,
        }
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

    fn page_font_metrics(&self, page_id: ObjectId) -> HashMap<String, FontMetrics> {
        let Ok(fonts) = self.document.get_page_fonts(page_id) else {
            return HashMap::new();
        };

        let mut metrics = HashMap::new();
        for (name, font) in fonts {
            let to_unicode = parse_font_to_unicode(&self.document, font);
            if let Some(font_metrics) =
                parse_font_metrics(&self.document, font, to_unicode.as_ref())
            {
                metrics.insert(String::from_utf8_lossy(&name).into_owned(), font_metrics);
            }
        }
        metrics
    }

    fn page_font_features(&self, page_id: ObjectId) -> HashMap<String, Vec<String>> {
        let Ok(fonts) = self.document.get_page_fonts(page_id) else {
            return HashMap::new();
        };
        let mut feature_map = HashMap::new();
        for (name, font) in fonts {
            let resource_name = String::from_utf8_lossy(&name).into_owned();
            let Some(descriptor) = font_descriptor(&self.document, font) else {
                continue;
            };
            let Some(bytes) = font_raw_sfnt_bytes(&self.document, descriptor) else {
                continue;
            };
            let features = sfnt_layout_features(&bytes);
            if !features.is_empty() {
                feature_map.insert(resource_name, features);
            }
        }
        feature_map
    }

    pub fn page_font_assets(&self, page: PageIndex) -> CoreResult<Vec<PageFontAsset>> {
        let page_id = self.page_id(page)?;
        let Ok(fonts) = self.document.get_page_fonts(page_id) else {
            return Ok(Vec::new());
        };

        let mut assets = Vec::new();
        for (name, font) in fonts {
            let resource_name = String::from_utf8_lossy(&name).into_owned();
            let to_unicode = parse_font_to_unicode(&self.document, font);
            let Some(descriptor) = font_descriptor(&self.document, font) else {
                continue;
            };
            let Some((bytes, mime_type, format, extension)) =
                font_file_bytes(&self.document, font, descriptor, to_unicode.as_ref())
            else {
                continue;
            };
            let family_name = font_family_name(&self.document, font, &resource_name);
            let font_weight = font_weight(&self.document, font).unwrap_or(400);
            let file_name = format!("{}.{}", sanitize_file_stem(&resource_name), extension);
            assets.push(PageFontAsset {
                resource_name,
                family_name,
                font_weight,
                is_bold: font_weight >= 600,
                file_name,
                mime_type: mime_type.to_string(),
                format: format.to_string(),
                bytes,
            });
        }

        Ok(assets)
    }

    fn structured_text(&self, page: PageIndex) -> CoreResult<Vec<StructuredTextObject>> {
        let page_id = self.page_id(page)?;
        let content = self.decoded_page_content(page_id)?;
        let page_fonts = self.document.get_page_fonts(page_id).ok();
        let font_maps = self.page_font_maps(page_id);
        let font_metrics = self.page_font_metrics(page_id);
        let font_feature_map = self.page_font_features(page_id);
        let mut state = PageParseState::default();
        let mut objects = Vec::new();

        // Track the active clipping rectangle set by `q re W n … Q` sequences so we
        // can expose it as `clip_bounds` on each text object.
        let mut clip_stack: Vec<Option<Rect>> = Vec::new();
        let mut active_clip: Option<Rect> = None;
        let mut pending_re: Option<Rect> = None;
        let mut pending_w_clip: Option<Rect> = None;

        for (operation_index, operation) in content.operations.iter().enumerate() {
            // Update clip-path tracking state before everything else.
            match operation.operator.as_str() {
                "q" => clip_stack.push(active_clip),
                "Q" => {
                    active_clip = clip_stack.pop().unwrap_or(None);
                    pending_re = None;
                    pending_w_clip = None;
                }
                "re" => {
                    if operation.operands.len() >= 4 {
                        let x = object_to_f32(&operation.operands[0]);
                        let y = object_to_f32(&operation.operands[1]);
                        let w = object_to_f32(&operation.operands[2]);
                        let h = object_to_f32(&operation.operands[3]);
                        if let (Some(x), Some(y), Some(w), Some(h)) = (x, y, w, h) {
                            pending_re = Some(Rect::new(x, y, w, h));
                        }
                    }
                }
                "W" | "W*" => {
                    pending_w_clip = pending_re.take();
                }
                "n" | "f" | "f*" | "F" | "S" | "s" | "B" | "B*" | "b" | "b*" => {
                    if let Some(clip) = pending_w_clip.take() {
                        active_clip = Some(clip);
                    }
                }
                _ => {}
            }

            update_page_state(&mut state, operation);
            let font_map = state
                .text
                .font_name
                .as_ref()
                .and_then(|name| font_maps.get(name));
            let metrics = state
                .text
                .font_name
                .as_ref()
                .and_then(|name| font_metrics.get(name));
            let is_background_only_text = state
                .text
                .font_name
                .as_ref()
                .is_some_and(|font_name| {
                    is_type3_font_name(page_fonts.as_ref(), font_name) || !state.text.is_svg_safe()
                });
            if let Some(text) = operation_text(operation, font_map) {
                if text.is_empty() {
                    advance_page_text_state(&mut state, operation, metrics);
                    continue;
                }
                if is_background_only_text {
                    advance_page_text_state(&mut state, operation, metrics);
                    continue;
                }
                let object_id = TextObjectId(PdfObjectId(encode_text_object_id(
                    page.0,
                    operation_index as u32,
                )));
                let transform = text_render_transform(&state);
                let layout_context = TextLayoutContext {
                    page,
                    object: StructuredTextObject {
                        id: object_id,
                        bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
                        content: text.clone(),
                        font_name: state.text.font_name.clone(),
                        font_size: state.text.font_size,
                        color: state.text.color,
                        stroke_color: state.text.stroke_color,
                        stroke_width: state.text.stroke_width,
                        rendering_mode: state.text.rendering_mode,
                        char_spacing: state.text.char_spacing,
                        word_spacing: state.text.word_spacing,
                        horizontal_scaling: state.text.horizontal_scaling,
                        transform,
                        angle_degrees: matrix_angle_degrees(transform),
                        z_index: operation_index,
                        glyphs: Vec::new(),
                        punct_width_squeeze: false,
                        font_features: Vec::new(),
                        clip_bounds: None,
                        runs: Vec::new(),
                    },
                    state: state.text.clone(),
                    font_map: font_map.cloned(),
                    metrics: metrics.cloned(),
                    // Not used for rendering (only for editing), set to 1.0 here.
                    tj_compression: 1.0,
                };
                let (glyphs, width) = layout_glyphs_tj(operation, &layout_context);
                let bounds = operation_text_advance(operation, metrics, &state.text)
                    .or_else(|| (!glyphs.is_empty()).then_some(width))
                    .map(|width| bounds_for_text_width(width, transform))
                    .unwrap_or_else(|| bounds_for_text(&text, state.text.font_size, transform));
                let punct_width_squeeze = match (font_map, metrics) {
                    (Some(fm), Some(m)) => font_has_punct_width_squeeze(m, fm),
                    _ => false,
                };
                let font_features = state
                    .text
                    .font_name
                    .as_ref()
                    .and_then(|name| font_feature_map.get(name))
                    .cloned()
                    .unwrap_or_default();
                objects.push(StructuredTextObject {
                    id: object_id,
                    bounds,
                    content: text.clone(),
                    font_name: state.text.font_name.clone(),
                    font_size: state.text.font_size,
                    color: state.text.color,
                    stroke_color: state.text.stroke_color,
                    stroke_width: state.text.stroke_width,
                    rendering_mode: state.text.rendering_mode,
                    char_spacing: state.text.char_spacing,
                    word_spacing: state.text.word_spacing,
                    horizontal_scaling: state.text.horizontal_scaling,
                    transform,
                    angle_degrees: matrix_angle_degrees(transform),
                    z_index: operation_index,
                    glyphs,
                    punct_width_squeeze,
                    font_features,
                    clip_bounds: active_clip,
                    runs: vec![TextRun::new(
                        text,
                        state.text.font_name.clone(),
                        state.text.font_size,
                        state.text.color,
                    )],
                });
                advance_page_text_state(&mut state, operation, metrics);
            }
        }

        Ok(objects)
    }

    /// Merges scatter-format text groups in a `structured_text` result so that
    /// each logical edit group (adjacent Tj operations for the same run) becomes
    /// a single `StructuredTextObject`.  Objects that do not belong to any group
    /// are emitted unchanged.  The output preserves the original z-order based on
    /// the primary member of each group.
    fn merge_structured_text_groups(
        &self,
        objects: Vec<StructuredTextObject>,
    ) -> Vec<StructuredTextObject> {
        use std::collections::HashSet;
        let id_to_object: HashMap<TextObjectId, &StructuredTextObject> =
            objects.iter().map(|o| (o.id, o)).collect();
        let mut emitted: HashSet<TextObjectId> = HashSet::new();
        let mut merged = Vec::with_capacity(objects.len());

        for object in &objects {
            if emitted.contains(&object.id) {
                continue;
            }
            // Look up the edit group for this object.
            let group = match self.text_edit_groups.get(&object.id) {
                Some(g) => g,
                None => {
                    emitted.insert(object.id);
                    merged.push(object.clone());
                    continue;
                }
            };
            // Collect all group members that are present in `objects`.
            let members: Vec<StructuredTextObject> = group
                .member_ids
                .iter()
                .filter_map(|id| id_to_object.get(id).map(|o| (*o).clone()))
                .collect();
            // Mark all members as emitted so they are not processed again.
            for id in &group.member_ids {
                emitted.insert(*id);
            }
            if members.len() <= 1 {
                // Single-member group — no merging needed.
                if let Some(m) = members.into_iter().next() {
                    merged.push(m);
                } else {
                    merged.push(object.clone());
                }
            } else {
                merged.push(merge_text_group_objects(group, &members));
            }
        }
        merged
    }

    fn structured_visual_text(
        &self,
        page: PageIndex,
    ) -> CoreResult<Vec<StructuredVisualTextObject>> {
        let page_id = self.page_id(page)?;
        let content = self.decoded_page_content(page_id)?;
        let page_fonts = self.document.get_page_fonts(page_id).ok();
        let font_maps = self.page_font_maps(page_id);
        let font_metrics = self.page_font_metrics(page_id);
        let mut state = PageParseState::default();
        let mut objects = Vec::new();

        for (operation_index, operation) in content.operations.iter().enumerate() {
            update_page_state(&mut state, operation);
            let font_map = state
                .text
                .font_name
                .as_ref()
                .and_then(|name| font_maps.get(name));
            let metrics = state
                .text
                .font_name
                .as_ref()
                .and_then(|name| font_metrics.get(name));
            let is_background_only_text = state
                .text
                .font_name
                .as_ref()
                .is_some_and(|font_name| {
                    is_type3_font_name(page_fonts.as_ref(), font_name) || !state.text.is_svg_safe()
                });

            if let Some(_text) = operation_text(operation, font_map) {
                if is_background_only_text {
                    let transform = text_render_transform(&state);
                    if let Some(width) = operation_text_advance(operation, metrics, &state.text) {
                        objects.push(StructuredVisualTextObject {
                            id: TextObjectId(PdfObjectId(encode_text_object_id(
                                page.0,
                                operation_index as u32,
                            ))),
                            bounds: bounds_for_text_width(width, transform),
                            font_name: state.text.font_name.clone(),
                            font_size: state.text.font_size,
                            transform,
                            angle_degrees: matrix_angle_degrees(transform),
                            z_index: operation_index,
                        });
                    }
                }
                advance_page_text_state(&mut state, operation, metrics);
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
        let (png, report) = self.background_png_bytes(page, options)?;
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(output, png)?;
        Ok(report)
    }

    fn background_png_bytes(
        &self,
        page: PageIndex,
        options: BackgroundRenderOptions,
    ) -> CoreResult<(Vec<u8>, BackgroundBitmapReport)> {
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
        let page_fonts = self.document.get_page_fonts(page_id).ok();
        let font_maps = self.page_font_maps(page_id);
        let font_metrics = self.page_font_metrics(page_id);
        let mut state = GraphicsParseState::default();
        let mut page_state = PageParseState::default();
        let mut path = PdfPath::default();
        let mut drawn_operations = 0usize;

        for operation in &content.operations {
            update_page_state(&mut page_state, operation);
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
                "Tj" | "TJ" | "'" | "\"" => {
                    if self.draw_background_only_text(
                        &mut pixmap,
                        page_fonts.as_ref(),
                        &font_maps,
                        &page_state,
                        operation,
                        page_info.size.height,
                        scale,
                    )? {
                        drawn_operations += 1;
                    }
                    let metrics =
                        font_metrics.get(page_state.text.font_name.as_deref().unwrap_or_default());
                    advance_page_text_state(&mut page_state, operation, metrics);
                }
                _ => {}
            }
        }

        let png = pixmap.encode_png().map_err(|error| {
            CoreError::Engine(format!("failed to encode background PNG: {error}"))
        })?;
        Ok((
            png,
            BackgroundBitmapReport {
                width_px,
                height_px,
                drawn_operations,
            },
        ))
    }

    fn draw_background_only_text(
        &self,
        pixmap: &mut Pixmap,
        page_fonts: Option<&BTreeMap<Vec<u8>, &Dictionary>>,
        font_maps: &HashMap<String, ToUnicodeMap>,
        state: &PageParseState,
        operation: &Operation,
        page_height: f32,
        scale: f32,
    ) -> CoreResult<bool> {
        let Some(font_name) = state.text.font_name.as_ref() else {
            return Ok(false);
        };
        let Some(font) = page_fonts.and_then(|fonts| fonts.get(font_name.as_bytes()).copied())
        else {
            return Ok(false);
        };
        if font
            .get(b"Subtype")
            .ok()
            .and_then(object_name_bytes)
            .as_deref()
            != Some("Type3")
        {
            return Ok(false);
        }
        let decoded = font_maps
            .get(font_name)
            .and_then(|font_map| operation_text(operation, Some(font_map)))
            .unwrap_or_default();
        let raw_bytes = if decoded.is_empty() {
            operation_text_bytes(operation).unwrap_or_default()
        } else {
            font_maps
                .get(font_name)
                .and_then(|font_map| font_map.encode(&decoded))
                .unwrap_or_else(|| operation_text_bytes(operation).unwrap_or_default())
        };
        if raw_bytes.is_empty() {
            return Ok(false);
        }

        let Some(type3) = Type3FontRenderInfo::from_font(&self.document, font) else {
            return Ok(false);
        };
        let transform = text_render_transform(state);
        let mut text_cursor = 0.0f32;
        let mut drew = false;
        for byte in raw_bytes {
            let Some(char_proc) = type3.char_proc(byte) else {
                text_cursor += type3.advance(byte);
                continue;
            };
            let glyph_transform = multiply_matrix(
                transform,
                multiply_matrix([1.0, 0.0, 0.0, 1.0, text_cursor, 0.0], type3.font_matrix),
            );
            if draw_type3_char_proc(
                pixmap,
                &char_proc,
                glyph_transform,
                state.text.color,
                page_height,
                scale,
            )? {
                drew = true;
            }
            text_cursor += type3.advance(byte);
        }

        Ok(drew)
    }

    fn write_page_images(
        &self,
        page: PageIndex,
        output_dir: &Path,
    ) -> CoreResult<Vec<PageImageExport>> {
        std::fs::create_dir_all(output_dir)?;
        let images = self.page_image_png_bytes(page)?;
        let mut exported = Vec::new();

        for image in images {
            let output = output_dir.join(&image.file_name);
            if !output.exists() {
                std::fs::write(&output, &image.png)?;
            }

            if !exported
                .iter()
                .any(|item: &PageImageExport| item.id == image.id)
            {
                exported.push(PageImageExport {
                    id: image.id,
                    file_name: image.file_name,
                    width_px: image.width_px,
                    height_px: image.height_px,
                });
            }
        }

        Ok(exported)
    }

    fn page_image_png_bytes(&self, page: PageIndex) -> CoreResult<Vec<PageImageBytesExport>> {
        self.ensure_page(page)?;
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
            if exported
                .iter()
                .any(|item: &PageImageBytesExport| item.id == id)
            {
                continue;
            }
            let mut pixmap = Pixmap::new(image.width, image.height).ok_or_else(|| {
                CoreError::Engine("failed to allocate image export bitmap".to_string())
            })?;
            pixmap.data_mut().copy_from_slice(&image.premultiplied_rgba);
            let png = pixmap.encode_png().map_err(|error| {
                CoreError::Engine(format!("failed to encode image object PNG: {error}"))
            })?;
            exported.push(PageImageBytesExport {
                id,
                file_name: format!("{}.image.png", (id.0).0),
                width_px: image.width,
                height_px: image.height,
                png,
            });
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

#[derive(Debug, Clone)]
struct Type3FontRenderInfo {
    font_matrix: [f32; 6],
    advance_scale: f32,
    widths: HashMap<u8, f32>,
    encoding: HashMap<u8, String>,
    char_procs: HashMap<String, Content>,
}

impl Type3FontRenderInfo {
    fn from_font(document: &Document, font: &Dictionary) -> Option<Self> {
        let font_matrix = font
            .get(b"FontMatrix")
            .ok()
            .and_then(object_matrix)
            .unwrap_or([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
        let widths = type3_widths(font);
        let encoding = type3_encoding(font);
        let char_procs_dict = font.get(b"CharProcs").ok()?.as_dict().ok()?;
        let mut char_procs = HashMap::new();
        for (name, object) in char_procs_dict {
            let stream = stream_from_object(document, object)?;
            let bytes = stream_content_bytes(stream)?;
            let content = Content::decode(&bytes).ok()?;
            char_procs.insert(String::from_utf8_lossy(name).into_owned(), content);
        }
        Some(Self {
            font_matrix,
            advance_scale: font_matrix_advance_scale(font_matrix),
            widths,
            encoding,
            char_procs,
        })
    }

    fn char_proc(&self, code: u8) -> Option<&Content> {
        self.encoding
            .get(&code)
            .and_then(|name| self.char_procs.get(name))
    }

    fn width(&self, code: u8) -> f32 {
        self.widths.get(&code).copied().unwrap_or(1000.0)
    }

    fn advance(&self, code: u8) -> f32 {
        self.width(code) * self.advance_scale
    }
}

fn type3_widths(font: &Dictionary) -> HashMap<u8, f32> {
    let first_char = font
        .get(b"FirstChar")
        .ok()
        .and_then(object_to_i64)
        .unwrap_or(0);
    let mut widths = HashMap::new();
    if let Ok(array) = font.get(b"Widths").and_then(Object::as_array) {
        for (index, object) in array.iter().enumerate() {
            let code = first_char + index as i64;
            if (0..=255).contains(&code) {
                if let Some(width) = object_to_f32(object) {
                    widths.insert(code as u8, width);
                }
            }
        }
    }
    widths
}

fn type3_encoding(font: &Dictionary) -> HashMap<u8, String> {
    let mut encoding = HashMap::new();
    if let Ok(encoding_object) = font.get(b"Encoding") {
        if let Some(dictionary) = dictionary_from_inline_object(encoding_object) {
            if let Some(base) = dictionary
                .get(b"BaseEncoding")
                .ok()
                .and_then(object_name)
                .and_then(|name| standard_encoding_glyph_names(name.as_bytes()))
            {
                encoding.extend(base);
            }
            if let Ok(differences) = dictionary.get(b"Differences").and_then(Object::as_array) {
                let mut code: Option<u8> = None;
                for item in differences {
                    if let Some(next_code) = object_to_i64(item)
                        .and_then(|value| (0..=255).contains(&value).then_some(value as u8))
                    {
                        code = Some(next_code);
                    } else if let (Some(current), Some(name)) = (code, object_name(item)) {
                        encoding.insert(current, name);
                        code = current.checked_add(1);
                    }
                }
            }
        } else if let Some(base_name) = match encoding_object {
            Object::Name(name) => Some(name.as_slice()),
            _ => None,
        } {
            if let Some(base) = standard_encoding_glyph_names(base_name) {
                encoding.extend(base);
            }
        }
    }
    if encoding.is_empty() {
        encoding.extend(standard_encoding_glyph_names(b"StandardEncoding").unwrap_or_default());
    }
    encoding
}

fn standard_encoding_glyph_names(base_encoding: &[u8]) -> Option<HashMap<u8, String>> {
    if base_encoding != b"StandardEncoding" && base_encoding != b"WinAnsiEncoding" {
        return None;
    }
    let names = [
        (0x20, "space"), (0x21, "exclam"), (0x22, "quotedbl"), (0x23, "numbersign"),
        (0x24, "dollar"), (0x25, "percent"), (0x26, "ampersand"), (0x27, "quotesingle"),
        (0x28, "parenleft"), (0x29, "parenright"), (0x2A, "asterisk"), (0x2B, "plus"),
        (0x2C, "comma"), (0x2D, "hyphen"), (0x2E, "period"), (0x2F, "slash"),
        (0x30, "zero"), (0x31, "one"), (0x32, "two"), (0x33, "three"), (0x34, "four"),
        (0x35, "five"), (0x36, "six"), (0x37, "seven"), (0x38, "eight"), (0x39, "nine"),
        (0x3A, "colon"), (0x3B, "semicolon"), (0x3C, "less"), (0x3D, "equal"),
        (0x3E, "greater"), (0x3F, "question"), (0x40, "at"), (0x5B, "bracketleft"),
        (0x5C, "backslash"), (0x5D, "bracketright"), (0x5E, "asciicircum"), (0x5F, "underscore"),
        (0x60, "grave"), (0x7B, "braceleft"), (0x7C, "bar"), (0x7D, "braceright"), (0x7E, "asciitilde"),
    ];
    let mut map = HashMap::new();
    for (code, name) in names {
        map.insert(code, name.to_string());
    }
    for code in b'A'..=b'Z' {
        map.insert(code, char::from(code).to_string());
    }
    for code in b'a'..=b'z' {
        map.insert(code, char::from(code).to_string());
    }
    Some(map)
}

fn glyph_name_to_unicode(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if let Some(hex) = name.strip_prefix("uni").filter(|value| value.len() == 4) {
        let cp = u32::from_str_radix(hex, 16).ok()?;
        return char::from_u32(cp).map(|ch| ch.to_string());
    }
    if let Some(hex) = name.strip_prefix('u').filter(|value| value.len() == 4 || value.len() == 5 || value.len() == 6) {
        let cp = u32::from_str_radix(hex, 16).ok()?;
        return char::from_u32(cp).map(|ch| ch.to_string());
    }
    if name.chars().count() == 1 {
        return Some(name.to_string());
    }
    let ch = match name {
        "space" => ' ',
        "exclam" => '!',
        "quotedbl" => '"',
        "numbersign" => '#',
        "dollar" => '$',
        "percent" => '%',
        "ampersand" => '&',
        "quotesingle" => '\'',
        "parenleft" => '(',
        "parenright" => ')',
        "asterisk" => '*',
        "plus" => '+',
        "comma" => ',',
        "hyphen" => '-',
        "period" => '.',
        "slash" => '/',
        "colon" => ':',
        "semicolon" => ';',
        "less" => '<',
        "equal" => '=',
        "greater" => '>',
        "question" => '?',
        "at" => '@',
        "bracketleft" => '[',
        "backslash" => '\\',
        "bracketright" => ']',
        "asciicircum" => '^',
        "underscore" => '_',
        "grave" => '`',
        "braceleft" => '{',
        "bar" => '|',
        "braceright" => '}',
        "asciitilde" => '~',
        "zero" => '0',
        "one" => '1',
        "two" => '2',
        "three" => '3',
        "four" => '4',
        "five" => '5',
        "six" => '6',
        "seven" => '7',
        "eight" => '8',
        "nine" => '9',
        _ => return None,
    };
    Some(ch.to_string())
}

fn draw_type3_char_proc(
    pixmap: &mut Pixmap,
    content: &Content,
    glyph_transform: [f32; 6],
    color: Color,
    page_height: f32,
    scale: f32,
) -> CoreResult<bool> {
    let mut path = PdfPath::default();
    let mut state = Type3GraphicsState::new(glyph_transform);
    let mut stack = Vec::new();
    let mut drew = false;
    for operation in &content.operations {
        match operation.operator.as_str() {
            "q" => stack.push(state),
            "Q" => {
                if let Some(previous) = stack.pop() {
                    state = previous;
                }
            }
            "cm" => {
                if let Some(matrix) = operation_matrix(operation) {
                    state.transform = multiply_matrix(state.transform, matrix);
                }
            }
            "w" => {
                if let Some(width) = operation.operands.first().and_then(object_to_f32) {
                    state.line_width = width.max(0.0);
                }
            }
            "m" => {
                if let Some((x, y)) = type3_operation_point(operation, 0, 2) {
                    let point = transform_point(state.transform, x, y);
                    path.move_to(point.0, point.1);
                }
            }
            "l" => {
                if let Some((x, y)) = type3_operation_point(operation, 0, 2) {
                    let point = transform_point(state.transform, x, y);
                    path.line_to(point.0, point.1);
                }
            }
            "c" => {
                if let (Some(p1), Some(p2), Some(p3)) = (
                    type3_operation_point(operation, 0, 6),
                    type3_operation_point(operation, 2, 6),
                    type3_operation_point(operation, 4, 6),
                ) {
                    path.curve_to(
                        transform_point(state.transform, p1.0, p1.1),
                        transform_point(state.transform, p2.0, p2.1),
                        transform_point(state.transform, p3.0, p3.1),
                    );
                }
            }
            "v" => {
                if let (Some(p2), Some(p3)) = (
                    type3_operation_point(operation, 0, 4),
                    type3_operation_point(operation, 2, 4),
                ) {
                    let current = path.current_point().unwrap_or((0.0, 0.0));
                    path.curve_to(
                        current,
                        transform_point(state.transform, p2.0, p2.1),
                        transform_point(state.transform, p3.0, p3.1),
                    );
                }
            }
            "y" => {
                if let (Some(p1), Some(p3)) = (
                    type3_operation_point(operation, 0, 4),
                    type3_operation_point(operation, 2, 4),
                ) {
                    let p1 = transform_point(state.transform, p1.0, p1.1);
                    let p3 = transform_point(state.transform, p3.0, p3.1);
                    path.curve_to(p1, p3, p3);
                }
            }
            "re" => {
                if let Some([x, y, w, h]) = type3_operation_operands::<4>(operation) {
                    let p0 = transform_point(state.transform, x, y);
                    let p1 = transform_point(state.transform, x + w, y);
                    let p2 = transform_point(state.transform, x + w, y + h);
                    let p3 = transform_point(state.transform, x, y + h);
                    path.move_to(p0.0, p0.1);
                    path.line_to(p1.0, p1.1);
                    path.line_to(p2.0, p2.1);
                    path.line_to(p3.0, p3.1);
                    path.close();
                }
            }
            "h" => path.close(),
            "n" => path.clear(),
            "f" | "F" | "f*" | "S" | "s" | "B" | "B*" | "b" | "b*" | "d0" | "d1" => {
                if matches!(operation.operator.as_str(), "s" | "b" | "b*") {
                    path.close();
                }
                if matches!(
                    operation.operator.as_str(),
                    "f" | "F" | "f*" | "B" | "B*" | "b" | "b*"
                ) && fill_type3_path(pixmap, &path, color, page_height, scale)
                {
                    drew = true;
                }
                if matches!(
                    operation.operator.as_str(),
                    "S" | "s" | "B" | "B*" | "b" | "b*"
                ) && stroke_type3_path(pixmap, &path, &state, color, page_height, scale)
                {
                    drew = true;
                }
                if matches!(
                    operation.operator.as_str(),
                    "f" | "F" | "f*" | "S" | "s" | "B" | "B*" | "b" | "b*"
                ) {
                    path.clear();
                }
            }
            _ => {}
        }
    }
    Ok(drew)
}

#[derive(Debug, Clone, Copy)]
struct Type3GraphicsState {
    transform: [f32; 6],
    line_width: f32,
}

impl Type3GraphicsState {
    fn new(transform: [f32; 6]) -> Self {
        Self {
            transform,
            line_width: 1.0,
        }
    }
}

fn type3_operation_point(
    operation: &Operation,
    offset: usize,
    expected_operands: usize,
) -> Option<(f32, f32)> {
    let operands = type3_operation_operand_slice(operation, expected_operands)?;
    Some((
        operands.get(offset).and_then(object_to_f32)?,
        operands.get(offset + 1).and_then(object_to_f32)?,
    ))
}

fn type3_operation_operands<const N: usize>(operation: &Operation) -> Option<[f32; N]> {
    let operands = type3_operation_operand_slice(operation, N)?;
    let mut values = [0.0; N];
    for (index, value) in values.iter_mut().enumerate() {
        *value = operands.get(index).and_then(object_to_f32)?;
    }
    Some(values)
}

fn type3_operation_operand_slice(
    operation: &Operation,
    expected_operands: usize,
) -> Option<&[Object]> {
    if operation.operands.len() < expected_operands {
        return None;
    }
    let start = operation.operands.len() - expected_operands;
    Some(&operation.operands[start..])
}

fn fill_type3_path(
    pixmap: &mut Pixmap,
    path: &PdfPath,
    color: Color,
    page_height: f32,
    scale: f32,
) -> bool {
    let Some(path) = path.to_skia_path(page_height, scale) else {
        return false;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    true
}

fn stroke_type3_path(
    pixmap: &mut Pixmap,
    path: &PdfPath,
    state: &Type3GraphicsState,
    color: Color,
    page_height: f32,
    scale: f32,
) -> bool {
    let Some(path) = path.to_skia_path(page_height, scale) else {
        return false;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    let stroke = Stroke {
        width: type3_stroke_width(state, scale),
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    true
}

fn type3_stroke_width(state: &Type3GraphicsState, scale: f32) -> f32 {
    let x_scale = state.transform[0].hypot(state.transform[1]);
    let y_scale = state.transform[2].hypot(state.transform[3]);
    (state.line_width * x_scale.max(y_scale) * scale).max(0.5)
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
    char_spacing: f32,
    word_spacing: f32,
    horizontal_scaling: f32,
    text_leading: f32,
    color: Color,
    stroke_color: Color,
    stroke_width: f32,
    rendering_mode: i32,
    fill_color_svg_safe: bool,
    stroke_color_svg_safe: bool,
}

#[derive(Debug, Clone)]
struct PageParseState {
    ctm: [f32; 6],
    stack: Vec<([f32; 6], TextParseState, [f32; 6], [f32; 6])>,
    text: TextParseState,
    text_matrix: [f32; 6],
    text_line_matrix: [f32; 6],
}

#[derive(Debug, Clone)]
struct TextLayoutContext {
    page: PageIndex,
    object: StructuredTextObject,
    state: TextParseState,
    font_map: Option<ToUnicodeMap>,
    metrics: Option<FontMetrics>,
    /// Spacing compression factor derived from TJ displacement values (1.0 = no compression).
    /// A value < 1.0 means the original PDF placed characters closer together than pure
    /// font-metric advances would, via positive numeric items in a TJ array.
    tj_compression: f32,
}

impl Default for PageParseState {
    fn default() -> Self {
        Self {
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            stack: Vec::new(),
            text: TextParseState::default(),
            text_matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            text_line_matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
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
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scaling: 100.0,
            text_leading: 0.0,
            color: Color::BLACK,
            stroke_color: Color::BLACK,
            stroke_width: 1.0,
            rendering_mode: 0,
            fill_color_svg_safe: true,
            stroke_color_svg_safe: true,
        }
    }
}

impl TextParseState {
    fn is_svg_safe(&self) -> bool {
        matches!(self.rendering_mode, 0 | 1 | 2)
            && self.fill_color_svg_safe
            && self.stroke_color_svg_safe
    }
}

fn update_page_state(state: &mut PageParseState, operation: &Operation) {
    match operation.operator.as_str() {
        "q" => state.stack.push((
            state.ctm,
            state.text.clone(),
            state.text_matrix,
            state.text_line_matrix,
        )),
        "Q" => {
            if let Some((ctm, text, text_matrix, text_line_matrix)) = state.stack.pop() {
                state.ctm = ctm;
                state.text = text;
                state.text_matrix = text_matrix;
                state.text_line_matrix = text_line_matrix;
            }
        }
        "cm" => {
            if let Some(matrix) = operation_matrix(operation) {
                state.ctm = multiply_matrix(state.ctm, matrix);
            }
        }
        "BT" => {
            state.text_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
            state.text_line_matrix = state.text_matrix;
        }
        "Tf" => {
            if let Some(name) = operation.operands.first().and_then(object_name) {
                state.text.font_name = Some(name);
            }
            if let Some(size) = operation.operands.get(1).and_then(object_to_f32) {
                state.text.font_size = size;
            }
        }
        "Tc" => {
            if let Some(spacing) = operation.operands.first().and_then(object_to_f32) {
                state.text.char_spacing = spacing;
            }
        }
        "Tw" => {
            if let Some(spacing) = operation.operands.first().and_then(object_to_f32) {
                state.text.word_spacing = spacing;
            }
        }
        "Tz" => {
            if let Some(scaling) = operation.operands.first().and_then(object_to_f32) {
                state.text.horizontal_scaling = scaling;
            }
        }
        "TL" => {
            if let Some(leading) = operation.operands.first().and_then(object_to_f32) {
                state.text.text_leading = leading;
            }
        }
        "Td" => {
            if let (Some(x), Some(y)) = (
                operation.operands.first().and_then(object_to_f32),
                operation.operands.get(1).and_then(object_to_f32),
            ) {
                let translate = [1.0, 0.0, 0.0, 1.0, x, y];
                state.text_line_matrix = multiply_matrix(state.text_line_matrix, translate);
                state.text_matrix = state.text_line_matrix;
                state.text.x = state.text_matrix[4];
                state.text.y = state.text_matrix[5];
            }
        }
        "TD" => {
            // TD tx ty: equivalent to  -ty TL  tx ty Td  (also sets text leading to -ty)
            if let (Some(x), Some(y)) = (
                operation.operands.first().and_then(object_to_f32),
                operation.operands.get(1).and_then(object_to_f32),
            ) {
                state.text.text_leading = -y;
                let translate = [1.0, 0.0, 0.0, 1.0, x, y];
                state.text_line_matrix = multiply_matrix(state.text_line_matrix, translate);
                state.text_matrix = state.text_line_matrix;
                state.text.x = state.text_matrix[4];
                state.text.y = state.text_matrix[5];
            }
        }
        // T*: move to start of next line, equivalent to  0 -TL Td
        "T*" => {
            let leading = state.text.text_leading;
            let translate = [1.0, 0.0, 0.0, 1.0, 0.0, -leading];
            state.text_line_matrix = multiply_matrix(state.text_line_matrix, translate);
            state.text_matrix = state.text_line_matrix;
            state.text.x = state.text_matrix[4];
            state.text.y = state.text_matrix[5];
        }
        // ' operator: T* then show string (advance handled by advance_page_text_state)
        "'" => {
            let leading = state.text.text_leading;
            let translate = [1.0, 0.0, 0.0, 1.0, 0.0, -leading];
            state.text_line_matrix = multiply_matrix(state.text_line_matrix, translate);
            state.text_matrix = state.text_line_matrix;
            state.text.x = state.text_matrix[4];
            state.text.y = state.text_matrix[5];
        }
        // " operator: set word/char spacing, then T*, then show string
        "\"" => {
            if let Some(word_spacing) = operation.operands.first().and_then(object_to_f32) {
                state.text.word_spacing = word_spacing;
            }
            if let Some(char_spacing) = operation.operands.get(1).and_then(object_to_f32) {
                state.text.char_spacing = char_spacing;
            }
            let leading = state.text.text_leading;
            let translate = [1.0, 0.0, 0.0, 1.0, 0.0, -leading];
            state.text_line_matrix = multiply_matrix(state.text_line_matrix, translate);
            state.text_matrix = state.text_line_matrix;
            state.text.x = state.text_matrix[4];
            state.text.y = state.text_matrix[5];
        }
        "Tm" => {
            if let Some(matrix) = operation_matrix(operation) {
                state.text_matrix = matrix;
                state.text_line_matrix = matrix;
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
                state.text.fill_color_svg_safe = true;
            }
        }
        "g" => {
            if let Some(color) = gray_color(operation) {
                state.text.color = color;
                state.text.fill_color_svg_safe = true;
            }
        }
        "RG" => {
            if let (Some(r), Some(g), Some(b)) = (
                operation.operands.first().and_then(object_to_f32),
                operation.operands.get(1).and_then(object_to_f32),
                operation.operands.get(2).and_then(object_to_f32),
            ) {
                state.text.stroke_color = Color::rgba(
                    normalized_color_channel(r),
                    normalized_color_channel(g),
                    normalized_color_channel(b),
                    255,
                );
                state.text.stroke_color_svg_safe = true;
            }
        }
        "G" => {
            if let Some(color) = gray_color(operation) {
                state.text.stroke_color = color;
                state.text.stroke_color_svg_safe = true;
            }
        }
        "k" | "sc" | "scn" | "cs" => {
            state.text.fill_color_svg_safe = false;
        }
        "K" | "SC" | "SCN" | "CS" => {
            state.text.stroke_color_svg_safe = false;
        }
        "w" => {
            if let Some(width) = operation.operands.first().and_then(object_to_f32) {
                state.text.stroke_width = width.max(0.0);
            }
        }
        "Tr" => {
            if let Some(mode) = operation.operands.first().and_then(object_to_i64) {
                state.text.rendering_mode = mode.clamp(0, 7) as i32;
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
        "Tc" => {
            if let Some(spacing) = operation.operands.first().and_then(object_to_f32) {
                state.char_spacing = spacing;
            }
        }
        "Tw" => {
            if let Some(spacing) = operation.operands.first().and_then(object_to_f32) {
                state.word_spacing = spacing;
            }
        }
        "Tz" => {
            if let Some(scaling) = operation.operands.first().and_then(object_to_f32) {
                state.horizontal_scaling = scaling;
            }
        }
        "TL" => {
            if let Some(leading) = operation.operands.first().and_then(object_to_f32) {
                state.text_leading = leading;
            }
        }
        "Td" => {
            if let (Some(x), Some(y)) = (
                operation.operands.first().and_then(object_to_f32),
                operation.operands.get(1).and_then(object_to_f32),
            ) {
                state.x += x;
                state.y += y;
            }
        }
        "TD" => {
            // TD tx ty: equivalent to -ty TL; tx ty Td
            if let (Some(x), Some(y)) = (
                operation.operands.first().and_then(object_to_f32),
                operation.operands.get(1).and_then(object_to_f32),
            ) {
                state.text_leading = -y;
                state.x += x;
                state.y += y;
            }
        }
        // T*: move to start of next line, equivalent to 0 -TL Td
        "T*" => {
            state.y -= state.text_leading;
        }
        // ' operator: T* then show string
        "'" => {
            state.y -= state.text_leading;
        }
        // " operator: set word/char spacing, then T*, then show string
        "\"" => {
            if let Some(word_spacing) = operation.operands.first().and_then(object_to_f32) {
                state.word_spacing = word_spacing;
            }
            if let Some(char_spacing) = operation.operands.get(1).and_then(object_to_f32) {
                state.char_spacing = char_spacing;
            }
            state.y -= state.text_leading;
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
        "g" => {
            if let Some(color) = gray_color(operation) {
                state.color = color;
            }
        }
        "RG" => {
            if let (Some(r), Some(g), Some(b)) = (
                operation.operands.first().and_then(object_to_f32),
                operation.operands.get(1).and_then(object_to_f32),
                operation.operands.get(2).and_then(object_to_f32),
            ) {
                state.stroke_color = Color::rgba(
                    normalized_color_channel(r),
                    normalized_color_channel(g),
                    normalized_color_channel(b),
                    255,
                );
            }
        }
        "G" => {
            if let Some(color) = gray_color(operation) {
                state.stroke_color = color;
            }
        }
        "w" => {
            if let Some(width) = operation.operands.first().and_then(object_to_f32) {
                state.stroke_width = width.max(0.0);
            }
        }
        "Tr" => {
            if let Some(mode) = operation.operands.first().and_then(object_to_i64) {
                state.rendering_mode = mode.clamp(0, 7) as i32;
            }
        }
        _ => {}
    }
}

fn advance_page_text_state(
    state: &mut PageParseState,
    operation: &Operation,
    metrics: Option<&FontMetrics>,
) {
    let Some(advance) = operation_text_advance(operation, metrics, &state.text) else {
        return;
    };
    let user_advance = advance * state.text.font_size.abs().max(1.0);
    state.text_matrix = multiply_matrix(state.text_matrix, [1.0, 0.0, 0.0, 1.0, user_advance, 0.0]);
    state.text.x = state.text_matrix[4];
    state.text.y = state.text_matrix[5];
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

fn object_matrix(object: &Object) -> Option<[f32; 6]> {
    let array = object.as_array().ok()?;
    Some([
        array.first().and_then(object_to_f32)?,
        array.get(1).and_then(object_to_f32)?,
        array.get(2).and_then(object_to_f32)?,
        array.get(3).and_then(object_to_f32)?,
        array.get(4).and_then(object_to_f32)?,
        array.get(5).and_then(object_to_f32)?,
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
    bounds_for_text_width(width, transform)
}

fn bounds_for_text_width(width: f32, transform: [f32; 6]) -> Rect {
    transformed_rect_bounds(transform, width.max(0.0), 1.2)
}

fn merge_text_group_objects(
    group: &TextEditGroup,
    members: &[StructuredTextObject],
) -> StructuredTextObject {
    let primary = members.first().cloned().unwrap_or(StructuredTextObject {
        id: group.member_ids[0],
        bounds: group.bounds,
        content: String::new(),
        font_name: group.font_name.clone(),
        font_size: group.font_size,
        color: Color::BLACK,
        stroke_color: Color::BLACK,
        stroke_width: 0.0,
        rendering_mode: 0,
        char_spacing: 0.0,
        word_spacing: 0.0,
        horizontal_scaling: 100.0,
        transform: group.matrix,
        angle_degrees: matrix_angle_degrees(group.matrix),
        z_index: 0,
        glyphs: Vec::new(),
        punct_width_squeeze: false,
        font_features: Vec::new(),
        clip_bounds: None,
        runs: Vec::new(),
    });
    let content = members
        .iter()
        .map(|member| member.content.as_str())
        .collect::<String>();
    let mut glyphs = members
        .iter()
        .flat_map(|member| member.glyphs.iter().cloned())
        .collect::<Vec<_>>();
    fix_scatter_glyph_advances(&mut glyphs, group.font_size);
    let bounds = glyph_bounds(&glyphs).unwrap_or(group.bounds);
    StructuredTextObject {
        id: primary.id,
        bounds,
        content: content.clone(),
        font_name: group.font_name.clone().or(primary.font_name),
        font_size: group.font_size,
        color: primary.color,
        stroke_color: primary.stroke_color,
        stroke_width: primary.stroke_width,
        rendering_mode: primary.rendering_mode,
        char_spacing: primary.char_spacing,
        word_spacing: primary.word_spacing,
        horizontal_scaling: primary.horizontal_scaling,
        transform: group.matrix,
        angle_degrees: matrix_angle_degrees(group.matrix),
        z_index: primary.z_index,
        glyphs,
        punct_width_squeeze: members.iter().any(|m| m.punct_width_squeeze),
        font_features: {
            let mut set = std::collections::BTreeSet::new();
            for member in members {
                for f in &member.font_features {
                    set.insert(f.clone());
                }
            }
            set.into_iter().collect()
        },
        // All members of a group share the same surrounding clip (if any); take it from
        // the primary member.
        clip_bounds: primary.clip_bounds,
        runs: members
            .iter()
            .flat_map(|member| member.runs.iter().cloned())
            .collect(),
    }
}

fn detect_text_edit_groups(page: PageIndex, objects: &[StructuredTextObject]) -> Vec<TextEditGroup> {
    let mut sorted = objects
        .iter()
        .filter(|object| object.angle_degrees.abs() < 1.0)
        .cloned()
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        left.bounds
            .origin
            .y
            .partial_cmp(&right.bounds.origin.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                left.bounds
                    .origin
                    .x
                    .partial_cmp(&right.bounds.origin.x)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut groups = Vec::new();
    let mut current = Vec::<StructuredTextObject>::new();
    let mut baseline_gap: Option<f32> = None;

    for object in sorted {
        let Some(previous) = current.last() else {
            current.push(object);
            continue;
        };

        if can_join_text_edit_group(previous, &object, baseline_gap) {
            let gap = text_group_gap(previous, &object);
            baseline_gap = Some(baseline_gap.unwrap_or(gap));
            current.push(object);
            continue;
        }

        if current.len() > 1 {
            groups.push(build_text_edit_group(page, &current));
        }
        current = vec![object];
        baseline_gap = None;
    }

    if current.len() > 1 {
        groups.push(build_text_edit_group(page, &current));
    }

    groups
}

fn build_text_edit_group(page: PageIndex, members: &[StructuredTextObject]) -> TextEditGroup {
    let first = &members[0];
    let left = members
        .iter()
        .map(|member| member.bounds.origin.x)
        .fold(f32::INFINITY, f32::min);
    let bottom = members
        .iter()
        .map(|member| member.bounds.origin.y)
        .fold(f32::INFINITY, f32::min);
    let right = members
        .iter()
        .map(|member| member.bounds.origin.x + member.bounds.size.width)
        .fold(f32::NEG_INFINITY, f32::max);
    let top = members
        .iter()
        .map(|member| member.bounds.origin.y + member.bounds.size.height)
        .fold(f32::NEG_INFINITY, f32::max);
    let font_name = members
        .iter()
        .find_map(|member| member.font_name.clone())
        .or_else(|| first.font_name.clone());
    let font_size = members
        .iter()
        .map(|member| member.font_size)
        .fold(first.font_size, f32::max);

    TextEditGroup {
        page,
        member_ids: members.iter().map(|member| member.id).collect(),
        bounds: Rect::new(left, bottom, (right - left).max(0.0), (top - bottom).max(0.0)),
        matrix: first.transform,
        font_name,
        font_size,
    }
}

fn can_join_text_edit_group(
    previous: &StructuredTextObject,
    next: &StructuredTextObject,
    baseline_gap: Option<f32>,
) -> bool {
    let y_tolerance = previous.font_size.max(next.font_size) * 0.35 + 1.0;
    if (previous.transform[5] - next.transform[5]).abs() > y_tolerance {
        return false;
    }
    if (previous.font_size - next.font_size).abs() > 1.0 {
        return false;
    }
    let gap = text_group_gap(previous, next);
    let max_overlap = previous.font_size.max(next.font_size) * 0.35 + 1.0;
    if gap < -max_overlap {
        return false;
    }
    let max_gap = previous.font_size.max(next.font_size) * 2.0;
    if gap > max_gap {
        return false;
    }
    if let Some(expected_gap) = baseline_gap {
        let tolerance = previous.font_size.max(next.font_size) * 0.4 + 1.0;
        return (gap - expected_gap).abs() <= tolerance;
    }
    true
}

fn text_group_gap(previous: &StructuredTextObject, next: &StructuredTextObject) -> f32 {
    next.bounds.origin.x - (previous.bounds.origin.x + previous.bounds.size.width)
}

fn layout_glyphs(text: &str, context: &TextLayoutContext) -> (Vec<LayoutGlyph>, f32) {
    let chars = text.chars().collect::<Vec<_>>();
    let mut glyphs = Vec::with_capacity(chars.len());
    let mut cursor = 0.0f32;

    for (index, ch) in chars.iter().copied().enumerate() {
        let encoded = context
            .font_map
            .as_ref()
            .and_then(|font_map| font_map.encode(&ch.to_string()));
        let glyph_id = encoded.as_deref().and_then(|bytes| {
            context
                .metrics
                .as_ref()
                .and_then(|metrics| metrics.codes(bytes).first().copied())
        });
        let mut advance = encoded
            .as_deref()
            .and_then(|bytes| {
                context
                    .metrics
                    .as_ref()
                    .map(|metrics| metrics.text_advance(bytes, &context.state))
            })
            // Fall back to per-character Unicode heuristics rather than a flat 0.6×
            // estimate: CJK characters return 1.0 (full-width), ASCII returns
            // proportional values, and other scripts get 0.6.
            .unwrap_or_else(|| fallback_char_advance(ch));

        if index + 1 < chars.len() {
            advance += (context.state.char_spacing / context.object.font_size.max(1.0))
                * (context.state.horizontal_scaling / 100.0);
        }

        let (x, y) = transform_point(context.object.transform, cursor, 0.0);
        let glyph_transform =
            multiply_matrix(context.object.transform, [1.0, 0.0, 0.0, 1.0, cursor, 0.0]);
        let bbox = transformed_rect_bounds(glyph_transform, advance.max(0.0), 1.2);
        glyphs.push(LayoutGlyph {
            ch: ch.to_string(),
            glyph_id,
            font_name: context.object.font_name.clone(),
            x,
            y,
            advance,
            width: bbox.size.width,
            bbox,
        });
        cursor += advance;
    }

    (glyphs, cursor)
}

/// TJ-aware variant of [`layout_glyphs`].
///
/// For `Tj`/`'`/`"` operations the result is identical to `layout_glyphs`.
/// For `TJ` operations the function walks the operand array and applies the
/// numeric displacement elements to the internal cursor between string chunks,
/// so each glyph's `x` reflects the actual PDF rendering position.
fn layout_glyphs_tj(operation: &Operation, context: &TextLayoutContext) -> (Vec<LayoutGlyph>, f32) {
    if operation.operator.as_str() != "TJ" {
        let text = match operation.operator.as_str() {
            "Tj" | "'" | "\"" => operation
                .operands
                .last()
                .and_then(|o| object_text(o, context.font_map.as_ref()))
                .unwrap_or_default(),
            _ => return (Vec::new(), 0.0),
        };
        return layout_glyphs(&text, context);
    }

    let Some(array) = operation
        .operands
        .first()
        .and_then(|o| o.as_array().ok())
    else {
        return (Vec::new(), 0.0);
    };

    // Pre-count total characters so we know which glyph is last (char_spacing is
    // skipped on the last glyph, matching the behaviour of layout_glyphs).
    let total_chars: usize = array
        .iter()
        .filter_map(|item| object_text(item, context.font_map.as_ref()))
        .map(|s| s.chars().count())
        .sum();

    if total_chars == 0 {
        return (Vec::new(), 0.0);
    }

    let mut glyphs = Vec::with_capacity(total_chars);
    let mut cursor = 0.0f32;
    let mut chars_placed = 0usize;

    for item in array {
        if let Some(text) = object_text(item, context.font_map.as_ref()) {
            for ch in text.chars() {
                chars_placed += 1;
                let is_last = chars_placed == total_chars;

                let encoded = context
                    .font_map
                    .as_ref()
                    .and_then(|fm| fm.encode(&ch.to_string()));
                let glyph_id = encoded.as_deref().and_then(|bytes| {
                    context
                        .metrics
                        .as_ref()
                        .and_then(|m| m.codes(bytes).first().copied())
                });
                // text_advance for a single glyph does NOT include char_spacing
                // (char_gaps = codes.len()-1 = 0).  We add it manually below.
                let mut advance = encoded
                    .as_deref()
                    .and_then(|bytes| {
                        context
                            .metrics
                            .as_ref()
                            .map(|m| m.text_advance(bytes, &context.state))
                    })
                    .unwrap_or_else(|| fallback_char_advance(ch));

                if !is_last {
                    advance += (context.state.char_spacing / context.object.font_size.max(1.0))
                        * (context.state.horizontal_scaling / 100.0);
                }

                let (x, y) = transform_point(context.object.transform, cursor, 0.0);
                let glyph_transform =
                    multiply_matrix(context.object.transform, [1.0, 0.0, 0.0, 1.0, cursor, 0.0]);
                let bbox = transformed_rect_bounds(glyph_transform, advance.max(0.0), 1.2);
                glyphs.push(LayoutGlyph {
                    ch: ch.to_string(),
                    glyph_id,
                    font_name: context.object.font_name.clone(),
                    x,
                    y,
                    advance,
                    width: bbox.size.width,
                    bbox,
                });
                cursor += advance;
            }
        } else if let Some(wi) = object_to_f32(item) {
            // Positive wi = tighten spacing (move cursor left).
            // The displacement is in thousandths of a text-space unit.
            // Multiply by horizontal_scaling to stay consistent with how
            // character advances are normalised in layout_glyphs / text_advance.
            cursor -= (wi / 1000.0) * (context.state.horizontal_scaling / 100.0);
        }
    }

    (glyphs, cursor)
}

/// Returns the ratio of actual advance to pure font-metric advance for a TJ operation.
///
/// The ratio is < 1.0 when the array contains positive displacement values that tighten
/// spacing (e.g. `[(char) 500 (char)]` makes the second character 0.5 em closer).
/// Returns 1.0 for non-TJ operations, empty arrays, or when there is no net compression.
fn tj_compression_factor(
    operation: &Operation,
    font_map: Option<&ToUnicodeMap>,
    metrics: Option<&FontMetrics>,
    state: &TextParseState,
) -> f32 {
    if operation.operator.as_str() != "TJ" {
        return 1.0;
    }
    let Some(array) = operation.operands.first().and_then(|o| o.as_array().ok()) else {
        return 1.0;
    };

    let mut font_metric_advance = 0.0f32;
    let mut total_positive_adj = 0.0f32;

    for item in array {
        if let Some(text) = object_text(item, font_map) {
            for ch in text.chars() {
                let encoded = font_map.and_then(|fm| fm.encode(&ch.to_string()));
                let adv = encoded
                    .as_deref()
                    .and_then(|bytes| metrics.map(|m| m.text_advance(bytes, state)))
                    .unwrap_or_else(|| fallback_char_advance(ch));
                font_metric_advance += adv;
            }
        } else if let Some(wi) = object_to_f32(item) {
            if wi > 0.0 {
                // Positive TJ value = tighten spacing; scale by Th so the units match
                // the per-character advances returned by text_advance.
                total_positive_adj += wi / 1000.0 * (state.horizontal_scaling / 100.0);
            }
        }
    }

    if font_metric_advance < 0.01 || total_positive_adj < 0.001 {
        return 1.0;
    }

    ((font_metric_advance - total_positive_adj) / font_metric_advance).max(0.1)
}

fn unit_bounds_after_transform(transform: [f32; 6]) -> Rect {
    transformed_rect_bounds(transform, 1.0, 1.0)
}

/// After merging glyphs from scatter-format (individual Tm+Tj) members the per-glyph
/// `advance` only reflects the raw font-metric step for that glyph, not the actual
/// cursor distance to the next character.  Reconstruct advances from the real
/// page-space x-position differences so that `advance[i] * font_size ≈ x[i+1] - x[i]`.
/// The last glyph keeps its original advance (no following position to reference).
/// `width` is left unchanged — it represents the rendered glyph ink width, which is
/// a different concept from the cursor step.
fn fix_scatter_glyph_advances(glyphs: &mut [LayoutGlyph], font_size: f32) {
    let scale = font_size.abs().max(1.0);
    for i in 0..glyphs.len().saturating_sub(1) {
        let delta_x = glyphs[i + 1].x - glyphs[i].x;
        if delta_x > 0.0 {
            glyphs[i].advance = delta_x / scale;
        }
    }
}

fn glyph_bounds(glyphs: &[LayoutGlyph]) -> Option<Rect> {
    let mut iter = glyphs.iter();
    let first = iter.next()?;
    let mut min_x = first.bbox.origin.x;
    let mut min_y = first.bbox.origin.y;
    let mut max_x = first.bbox.origin.x + first.bbox.size.width;
    let mut max_y = first.bbox.origin.y + first.bbox.size.height;
    for glyph in iter {
        min_x = min_x.min(glyph.bbox.origin.x);
        min_y = min_y.min(glyph.bbox.origin.y);
        max_x = max_x.max(glyph.bbox.origin.x + glyph.bbox.size.width);
        max_y = max_y.max(glyph.bbox.origin.y + glyph.bbox.size.height);
    }
    Some(Rect::new(min_x, min_y, max_x - min_x, max_y - min_y))
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

fn inverse_transform_point(transform: [f32; 6], point: Point) -> Point {
    let [a, b, c, d, e, f] = transform;
    let determinant = a * d - b * c;
    if determinant.abs() <= f32::EPSILON {
        return Point::new(point.x, point.y);
    }
    let x = point.x - e;
    let y = point.y - f;
    Point::new(
        (d * x - c * y) / determinant,
        (-b * x + a * y) / determinant,
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

fn operation_text_bytes(operation: &Operation) -> Option<Vec<u8>> {
    match operation.operator.as_str() {
        "Tj" | "'" | "\"" => operation
            .operands
            .last()
            .and_then(object_string_bytes)
            .map(|bytes| bytes.to_vec()),
        "TJ" => {
            let array = operation.operands.first()?.as_array().ok()?;
            let mut bytes = Vec::new();
            for item in array {
                if let Some(part) = object_string_bytes(item) {
                    bytes.extend_from_slice(part);
                }
            }
            Some(bytes)
        }
        _ => None,
    }
}

fn operation_text_advance(
    operation: &Operation,
    metrics: Option<&FontMetrics>,
    state: &TextParseState,
) -> Option<f32> {
    match operation.operator.as_str() {
        "Tj" | "'" | "\"" => operation
            .operands
            .last()
            .and_then(|object| object_text_advance(object, metrics, state)),
        "TJ" => {
            let array = operation.operands.first()?.as_array().ok()?;
            let mut advance = 0.0;
            let mut has_text = false;
            for item in array {
                if let Some(text_advance) = object_text_advance(item, metrics, state) {
                    advance += text_advance;
                    has_text = true;
                } else if let Some(adjustment) = object_to_f32(item) {
                    advance -= adjustment / 1000.0;
                }
            }
            has_text.then_some(advance)
        }
        _ => None,
    }
}

fn object_text_advance(
    object: &Object,
    metrics: Option<&FontMetrics>,
    state: &TextParseState,
) -> Option<f32> {
    let Object::String(bytes, _) = object else {
        return None;
    };
    metrics.map(|metrics| metrics.text_advance(bytes, state))
}


fn font_set_operation(font_name: &str, font_size: f32) -> Operation {
    Operation::new(
        "Tf",
        vec![
            Object::Name(font_name.as_bytes().to_vec()),
            Object::Real(font_size),
        ],
    )
}

fn needs_cjk_fallback_font(member: &GroupMemberPlan, replacement: &str) -> bool {
    !replacement.is_empty()
        && replacement != member.original_content
        && template_string_format(&member.template) == Some(StringFormat::Hexadecimal)
        && member
            .font_map
            .as_ref()
            .is_some_and(ToUnicodeMap::supports_direct_utf16)
}

fn replacement_text_object(
    template: &Object,
    replacement: String,
    font_map: Option<&ToUnicodeMap>,
) -> CoreResult<Object> {
    if let Some(bytes) = font_map.and_then(|map| map.encode(&replacement)) {
        return Ok(Object::String(bytes, StringFormat::Hexadecimal));
    }

    match template_string_format(template) {
        Some(StringFormat::Hexadecimal) if font_map.is_some_and(ToUnicodeMap::supports_direct_utf16) => {
            Ok(Object::String(
                utf16be_bytes(&replacement),
                StringFormat::Hexadecimal,
            ))
        }
        Some(StringFormat::Literal) | None if replacement.is_ascii() => {
            Ok(Object::string_literal(replacement))
        }
        _ => Err(CoreError::Unsupported(
            "replacement text cannot be encoded by the original PDF font".to_string(),
        )),
    }
}

fn template_string_format(object: &Object) -> Option<StringFormat> {
    match object {
        Object::String(_, format) => Some(*format),
        Object::Array(items) => items.iter().find_map(template_string_format),
        _ => None,
    }
}

fn utf16be_bytes(content: &str) -> Vec<u8> {
    content
        .encode_utf16()
        .flat_map(|unit| unit.to_be_bytes())
        .collect()
}

fn prepare_document_for_full_save(document: &mut Document) {
    for key in [
        b"Prev".as_slice(),
        b"XRefStm".as_slice(),
        b"Type".as_slice(),
        b"W".as_slice(),
        b"Index".as_slice(),
        b"Filter".as_slice(),
        b"Length".as_slice(),
        b"DecodeParms".as_slice(),
    ] {
        document.trailer.remove(key);
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
        Object::String(value, StringFormat::Hexadecimal) => Some(utf16be_to_string(value)),
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

fn is_type3_font_name(
    page_fonts: Option<&BTreeMap<Vec<u8>, &Dictionary>>,
    font_name: &str,
) -> bool {
    page_fonts
        .and_then(|fonts| fonts.get(font_name.as_bytes()).copied())
        .is_some_and(|font| {
            font.get(b"Subtype")
                .ok()
                .and_then(object_name_bytes)
                .as_deref()
                == Some("Type3")
        })
}

fn object_text(object: &Object, font_map: Option<&ToUnicodeMap>) -> Option<String> {
    match object {
        Object::String(value, format) => Some(match font_map {
            Some(map) => map.decode(value),
            None if *format == StringFormat::Hexadecimal => utf16be_to_string(value),
            None => String::from_utf8_lossy(value).into_owned(),
        }),
        _ => None,
    }
}

fn object_string_bytes(object: &Object) -> Option<&[u8]> {
    match object {
        Object::String(value, _) => Some(value),
        _ => None,
    }
}

fn normalized_color_channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn estimate_text_width(content: &str, font_size: f32) -> f32 {
    content.chars().count() as f32 * font_size.max(1.0) * 0.6
}

fn estimated_unicode_width_units(code: u32) -> f32 {
    char::from_u32(code)
        .map(fallback_char_width_units)
        .unwrap_or(1000.0)
}

fn fallback_char_advance(ch: char) -> f32 {
    fallback_char_width_units(ch) / 1000.0
}

fn fallback_char_width_units(ch: char) -> f32 {
    if ch.is_ascii() {
        fallback_ascii_width_units(ch)
    } else if is_cjk_or_fullwidth(ch) {
        1000.0
    } else {
        600.0
    }
}

fn fallback_ascii_width_units(ch: char) -> f32 {
    match ch {
        ' ' => 250.0,
        '!' | '"' | '\'' | ',' | '.' | ':' | ';' | '|' => 280.0,
        '(' | ')' | '[' | ']' | '{' | '}' | '/' | '\\' | '-' => 330.0,
        'i' | 'j' | 'l' | 'I' => 280.0,
        'f' | 'r' | 't' => 330.0,
        'm' | 'w' | 'M' | 'W' => 780.0,
        '0'..='9' => 560.0,
        'A'..='Z' => 670.0,
        'a'..='z' => 560.0,
        _ => 600.0,
    }
}

fn fallback_cjk_widths() -> Object {
    Object::Array(vec![
        Object::Integer(0),
        Object::Array(
            (0u32..=127)
                .map(|code| {
                    char::from_u32(code)
                        .map(|ch| Object::Integer(fallback_ascii_width_units(ch).round() as i64))
                        .unwrap_or(Object::Integer(600))
                })
                .collect(),
        ),
    ])
}

fn is_cjk_or_fullwidth(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x11FF
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE1F
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFFEF
            | 0x20000..=0x3FFFD
    )
}

fn repartition_group_text(
    original_text: &str,
    replacement_text: &str,
    members: &[GroupMemberPlan],
) -> CoreResult<Vec<String>> {
    if members.is_empty() {
        return Ok(Vec::new());
    }

    if let Some(aligned) = repartition_group_text_by_alignment(original_text, replacement_text, members) {
        return Ok(aligned);
    }

    repartition_group_text_by_dp(replacement_text, members)
}

// Distributes replacement chars across members proportionally to original char counts.
// Used as a last-resort fallback when the font can't encode the replacement characters;
// in that case the caller must also enable the CJK fallback font.
fn proportional_split(replacement_text: &str, members: &[GroupMemberPlan]) -> Vec<String> {
    let chars: Vec<char> = replacement_text.chars().collect();
    let total_replacement = chars.len();
    let total_original: usize = members.iter().map(|m| m.original_char_count.max(1)).sum();
    let mut result = Vec::with_capacity(members.len());
    let mut placed = 0usize;
    for (i, member) in members.iter().enumerate() {
        let is_last = i + 1 == members.len();
        let count = if is_last {
            total_replacement - placed
        } else {
            let weight = member.original_char_count.max(1);
            (weight * total_replacement + total_original / 2) / total_original
        };
        let end = (placed + count).min(total_replacement);
        result.push(chars[placed..end].iter().collect());
        placed = end;
    }
    result
}

fn can_write_replacement_with_template(
    template: &Object,
    replacement: &str,
    font_map: Option<&ToUnicodeMap>,
) -> bool {
    if font_map.and_then(|map| map.encode(replacement)).is_some() {
        return true;
    }

    match template_string_format(template) {
        Some(StringFormat::Hexadecimal) => font_map.is_some_and(ToUnicodeMap::supports_direct_utf16),
        Some(StringFormat::Literal) | None => replacement.is_ascii(),
    }
}

fn repartition_group_text_by_alignment(
    original_text: &str,
    replacement_text: &str,
    members: &[GroupMemberPlan],
) -> Option<Vec<String>> {
    let original_chars = original_text.chars().collect::<Vec<_>>();
    let replacement_chars = replacement_text.chars().collect::<Vec<_>>();
    let original_segments = members
        .iter()
        .enumerate()
        .flat_map(|(segment_index, plan)| {
            plan.original_content
                .chars()
                .map(move |_| segment_index)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    if original_chars.len() != original_segments.len() {
        return None;
    }

    let matched_segments =
        longest_common_subsequence_segments(&original_chars, &replacement_chars, &original_segments);
    let mut result = vec![String::new(); members.len()];

    for (index, ch) in replacement_chars.iter().copied().enumerate() {
        let segment_index = if let Some(segment) = matched_segments[index] {
            segment
        } else {
            choose_insertion_segment(index, &matched_segments, ch, members)?
        };
        result[segment_index].push(ch);
    }

    result
        .iter()
        .zip(members.iter())
        .all(|(segment, member)| {
            can_write_replacement_with_template(&member.template, segment, member.font_map.as_ref())
        })
        .then_some(result)
}

fn longest_common_subsequence_segments(
    original_chars: &[char],
    replacement_chars: &[char],
    original_segments: &[usize],
) -> Vec<Option<usize>> {
    let mut dp = vec![vec![0usize; replacement_chars.len() + 1]; original_chars.len() + 1];
    for original_index in 0..original_chars.len() {
        for replacement_index in 0..replacement_chars.len() {
            dp[original_index + 1][replacement_index + 1] = if original_chars[original_index]
                == replacement_chars[replacement_index]
            {
                dp[original_index][replacement_index] + 1
            } else {
                dp[original_index][replacement_index + 1]
                    .max(dp[original_index + 1][replacement_index])
            };
        }
    }

    let mut result = vec![None; replacement_chars.len()];
    let mut original_index = original_chars.len();
    let mut replacement_index = replacement_chars.len();
    while original_index > 0 && replacement_index > 0 {
        if original_chars[original_index - 1] == replacement_chars[replacement_index - 1] {
            result[replacement_index - 1] = Some(original_segments[original_index - 1]);
            original_index -= 1;
            replacement_index -= 1;
        } else if dp[original_index - 1][replacement_index] >= dp[original_index][replacement_index - 1] {
            original_index -= 1;
        } else {
            replacement_index -= 1;
        }
    }
    result
}

fn choose_insertion_segment(
    replacement_index: usize,
    matched_segments: &[Option<usize>],
    ch: char,
    members: &[GroupMemberPlan],
) -> Option<usize> {
    let previous_segment = matched_segments[..replacement_index]
        .iter()
        .rev()
        .find_map(|segment| *segment);
    let next_segment = matched_segments[replacement_index + 1..]
        .iter()
        .find_map(|segment| *segment);

    let lower = previous_segment.unwrap_or(0);
    let upper = next_segment.unwrap_or(members.len().saturating_sub(1));
    let mut candidates = (lower..=upper).collect::<Vec<_>>();
    if let Some(previous) = previous_segment {
        candidates.sort_by_key(|candidate| candidate.abs_diff(previous));
    }

    candidates.into_iter().find(|candidate| {
        can_write_replacement_with_template(
            &members[*candidate].template,
            &ch.to_string(),
            members[*candidate].font_map.as_ref(),
        )
    })
}

fn repartition_group_text_by_dp(
    replacement_text: &str,
    members: &[GroupMemberPlan],
) -> CoreResult<Vec<String>> {
    let chars = replacement_text.chars().collect::<Vec<_>>();
    let char_len = chars.len();
    let member_len = members.len();
    let inf = usize::MAX / 8;
    let mut dp = vec![vec![inf; char_len + 1]; member_len + 1];
    let mut backtrack = vec![vec![None; char_len + 1]; member_len + 1];
    dp[0][0] = 0;

    for member_index in 0..member_len {
        for start in 0..=char_len {
            let current_cost = dp[member_index][start];
            if current_cost >= inf {
                continue;
            }
            let end_range = if member_index + 1 == member_len {
                char_len..=char_len
            } else {
                start..=char_len
            };
            for end in end_range {
                let segment = chars[start..end].iter().collect::<String>();
                if !can_write_replacement_with_template(
                    &members[member_index].template,
                    &segment,
                    members[member_index].font_map.as_ref(),
                ) {
                    continue;
                }
                let deviation = end.abs_diff(start + members[member_index].original_char_count);
                let next_cost = current_cost.saturating_add(deviation * deviation + deviation);
                if next_cost < dp[member_index + 1][end] {
                    dp[member_index + 1][end] = next_cost;
                    backtrack[member_index + 1][end] = Some(start);
                }
            }
        }
    }

    if dp[member_len][char_len] >= inf {
        return Err(CoreError::Unsupported(
            "replacement text cannot be partitioned across the original PDF font runs".to_string(),
        ));
    }

    let mut result = vec![String::new(); member_len];
    let mut end = char_len;
    for member_index in (0..member_len).rev() {
        let start = backtrack[member_index + 1][end].ok_or_else(|| {
            CoreError::Engine("missing grouped text repartition backtrack".to_string())
        })?;
        result[member_index] = chars[start..end].iter().collect();
        end = start;
    }

    Ok(result)
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

#[derive(Debug, Clone)]
struct FontMetrics {
    widths: HashMap<u32, f32>,
    default_width: f32,
    code_len: usize,
    width_scale: f32,
    estimate_missing_widths_from_unicode: bool,
}

impl FontMetrics {
    fn text_advance(&self, bytes: &[u8], state: &TextParseState) -> f32 {
        let codes = self.codes(bytes);
        let glyph_units = codes
            .iter()
            .map(|code| self.width_for_code(*code))
            .sum::<f32>()
            * self.width_scale;
        let char_gaps = codes.len().saturating_sub(1) as f32;
        let word_gaps = codes.iter().filter(|code| **code == 32).count() as f32;
        let font_size = state.font_size.abs().max(1.0);
        let spacing_units =
            (char_gaps * state.char_spacing + word_gaps * state.word_spacing) / font_size;
        (glyph_units + spacing_units) * (state.horizontal_scaling / 100.0)
    }

    fn width_for_code(&self, code: u32) -> f32 {
        self.widths
            .get(&code)
            .copied()
            .or_else(|| {
                self.estimate_missing_widths_from_unicode
                    .then(|| estimated_unicode_width_units(code))
            })
            .unwrap_or(self.default_width)
    }

    fn codes(&self, bytes: &[u8]) -> Vec<u32> {
        if self.code_len <= 1 {
            return bytes.iter().map(|byte| u32::from(*byte)).collect();
        }

        bytes
            .chunks(self.code_len)
            .filter(|chunk| chunk.len() == self.code_len)
            .map(|chunk| {
                chunk
                    .iter()
                    .fold(0u32, |code, byte| (code << 8) | u32::from(*byte))
            })
            .collect()
    }
}

/// Returns `true` when the font defines reduced advance widths (< 850/1000 units)
/// for common fullwidth CJK punctuation characters, indicating the font implements
/// the "punctuation width substitution" (标点宽度替换) typographic feature.
fn font_has_punct_width_squeeze(metrics: &FontMetrics, font_map: &ToUnicodeMap) -> bool {
    const CJK_PUNCT: &[&str] = &[
        "，", "。", "、", "：", "；", "！", "？", "（", "）", "「", "」", "…", "—",
    ];
    let default_state = TextParseState::default();
    CJK_PUNCT.iter().any(|s| {
        font_map
            .encode(s)
            .is_some_and(|bytes| metrics.text_advance(&bytes, &default_state) < 0.85)
    })
}

fn parse_font_metrics(
    document: &Document,
    font: &Dictionary,
    to_unicode: Option<&ToUnicodeMap>,
) -> Option<FontMetrics> {
    if font.get(b"DescendantFonts").is_ok() {
        parse_cid_font_metrics(document, font, to_unicode)
    } else {
        parse_simple_font_metrics(document, font)
    }
}

fn parse_simple_font_metrics(document: &Document, font: &Dictionary) -> Option<FontMetrics> {
    let first_char = font
        .get(b"FirstChar")
        .ok()
        .and_then(object_to_i64)
        .unwrap_or(0)
        .max(0) as u32;
    let mut widths = HashMap::new();
    if let Some(widths_array) = font
        .get(b"Widths")
        .ok()
        .and_then(|object| array_from_object(document, object))
    {
        for (offset, width) in widths_array.iter().enumerate() {
            if let Some(width) = object_to_f32(width) {
                widths.insert(first_char + offset as u32, width);
            }
        }
    }
    if let Some(base_font) = font.get(b"BaseFont").ok().and_then(object_plain_text) {
        if should_use_standard_latin_widths(&base_font, &widths) {
            widths.extend(standard_latin_widths(&base_font));
        }
    }
    (!widths.is_empty()).then_some(FontMetrics {
        widths,
        default_width: 0.0,
        code_len: 1,
        width_scale: simple_font_width_scale(font),
        estimate_missing_widths_from_unicode: false,
    })
}

fn should_use_standard_latin_widths(base_font: &str, widths: &HashMap<u32, f32>) -> bool {
    if !is_standard_proportional_latin_font(base_font) {
        return false;
    }
    if widths.is_empty() {
        return true;
    }
    let mut printable_widths = (32u32..=126).filter_map(|code| widths.get(&code).copied());
    let Some(first) = printable_widths.next() else {
        return true;
    };
    printable_widths.all(|width| (width - first).abs() < 0.01)
}

fn is_standard_proportional_latin_font(base_font: &str) -> bool {
    let normalized = normalize_base_font_name(base_font);
    matches!(
        normalized.as_str(),
        "Helvetica"
            | "Helvetica-Bold"
            | "Helvetica-Oblique"
            | "Helvetica-BoldOblique"
            | "Arial"
            | "Arial,Bold"
            | "Arial,Italic"
            | "Arial,BoldItalic"
            | "Times-Roman"
            | "Times-Bold"
            | "Times-Italic"
            | "Times-BoldItalic"
    )
}

fn normalize_base_font_name(base_font: &str) -> String {
    base_font
        .rsplit_once('+')
        .map(|(_, name)| name)
        .unwrap_or(base_font)
        .trim_start_matches('/')
        .to_string()
}

fn standard_latin_widths(base_font: &str) -> HashMap<u32, f32> {
    let normalized = normalize_base_font_name(base_font);
    (0u32..=127)
        .map(|code| {
            let ch = char::from_u32(code).unwrap_or(' ');
            (code, standard_latin_width_units(&normalized, ch))
        })
        .collect()
}

fn standard_latin_width_units(base_font: &str, ch: char) -> f32 {
    if base_font.starts_with("Times") {
        times_latin_width_units(ch)
    } else {
        helvetica_latin_width_units(ch)
    }
}

fn helvetica_latin_width_units(ch: char) -> f32 {
    match ch {
        ' ' => 278.0,
        '!' | '\'' | ',' | '.' | ':' | ';' | 'I' | 'i' | 'j' | 'l' | '|' => 278.0,
        '"' | '(' | ')' | '-' | '/' | '[' | ']' | 'f' | 'r' | 't' | '{' | '}' => 333.0,
        'J' | 'c' | 's' | 'z' => 500.0,
        '0'..='9' | 'a' | 'b' | 'd' | 'e' | 'g' | 'h' | 'n' | 'o' | 'p' | 'q' | 'u' | 'v' | 'x' | 'y' => 556.0,
        'k' => 500.0,
        'A' | 'B' | 'E' | 'K' | 'P' | 'S' | 'V' | 'X' | 'Y' | 'Z' => 667.0,
        'C' | 'D' | 'H' | 'N' | 'O' | 'R' | 'U' => 722.0,
        'F' | 'L' | 'T' => 611.0,
        'G' | 'M' | 'Q' => 778.0,
        'm' => 833.0,
        'W' => 944.0,
        'w' => 722.0,
        _ => fallback_ascii_width_units(ch),
    }
}

fn times_latin_width_units(ch: char) -> f32 {
    match ch {
        ' ' => 250.0,
        '!' | '\'' | ',' | '.' | ':' | ';' | 'I' | 'i' | 'j' | 'l' => 278.0,
        '"' | '(' | ')' | '/' | '[' | ']' | 'f' | 'r' | 't' | '{' | '}' => 333.0,
        '-' => 333.0,
        '0'..='9' => 500.0,
        'a' | 'c' | 'e' | 's' | 'v' | 'x' | 'z' => 444.0,
        'b' | 'd' | 'g' | 'h' | 'k' | 'n' | 'o' | 'p' | 'q' | 'u' | 'y' => 500.0,
        'm' => 778.0,
        'w' => 722.0,
        'A' | 'B' | 'E' | 'K' | 'P' | 'S' | 'V' | 'X' | 'Y' | 'Z' => 667.0,
        'C' | 'D' | 'H' | 'N' | 'O' | 'R' | 'U' => 722.0,
        'F' | 'L' | 'T' => 611.0,
        'G' | 'M' | 'Q' | 'W' => 889.0,
        _ => fallback_ascii_width_units(ch),
    }
}

fn parse_cid_font_metrics(
    document: &Document,
    font: &Dictionary,
    to_unicode: Option<&ToUnicodeMap>,
) -> Option<FontMetrics> {
    let descendants = font
        .get(b"DescendantFonts")
        .ok()
        .and_then(|object| array_from_object(document, object))?;
    let descendant = descendants
        .first()
        .and_then(|object| dictionary_from_object(document, object))?;
    let default_width = descendant
        .get(b"DW")
        .ok()
        .and_then(object_to_f32)
        .unwrap_or(1000.0);
    let mut widths = HashMap::new();
    if let Some(width_entries) = descendant
        .get(b"W")
        .ok()
        .and_then(|object| array_from_object(document, object))
    {
        parse_cid_widths(width_entries, &mut widths);
    }
    Some(FontMetrics {
        widths,
        default_width,
        code_len: to_unicode
            .map(|map| {
                if map.identity_utf16 && map.max_code_len == 0 {
                    2
                } else {
                    map.max_code_len.max(1)
                }
            })
            .unwrap_or(2),
        width_scale: 0.001,
        estimate_missing_widths_from_unicode: to_unicode.is_some_and(ToUnicodeMap::supports_direct_utf16),
    })
}

fn simple_font_width_scale(font: &Dictionary) -> f32 {
    if font
        .get(b"Subtype")
        .ok()
        .and_then(object_name_bytes)
        .as_deref()
        == Some("Type3")
    {
        font.get(b"FontMatrix")
            .ok()
            .and_then(object_matrix)
            .map(font_matrix_advance_scale)
            .unwrap_or(0.001)
    } else {
        0.001
    }
}

fn font_matrix_advance_scale(matrix: [f32; 6]) -> f32 {
    let scale = matrix[0].hypot(matrix[1]);
    if scale > 0.0 {
        scale
    } else {
        0.001
    }
}

fn parse_cid_widths(entries: &[Object], widths: &mut HashMap<u32, f32>) {
    let mut index = 0usize;
    while index + 1 < entries.len() {
        let Some(first) = object_to_i64(&entries[index]).filter(|value| *value >= 0) else {
            index += 1;
            continue;
        };
        match &entries[index + 1] {
            Object::Array(values) => {
                for (offset, width) in values.iter().enumerate() {
                    if let Some(width) = object_to_f32(width) {
                        widths.insert(first as u32 + offset as u32, width);
                    }
                }
                index += 2;
            }
            _ if index + 2 < entries.len() => {
                if let (Some(last), Some(width)) = (
                    object_to_i64(&entries[index + 1]),
                    object_to_f32(&entries[index + 2]),
                ) {
                    for code in first..=last {
                        if code >= 0 {
                            widths.insert(code as u32, width);
                        }
                    }
                }
                index += 3;
            }
            _ => break,
        }
    }
}

fn font_descriptor<'a>(document: &'a Document, font: &'a Dictionary) -> Option<&'a Dictionary> {
    if let Some(descriptor) = font
        .get(b"FontDescriptor")
        .ok()
        .and_then(|object| dictionary_from_object(document, object))
    {
        return Some(descriptor);
    }

    let descendants = font
        .get(b"DescendantFonts")
        .ok()
        .and_then(|object| array_from_object(document, object))?;
    let descendant = descendants
        .first()
        .and_then(|object| dictionary_from_object(document, object))?;
    descendant
        .get(b"FontDescriptor")
        .ok()
        .and_then(|object| dictionary_from_object(document, object))
}

fn font_file_bytes(
    document: &Document,
    _font: &Dictionary,
    descriptor: &Dictionary,
    to_unicode: Option<&ToUnicodeMap>,
) -> Option<(Vec<u8>, &'static str, &'static str, &'static str)> {
    if let Some(bytes) = descriptor
        .get(b"FontFile2")
        .ok()
        .and_then(|object| stream_from_object(document, object))
        .and_then(stream_content_bytes)
    {
        if let Some(to_unicode) = to_unicode {
            if let Some(remapped) = cff_otf_wrap::wrap_sfnt_with_tounicode_cmap(&bytes, to_unicode)
            {
                return Some((remapped, "font/otf", "opentype", "otf"));
            }
        }
        if !sfnt_has_usable_cmap(&bytes) {
            return Some((bytes, "application/octet-stream", "unknown", "font"));
        }
        return Some((bytes, "font/otf", "opentype", "otf"));
    }

    if let Some(stream) = descriptor
        .get(b"FontFile3")
        .ok()
        .and_then(|object| stream_from_object(document, object))
    {
        let subtype = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(object_name_bytes)
            .unwrap_or_else(|| "OpenType".to_string());
        let (mime_type, format, extension) = match subtype.as_str() {
            "OpenType" => ("font/otf", "opentype", "otf"),
            "Type1C" | "CIDFontType0C" => ("application/x-font-cff", "cff", "cff"),
            _ => ("application/octet-stream", "unknown", "font"),
        };
        if let Some(bytes) = stream_content_bytes(stream) {
            if matches!(subtype.as_str(), "Type1C" | "CIDFontType0C") {
                if let Some(otf) = cff_otf_wrap::wrap_cff_as_otf(&bytes, to_unicode) {
                    return Some((otf, "font/otf", "opentype", "otf"));
                }
            }
            if matches!(format, "opentype") && !sfnt_has_usable_cmap(&bytes) {
                return Some((bytes, "application/octet-stream", "unknown", "font"));
            }
            return Some((bytes, mime_type, format, extension));
        }
    }

    descriptor
        .get(b"FontFile")
        .ok()
        .and_then(|object| stream_from_object(document, object))
        .and_then(stream_content_bytes)
        .map(|bytes| (bytes, "application/x-font-type1", "type1", "pfb"))
}

/// Finds a table by 4-byte tag in an SFNT font binary and returns a slice of its data.
fn sfnt_find_table_data<'a>(sfnt: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    if sfnt.len() < 12 {
        return None;
    }
    let num_tables = u16::from_be_bytes([sfnt[4], sfnt[5]]) as usize;
    for index in 0..num_tables {
        let rec = 12 + index * 16;
        if sfnt.get(rec..rec + 4)? != tag {
            continue;
        }
        let offset = u32::from_be_bytes(sfnt.get(rec + 8..rec + 12)?.try_into().ok()?) as usize;
        let length = u32::from_be_bytes(sfnt.get(rec + 12..rec + 16)?.try_into().ok()?) as usize;
        return sfnt.get(offset..offset + length);
    }
    None
}

/// Scans the GSUB and GPOS tables of an SFNT binary and returns those of
/// [palt, halt, kern, liga, fwid, hwid] that are present, sorted alphabetically.
fn sfnt_layout_features(bytes: &[u8]) -> Vec<String> {
    const INTERESTING: &[[u8; 4]] = &[
        *b"fwid", *b"halt", *b"hwid", *b"kern", *b"liga", *b"palt",
    ];
    let mut found = std::collections::BTreeSet::new();
    for table_tag in [b"GSUB", b"GPOS"] {
        let Some(table) = sfnt_find_table_data(bytes, table_tag) else {
            continue;
        };
        // Common layout table header (version 1.0):
        //   uint16 majorVersion, uint16 minorVersion,
        //   Offset16 scriptListOffset, Offset16 featureListOffset, ...
        if table.len() < 10 {
            continue;
        }
        let feature_list_offset = u16::from_be_bytes([table[6], table[7]]) as usize;
        let Some(feature_list) = table.get(feature_list_offset..) else {
            continue;
        };
        if feature_list.len() < 2 {
            continue;
        }
        let feature_count = u16::from_be_bytes([feature_list[0], feature_list[1]]) as usize;
        // FeatureRecord: Tag(4) + Offset16(2) = 6 bytes each, starting at offset 2
        for i in 0..feature_count {
            let rec = 2 + i * 6;
            let Some(tag) = feature_list.get(rec..rec + 4) else {
                break;
            };
            if let Ok(tag_arr) = <&[u8; 4]>::try_from(tag) {
                if INTERESTING.contains(tag_arr) {
                    if let Ok(s) = std::str::from_utf8(tag) {
                        found.insert(s.to_string());
                    }
                }
            }
        }
    }
    found.into_iter().collect()
}

/// Returns the raw SFNT binary of a font (TrueType/OpenType) from its descriptor,
/// without any cmap remapping.  Returns `None` for pure-CFF / Type1 fonts.
fn font_raw_sfnt_bytes(document: &Document, descriptor: &Dictionary) -> Option<Vec<u8>> {
    // FontFile2 = TrueType or OpenType/TT sfnt
    if let Some(bytes) = descriptor
        .get(b"FontFile2")
        .ok()
        .and_then(|obj| stream_from_object(document, obj))
        .and_then(stream_content_bytes)
    {
        return Some(bytes);
    }
    // FontFile3 with Subtype=OpenType = CFF-in-OTF or TT-in-OTF, still an sfnt
    if let Some(stream) = descriptor
        .get(b"FontFile3")
        .ok()
        .and_then(|obj| stream_from_object(document, obj))
    {
        let subtype = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(object_name_bytes);
        if matches!(subtype.as_deref(), Some("OpenType")) {
            return stream_content_bytes(stream);
        }
    }
    None
}

fn sfnt_has_usable_cmap(bytes: &[u8]) -> bool {
    if bytes.len() < 12 {
        return false;
    }
    let version = &bytes[0..4];
    if version != b"OTTO" && version != [0x00, 0x01, 0x00, 0x00] {
        return false;
    }
    let num_tables = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    for index in 0..num_tables {
        let record_offset = 12 + index * 16;
        if record_offset + 16 > bytes.len() {
            return false;
        }
        if &bytes[record_offset..record_offset + 4] != b"cmap" {
            continue;
        }
        let table_offset = u32::from_be_bytes([
            bytes[record_offset + 8],
            bytes[record_offset + 9],
            bytes[record_offset + 10],
            bytes[record_offset + 11],
        ]) as usize;
        let table_length = u32::from_be_bytes([
            bytes[record_offset + 12],
            bytes[record_offset + 13],
            bytes[record_offset + 14],
            bytes[record_offset + 15],
        ]) as usize;
        if table_offset + table_length > bytes.len() || table_length < 4 {
            return false;
        }
        let cmap = &bytes[table_offset..table_offset + table_length];
        let subtable_count = u16::from_be_bytes([cmap[2], cmap[3]]);
        return subtable_count > 0;
    }
    false
}

#[allow(dead_code)]
mod cff_otf_wrap {
    use super::*;

    pub(super) fn wrap_sfnt_with_tounicode_cmap(
        sfnt: &[u8],
        to_unicode: &ToUnicodeMap,
    ) -> Option<Vec<u8>> {
        let version = sfnt.get(0..4)?.try_into().ok()?;
        let mut tables = read_sfnt_tables(sfnt)?;
        let cmap_entries = build_sfnt_tounicode_cmap_entries(sfnt, to_unicode)?;
        let cmap = build_cmap_table(&cmap_entries)?;

        let mut replaced = false;
        for (tag, data) in &mut tables {
            if tag == b"cmap" {
                *data = cmap.clone();
                replaced = true;
                break;
            }
        }
        if !replaced {
            tables.push((*b"cmap", cmap));
        }
        build_sfnt(version, tables)
    }

    pub(super) fn wrap_cff_as_otf(
        cff: &[u8],
        to_unicode: Option<&ToUnicodeMap>,
    ) -> Option<Vec<u8>> {
        let (sid_map, cff_glyph_count) = parse_cff_unicode_map(cff)?;
        let glyph_count = cff_glyph_count.max(1);

        // For subset Latin CFF fonts, the charset/SID map is the most reliable
        // source of unicode->gid. ToUnicode+Encoding is used as a supplement
        // because some PDFs omit useful glyph names but still provide ToUnicode.
        let mut cmap_entries = filter_cmap_entries_by_glyph_count(sid_map, glyph_count);
        if let Some(extra_entries) = to_unicode.and_then(|tu| build_tounicode_cmap_entries(cff, tu, glyph_count)) {
            merge_cmap_entries(&mut cmap_entries, extra_entries);
        }

        let cmap = if cmap_entries.is_empty() {
            build_cmap_table(&[(0x0020, 0)])?
        } else {
            build_cmap_table(&cmap_entries)?
        };
        let hmtx = build_hmtx_table(glyph_count);
        let tables = vec![
            (*b"CFF ", cff.to_vec()),
            (*b"OS/2", build_os2_table()),
            (*b"cmap", cmap),
            (*b"head", build_head_table()),
            (*b"hhea", build_hhea_table(glyph_count)),
            (*b"hmtx", hmtx),
            (*b"maxp", build_maxp_table(glyph_count)),
            (*b"name", build_name_table()),
            (*b"post", build_post_table()),
        ];
        build_sfnt(*b"OTTO", tables)
    }

    /// Build cmap entries using the PDF ToUnicode CMap and the CFF Encoding.
    ///
    /// The CFF Encoding maps character codes → glyph IDs.  The PDF ToUnicode CMap
    /// maps character codes → Unicode strings.  By combining both we get
    /// Unicode → glyph ID which is exactly what the OTF cmap needs.
    fn build_tounicode_cmap_entries(
        cff: &[u8],
        to_unicode: &ToUnicodeMap,
        glyph_count: u16,
    ) -> Option<Vec<(u16, u16)>> {
        let encoding = parse_cff_encoding(cff)?;
        if encoding.is_empty() {
            return None;
        }
        let mut entries = Vec::new();
        for &(char_code, glyph_id) in &encoding {
            let key = vec![char_code];
            if let Some(unicode_str) = to_unicode.forward.get(&key) {
                if let Some(ch) = unicode_str.chars().next() {
                    let cp = ch as u32;
                    if cp > 0 && cp <= 0xFFFF && glyph_id != 0 && glyph_id < glyph_count {
                        entries.push((cp as u16, glyph_id));
                    }
                }
            }
        }
        if entries.is_empty() {
            // Try 2-byte codes (CID fonts)
            for &(char_code, glyph_id) in &encoding {
                let key = vec![0u8, char_code];
                if let Some(unicode_str) = to_unicode.forward.get(&key) {
                    if let Some(ch) = unicode_str.chars().next() {
                        let cp = ch as u32;
                        if cp > 0 && cp <= 0xFFFF && glyph_id != 0 && glyph_id < glyph_count {
                            entries.push((cp as u16, glyph_id));
                        }
                    }
                }
            }
        }
        if entries.is_empty() {
            None
        } else {
            entries.sort_by_key(|(cp, _)| *cp);
            entries.dedup_by_key(|(cp, _)| *cp);
            Some(entries)
        }
    }

    fn filter_cmap_entries_by_glyph_count(
        mut entries: Vec<(u16, u16)>,
        glyph_count: u16,
    ) -> Vec<(u16, u16)> {
        entries.retain(|(_, glyph_id)| *glyph_id != 0 && *glyph_id < glyph_count);
        entries.sort_by_key(|(cp, _)| *cp);
        entries.dedup_by_key(|(cp, _)| *cp);
        entries
    }

    fn merge_cmap_entries(base: &mut Vec<(u16, u16)>, extra: Vec<(u16, u16)>) {
        let mut seen = base.iter().map(|(cp, _)| *cp).collect::<HashSet<_>>();
        for (cp, glyph_id) in extra {
            if seen.insert(cp) {
                base.push((cp, glyph_id));
            }
        }
        base.sort_by_key(|(cp, _)| *cp);
    }

    /// Parse the CFF Encoding table to get character_code → glyph_index mappings.
    ///
    /// The Encoding is referenced by operator 16 in the Top DICT.
    /// Value 0 = Standard Encoding, 1 = Expert Encoding, otherwise offset.
    fn parse_cff_encoding(cff: &[u8]) -> Option<Vec<(u8, u16)>> {
        let header_size = usize::from(*cff.get(2)?);
        let (_, name_end) = read_cff_index(cff, header_size)?;
        let (top_dicts, _) = read_cff_index(cff, name_end)?;
        let top_dict = top_dicts.first()?;
        let top_values = parse_cff_top_dict(top_dict);

        let encoding_offset = *top_values.get(&16).unwrap_or(&0);
        match encoding_offset {
            0 => Some(standard_encoding_map()),
            1 => Some(expert_encoding_map()),
            offset => {
                let offset = usize::try_from(offset).ok()?;
                parse_cff_encoding_at(cff, offset)
            }
        }
    }

    fn parse_cff_encoding_at(cff: &[u8], offset: usize) -> Option<Vec<(u8, u16)>> {
        let format = *cff.get(offset)? & 0x7F; // high bit = supplement flag
        let mut result = Vec::new();
        match format {
            0 => {
                let n_codes = usize::from(*cff.get(offset + 1)?);
                for i in 0..n_codes {
                    let code = *cff.get(offset + 2 + i)?;
                    result.push((code, (i + 1) as u16)); // GID 1, 2, 3, ...
                }
            }
            1 => {
                let n_ranges = usize::from(*cff.get(offset + 1)?);
                let mut gid = 1u16;
                let mut cursor = offset + 2;
                for _ in 0..n_ranges {
                    let first = *cff.get(cursor)?;
                    let n_left = usize::from(*cff.get(cursor + 1)?);
                    cursor += 2;
                    for j in 0..=n_left {
                        let code = first.wrapping_add(j as u8);
                        result.push((code, gid));
                        gid += 1;
                    }
                }
            }
            _ => return None,
        }
        Some(result)
    }

    fn standard_encoding_map() -> Vec<(u8, u16)> {
        // Map from standard encoding character codes to glyph index (1-based)
        // This is the Adobe Standard Encoding; map code→GID where GID = code position
        let codes: &[u8] = &[
            0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, // space ! " # $ % & '
            0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F, // ( ) * + , - . /
            0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, // 0-7
            0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F, // 8 9 : ; < = > ?
            0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, // @ A-G
            0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, // H-O
            0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, // P-W
            0x58, 0x59, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F, // X-Z [ \ ] ^ _
            0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, // ` a-g
            0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, // h-o
            0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, // p-w
            0x78, 0x79, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, // x-z { | } ~
        ];
        codes
            .iter()
            .enumerate()
            .map(|(i, &code)| (code, (i + 1) as u16))
            .collect()
    }

    fn expert_encoding_map() -> Vec<(u8, u16)> {
        // Simplified expert encoding — rare in PDF subset fonts
        standard_encoding_map()
    }

    fn build_sfnt(sfnt_version: [u8; 4], mut tables: Vec<([u8; 4], Vec<u8>)>) -> Option<Vec<u8>> {
        tables.sort_by_key(|(tag, _)| *tag);
        let num_tables = u16::try_from(tables.len()).ok()?;
        let max_power = 1u16 << (15 - num_tables.leading_zeros() as u16);
        let search_range = max_power * 16;
        let entry_selector = max_power.trailing_zeros() as u16;
        let range_shift = num_tables * 16 - search_range;
        let directory_len = 12usize + tables.len() * 16;
        let mut offset = directory_len as u32;
        let mut records = Vec::new();
        let mut payload = Vec::new();

        for (tag, data) in &tables {
            let checksum = table_checksum(data);
            records.push((*tag, checksum, offset, data.len() as u32));
            payload.extend_from_slice(data);
            while payload.len() % 4 != 0 {
                payload.push(0);
            }
            offset = directory_len as u32 + payload.len() as u32;
        }

        let mut output = Vec::new();
        output.extend_from_slice(&sfnt_version);
        push_u16(&mut output, num_tables);
        push_u16(&mut output, search_range);
        push_u16(&mut output, entry_selector);
        push_u16(&mut output, range_shift);
        for (tag, checksum, table_offset, length) in &records {
            output.extend_from_slice(tag);
            push_u32(&mut output, *checksum);
            push_u32(&mut output, *table_offset);
            push_u32(&mut output, *length);
        }
        output.extend_from_slice(&payload);

        let checksum_adjustment = 0xB1B0AFBAu32.wrapping_sub(table_checksum(&output));
        let head_offset = records
            .iter()
            .find(|(tag, _, _, _)| tag == b"head")
            .map(|(_, _, table_offset, _)| *table_offset as usize)?;
        output[head_offset + 8..head_offset + 12]
            .copy_from_slice(&checksum_adjustment.to_be_bytes());
        Some(output)
    }

    fn build_head_table() -> Vec<u8> {
        let mut table = Vec::new();
        push_u32(&mut table, 0x0001_0000);
        push_u32(&mut table, 0x0001_0000);
        push_u32(&mut table, 0);
        push_u32(&mut table, 0x5F0F_3CF5);
        push_u16(&mut table, 0);
        push_u16(&mut table, 1000);
        table.extend_from_slice(&0u64.to_be_bytes());
        table.extend_from_slice(&0u64.to_be_bytes());
        push_i16(&mut table, 0);
        push_i16(&mut table, -250);
        push_i16(&mut table, 1000);
        push_i16(&mut table, 1000);
        push_u16(&mut table, 0);
        push_u16(&mut table, 8);
        push_i16(&mut table, 2);
        push_i16(&mut table, 0);
        push_i16(&mut table, 0);
        table
    }

    fn build_hhea_table(glyph_count: u16) -> Vec<u8> {
        let mut table = Vec::new();
        push_u32(&mut table, 0x0001_0000);
        push_i16(&mut table, 800);
        push_i16(&mut table, -200);
        push_i16(&mut table, 0);
        push_u16(&mut table, 1000);
        push_i16(&mut table, 0);
        push_i16(&mut table, 0);
        push_i16(&mut table, 1000);
        push_i16(&mut table, 1);
        push_i16(&mut table, 0);
        push_i16(&mut table, 0);
        for _ in 0..4 {
            push_i16(&mut table, 0);
        }
        push_i16(&mut table, 0);
        push_u16(&mut table, glyph_count);
        table
    }

    fn build_maxp_table(glyph_count: u16) -> Vec<u8> {
        let mut table = Vec::new();
        push_u32(&mut table, 0x0000_5000);
        push_u16(&mut table, glyph_count);
        table
    }

    fn build_hmtx_table(glyph_count: u16) -> Vec<u8> {
        let mut table = Vec::new();
        for _ in 0..glyph_count {
            push_u16(&mut table, 600);
            push_i16(&mut table, 0);
        }
        table
    }

    fn build_os2_table() -> Vec<u8> {
        let mut table = vec![0u8; 78];
        table[0..2].copy_from_slice(&0u16.to_be_bytes());
        table[2..4].copy_from_slice(&600i16.to_be_bytes());
        table[4..6].copy_from_slice(&400u16.to_be_bytes());
        table[6..8].copy_from_slice(&5u16.to_be_bytes());
        table[68..70].copy_from_slice(&800i16.to_be_bytes());
        table[70..72].copy_from_slice(&200i16.to_be_bytes());
        table
    }

    fn build_post_table() -> Vec<u8> {
        let mut table = Vec::new();
        push_u32(&mut table, 0x0003_0000);
        push_u32(&mut table, 0);
        push_i16(&mut table, 0);
        push_i16(&mut table, 0);
        push_u32(&mut table, 0);
        push_u32(&mut table, 0);
        push_u32(&mut table, 0);
        push_u32(&mut table, 0);
        push_u32(&mut table, 0);
        table
    }

    fn build_name_table() -> Vec<u8> {
        let names: &[(u16, &str)] = &[
            (1, "PDF Embedded CFF"),
            (2, "Regular"),
            (4, "PDF Embedded CFF Regular"),
            (6, "PDFEmbeddedCFF-Regular"),
        ];
        let encoded: Vec<(u16, Vec<u8>)> = names
            .iter()
            .map(|(id, text)| {
                let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
                (*id, bytes)
            })
            .collect();
        let count = encoded.len() as u16;
        let storage_offset = 6 + count * 12;
        let mut table = Vec::new();
        push_u16(&mut table, 0); // format
        push_u16(&mut table, count);
        push_u16(&mut table, storage_offset);
        let mut string_offset = 0u16;
        for (name_id, bytes) in &encoded {
            push_u16(&mut table, 3); // platformID (Windows)
            push_u16(&mut table, 1); // encodingID (Unicode BMP)
            push_u16(&mut table, 0x0409); // languageID (English US)
            push_u16(&mut table, *name_id);
            push_u16(&mut table, bytes.len() as u16);
            push_u16(&mut table, string_offset);
            string_offset += bytes.len() as u16;
        }
        for (_, bytes) in &encoded {
            table.extend_from_slice(bytes);
        }
        table
    }

    fn build_cmap_table(map: &[(u16, u16)]) -> Option<Vec<u8>> {
        let mut entries = map.to_vec();
        entries.sort_by_key(|(code, _)| *code);
        entries.dedup_by_key(|(code, _)| *code);
        entries.retain(|(code, _)| *code != 0xFFFF);
        let seg_count = u16::try_from(entries.len() + 1).ok()?;
        let seg_count_x2 = seg_count * 2;
        let max_power = 1u16 << (15 - seg_count.leading_zeros() as u16);
        let search_range = max_power * 2;
        let entry_selector = max_power.trailing_zeros() as u16;
        let range_shift = seg_count_x2 - search_range;
        let length = 16 + usize::from(seg_count) * 8;
        let mut subtable = Vec::new();
        push_u16(&mut subtable, 4);
        push_u16(&mut subtable, length as u16);
        push_u16(&mut subtable, 0);
        push_u16(&mut subtable, seg_count_x2);
        push_u16(&mut subtable, search_range);
        push_u16(&mut subtable, entry_selector);
        push_u16(&mut subtable, range_shift);
        for (code, _) in &entries {
            push_u16(&mut subtable, *code);
        }
        push_u16(&mut subtable, 0xFFFF);
        push_u16(&mut subtable, 0);
        for (code, _) in &entries {
            push_u16(&mut subtable, *code);
        }
        push_u16(&mut subtable, 0xFFFF);
        for (code, glyph_id) in &entries {
            push_i16(&mut subtable, (*glyph_id as i32 - *code as i32) as i16);
        }
        push_i16(&mut subtable, 1);
        for _ in 0..seg_count {
            push_u16(&mut subtable, 0);
        }

        let mut table = Vec::new();
        push_u16(&mut table, 0);
        push_u16(&mut table, 1);
        push_u16(&mut table, 3);
        push_u16(&mut table, 1);
        push_u32(&mut table, 12);
        table.extend_from_slice(&subtable);
        Some(table)
    }

    fn build_sfnt_tounicode_cmap_entries(
        sfnt: &[u8],
        to_unicode: &ToUnicodeMap,
    ) -> Option<Vec<(u16, u16)>> {
        let cmap = find_sfnt_table(sfnt, b"cmap")?;
        let glyph_count = read_sfnt_num_glyphs(sfnt)?;
        let mut entries = Vec::new();
        for (source, unicode_str) in &to_unicode.forward {
            let char_code = bytes_to_u32(source)?;
            let glyph_id = lookup_sfnt_glyph_id(cmap, char_code)?;
            let ch = unicode_str.chars().next()?;
            let codepoint = ch as u32;
            if glyph_id != 0
                && glyph_id < glyph_count
                && codepoint > 0
                && codepoint <= 0xFFFF
            {
                entries.push((codepoint as u16, glyph_id));
            }
        }
        if entries.is_empty() {
            None
        } else {
            entries.sort_by_key(|(cp, _)| *cp);
            entries.dedup_by_key(|(cp, _)| *cp);
            Some(entries)
        }
    }

    fn read_sfnt_num_glyphs(sfnt: &[u8]) -> Option<u16> {
        let maxp = find_sfnt_table(sfnt, b"maxp")?;
        read_u16(maxp, 4)
    }

    fn read_sfnt_tables(sfnt: &[u8]) -> Option<Vec<([u8; 4], Vec<u8>)>> {
        if sfnt.len() < 12 {
            return None;
        }
        let num_tables = usize::from(read_u16(sfnt, 4)?);
        let mut tables = Vec::with_capacity(num_tables);
        for index in 0..num_tables {
            let record_offset = 12 + index * 16;
            let tag: [u8; 4] = sfnt.get(record_offset..record_offset + 4)?.try_into().ok()?;
            let table_offset = read_u32(sfnt, record_offset + 8)? as usize;
            let table_length = read_u32(sfnt, record_offset + 12)? as usize;
            let data = sfnt.get(table_offset..table_offset + table_length)?.to_vec();
            tables.push((tag, data));
        }
        Some(tables)
    }

    fn find_sfnt_table<'a>(sfnt: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
        if sfnt.len() < 12 {
            return None;
        }
        let num_tables = usize::from(read_u16(sfnt, 4)?);
        for index in 0..num_tables {
            let record_offset = 12 + index * 16;
            if sfnt.get(record_offset..record_offset + 4)? != tag {
                continue;
            }
            let table_offset = read_u32(sfnt, record_offset + 8)? as usize;
            let table_length = read_u32(sfnt, record_offset + 12)? as usize;
            return sfnt.get(table_offset..table_offset + table_length);
        }
        None
    }

    fn lookup_sfnt_glyph_id(cmap: &[u8], char_code: u32) -> Option<u16> {
        if cmap.len() < 4 {
            return None;
        }
        let subtable_count = usize::from(read_u16(cmap, 2)?);
        for index in 0..subtable_count {
            let record_offset = 4 + index * 8;
            let subtable_offset = read_u32(cmap, record_offset + 4)? as usize;
            let subtable = cmap.get(subtable_offset..)?;
            let format = read_u16(subtable, 0)?;
            let glyph_id = match format {
                4 => lookup_cmap_format4(subtable, char_code),
                12 => lookup_cmap_format12(subtable, char_code),
                _ => None,
            };
            if let Some(glyph_id) = glyph_id {
                if glyph_id != 0 {
                    return Some(glyph_id);
                }
            }
        }
        None
    }

    fn lookup_cmap_format4(subtable: &[u8], char_code: u32) -> Option<u16> {
        let code = u16::try_from(char_code).ok()?;
        let seg_count = usize::from(read_u16(subtable, 6)?) / 2;
        let end_codes_offset = 14usize;
        let start_codes_offset = end_codes_offset + seg_count * 2 + 2;
        let id_delta_offset = start_codes_offset + seg_count * 2;
        let id_range_offset_offset = id_delta_offset + seg_count * 2;
        for index in 0..seg_count {
            let end_code = read_u16(subtable, end_codes_offset + index * 2)?;
            let start_code = read_u16(subtable, start_codes_offset + index * 2)?;
            if code < start_code || code > end_code {
                continue;
            }
            let id_delta = read_u16(subtable, id_delta_offset + index * 2)?;
            let id_range_offset = read_u16(subtable, id_range_offset_offset + index * 2)?;
            if id_range_offset == 0 {
                return Some(code.wrapping_add(id_delta));
            }
            let ro_pos = id_range_offset_offset + index * 2;
            let glyph_offset =
                ro_pos + usize::from(id_range_offset) + usize::from(code - start_code) * 2;
            let glyph_id = read_u16(subtable, glyph_offset)?;
            if glyph_id == 0 {
                return Some(0);
            }
            return Some(glyph_id.wrapping_add(id_delta));
        }
        None
    }

    fn lookup_cmap_format12(subtable: &[u8], char_code: u32) -> Option<u16> {
        let groups = read_u32(subtable, 12)? as usize;
        for index in 0..groups {
            let group_offset = 16 + index * 12;
            let start_char = read_u32(subtable, group_offset)?;
            let end_char = read_u32(subtable, group_offset + 4)?;
            if char_code < start_char || char_code > end_char {
                continue;
            }
            let start_glyph = read_u32(subtable, group_offset + 8)?;
            return u16::try_from(start_glyph + (char_code - start_char)).ok();
        }
        None
    }

    fn bytes_to_u32(bytes: &[u8]) -> Option<u32> {
        if bytes.is_empty() || bytes.len() > 4 {
            return None;
        }
        Some(
            bytes
                .iter()
                .fold(0u32, |value, byte| (value << 8) | u32::from(*byte)),
        )
    }

    fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
        let bytes: [u8; 4] = data.get(offset..offset + 4)?.try_into().ok()?;
        Some(u32::from_be_bytes(bytes))
    }

    fn parse_cff_unicode_map(cff: &[u8]) -> Option<(Vec<(u16, u16)>, u16)> {
        let header_size = usize::from(*cff.get(2)?);
        let (_, name_end) = read_cff_index(cff, header_size)?;
        let (top_dicts, top_end) = read_cff_index(cff, name_end)?;
        let (_, string_end) = read_cff_index(cff, top_end)?;
        let (_, _) = read_cff_index(cff, string_end)?;
        let top_dict = top_dicts.first()?;
        let top_values = parse_cff_top_dict(top_dict);
        let charstrings_offset = usize::try_from(*top_values.get(&17)?).ok()?;
        let charset_offset = usize::try_from(*top_values.get(&15).unwrap_or(&0)).ok()?;
        let (charstrings, _) = read_cff_index(cff, charstrings_offset)?;
        let glyph_count = charstrings.len();
        let sids = parse_cff_charset(cff, charset_offset, glyph_count)?;
        let mut map = Vec::new();
        for (glyph_index, sid) in sids.into_iter().enumerate().skip(1) {
            if let Some(unicode) = cff_sid_to_unicode(sid) {
                map.push((unicode, glyph_index as u16));
            }
        }
        let actual_count = u16::try_from(glyph_count).unwrap_or(u16::MAX);
        Some((map, actual_count))
    }

    fn read_cff_index(data: &[u8], offset: usize) -> Option<(Vec<&[u8]>, usize)> {
        let count = usize::from(read_u16(data, offset)?);
        if count == 0 {
            return Some((Vec::new(), offset + 2));
        }
        let off_size = usize::from(*data.get(offset + 2)?);
        let offsets_start = offset + 3;
        let data_start = offsets_start + (count + 1) * off_size;
        let mut offsets = Vec::new();
        for index in 0..=count {
            offsets.push(read_cff_offset(
                data,
                offsets_start + index * off_size,
                off_size,
            )?);
        }
        let mut objects = Vec::new();
        for index in 0..count {
            let start = data_start + offsets[index].saturating_sub(1);
            let end = data_start + offsets[index + 1].saturating_sub(1);
            objects.push(data.get(start..end)?);
        }
        let end = data_start + offsets[count].saturating_sub(1);
        Some((objects, end))
    }

    fn read_cff_offset(data: &[u8], offset: usize, size: usize) -> Option<usize> {
        let mut value = 0usize;
        for byte in data.get(offset..offset + size)? {
            value = (value << 8) | usize::from(*byte);
        }
        Some(value)
    }

    fn parse_cff_top_dict(dict: &[u8]) -> HashMap<u16, i32> {
        let mut values = HashMap::new();
        let mut stack = Vec::new();
        let mut index = 0usize;
        while index < dict.len() {
            let byte = dict[index];
            match byte {
                0..=21 => {
                    let operator = if byte == 12 {
                        index += 1;
                        1200 + u16::from(*dict.get(index).unwrap_or(&0))
                    } else {
                        u16::from(byte)
                    };
                    if let Some(value) = stack.last().copied() {
                        values.insert(operator, value);
                    }
                    stack.clear();
                    index += 1;
                }
                28 => {
                    if index + 2 < dict.len() {
                        stack.push(i16::from_be_bytes([dict[index + 1], dict[index + 2]]) as i32);
                    }
                    index += 3;
                }
                29 => {
                    if index + 4 < dict.len() {
                        stack.push(i32::from_be_bytes([
                            dict[index + 1],
                            dict[index + 2],
                            dict[index + 3],
                            dict[index + 4],
                        ]));
                    }
                    index += 5;
                }
                32..=246 => {
                    stack.push(i32::from(byte) - 139);
                    index += 1;
                }
                247..=250 => {
                    if let Some(next) = dict.get(index + 1) {
                        stack.push((i32::from(byte) - 247) * 256 + i32::from(*next) + 108);
                    }
                    index += 2;
                }
                251..=254 => {
                    if let Some(next) = dict.get(index + 1) {
                        stack.push(-((i32::from(byte) - 251) * 256) - i32::from(*next) - 108);
                    }
                    index += 2;
                }
                _ => index += 1,
            }
        }
        values
    }

    fn parse_cff_charset(cff: &[u8], offset: usize, glyph_count: usize) -> Option<Vec<u16>> {
        if glyph_count == 0 {
            return Some(Vec::new());
        }
        if offset == 0 {
            return Some((0..glyph_count as u16).collect());
        }
        let format = *cff.get(offset)?;
        let mut sids = vec![0u16];
        let mut cursor = offset + 1;
        match format {
            0 => {
                while sids.len() < glyph_count {
                    sids.push(read_u16(cff, cursor)?);
                    cursor += 2;
                }
            }
            1 => {
                while sids.len() < glyph_count {
                    let first = read_u16(cff, cursor)?;
                    let n_left = u16::from(*cff.get(cursor + 2)?);
                    cursor += 3;
                    for sid in first..=first + n_left {
                        sids.push(sid);
                        if sids.len() == glyph_count {
                            break;
                        }
                    }
                }
            }
            2 => {
                while sids.len() < glyph_count {
                    let first = read_u16(cff, cursor)?;
                    let n_left = read_u16(cff, cursor + 2)?;
                    cursor += 4;
                    for sid in first..=first + n_left {
                        sids.push(sid);
                        if sids.len() == glyph_count {
                            break;
                        }
                    }
                }
            }
            _ => return None,
        }
        Some(sids)
    }

    fn cff_sid_to_unicode(sid: u16) -> Option<u16> {
        // Standard CFF SID to Unicode mapping (Adobe standard encoding, SIDs 0–390)
        match sid {
            0 => Some(0x0000),                // .notdef
            1 => Some(0x0020),                // space
            2 => Some(0x0021),                // exclam
            3 => Some(0x0022),                // quotedbl
            4 => Some(0x0023),                // numbersign
            5 => Some(0x0024),                // dollar
            6 => Some(0x0025),                // percent
            7 => Some(0x0026),                // ampersand
            8 => Some(0x2019),                // quoteright
            9 => Some(0x0028),                // parenleft
            10 => Some(0x0029),               // parenright
            11 => Some(0x002A),               // asterisk
            12 => Some(0x002B),               // plus
            13 => Some(0x002C),               // comma
            14 => Some(0x002D),               // hyphen
            15 => Some(0x002E),               // period
            16 => Some(0x002F),               // slash
            17..=26 => Some(0x30 + sid - 17), // zero..nine
            27 => Some(0x003A),               // colon
            28 => Some(0x003B),               // semicolon
            29 => Some(0x003C),               // less
            30 => Some(0x003D),               // equal
            31 => Some(0x003E),               // greater
            32 => Some(0x003F),               // question
            33 => Some(0x0040),               // at
            34..=59 => Some(0x41 + sid - 34), // A..Z
            60 => Some(0x005B),               // bracketleft
            61 => Some(0x005C),               // backslash
            62 => Some(0x005D),               // bracketright
            63 => Some(0x005E),               // asciicircum
            64 => Some(0x005F),               // underscore
            65 => Some(0x2018),               // quoteleft
            66..=91 => Some(0x61 + sid - 66), // a..z
            92 => Some(0x007B),               // braceleft
            93 => Some(0x007C),               // bar
            94 => Some(0x007D),               // braceright
            95 => Some(0x007E),               // asciitilde
            96 => Some(0x00A1),               // exclamdown
            97 => Some(0x00A2),               // cent
            98 => Some(0x00A3),               // sterling
            99 => Some(0x2044),               // fraction
            100 => Some(0x00A5),              // yen
            101 => Some(0x0192),              // florin
            102 => Some(0x00A7),              // section
            103 => Some(0x00A4),              // currency
            104 => Some(0x0027),              // quotesingle
            105 => Some(0x201C),              // quotedblleft
            106 => Some(0x00AB),              // guillemotleft
            107 => Some(0x2039),              // guilsinglleft
            108 => Some(0x203A),              // guilsinglright
            109 => Some(0xFB01),              // fi
            110 => Some(0xFB02),              // fl
            111 => Some(0x2013),              // endash
            112 => Some(0x2020),              // dagger
            113 => Some(0x2021),              // daggerdbl
            114 => Some(0x00B7),              // periodcentered
            115 => Some(0x00B6),              // paragraph
            116 => Some(0x2022),              // bullet
            117 => Some(0x201A),              // quotesinglbase
            118 => Some(0x201E),              // quotedblbase
            119 => Some(0x201D),              // quotedblright
            120 => Some(0x00BB),              // guillemotright
            121 => Some(0x2026),              // ellipsis
            122 => Some(0x2030),              // perthousand
            123 => Some(0x00BF),              // questiondown
            124 => Some(0x0060),              // grave
            125 => Some(0x00B4),              // acute
            126 => Some(0x02C6),              // circumflex
            127 => Some(0x02DC),              // tilde
            128 => Some(0x00AF),              // macron
            129 => Some(0x02D8),              // breve
            130 => Some(0x02D9),              // dotaccent
            131 => Some(0x00A8),              // dieresis
            132 => Some(0x02DA),              // ring
            133 => Some(0x00B8),              // cedilla
            134 => Some(0x02DD),              // hungarumlaut
            135 => Some(0x02DB),              // ogonek
            136 => Some(0x02C7),              // caron
            137 => Some(0x2014),              // emdash
            138 => Some(0x00C6),              // AE
            139 => Some(0x00AA),              // ordfeminine
            140 => Some(0x0141),              // Lslash
            141 => Some(0x00D8),              // Oslash
            142 => Some(0x0152),              // OE
            143 => Some(0x00BA),              // ordmasculine
            144 => Some(0x00E6),              // ae
            145 => Some(0x0131),              // dotlessi
            146 => Some(0x0142),              // lslash
            147 => Some(0x00F8),              // oslash
            148 => Some(0x0153),              // oe
            149 => Some(0x00DF),              // germandbls
            150 => Some(0x00C1),              // Aacute
            151 => Some(0x00C2),              // Acircumflex
            152 => Some(0x00C4),              // Adieresis
            153 => Some(0x00C0),              // Agrave
            154 => Some(0x00C5),              // Aring
            155 => Some(0x00C3),              // Atilde
            156 => Some(0x00C7),              // Ccedilla
            157 => Some(0x00C9),              // Eacute
            158 => Some(0x00CA),              // Ecircumflex
            159 => Some(0x00CB),              // Edieresis
            160 => Some(0x00C8),              // Egrave
            161 => Some(0x00CD),              // Iacute
            162 => Some(0x00CE),              // Icircumflex
            163 => Some(0x00CF),              // Idieresis
            164 => Some(0x00CC),              // Igrave
            165 => Some(0x00D1),              // Ntilde
            166 => Some(0x00D3),              // Oacute
            167 => Some(0x00D4),              // Ocircumflex
            168 => Some(0x00D6),              // Odieresis
            169 => Some(0x00D2),              // Ograve
            170 => Some(0x00D5),              // Otilde
            171 => Some(0x0160),              // Scaron
            172 => Some(0x00DA),              // Uacute
            173 => Some(0x00DB),              // Ucircumflex
            174 => Some(0x00DC),              // Udieresis
            175 => Some(0x00D9),              // Ugrave
            176 => Some(0x0178),              // Ydieresis
            177 => Some(0x017D),              // Zcaron
            178 => Some(0x00E1),              // aacute
            179 => Some(0x00E2),              // acircumflex
            180 => Some(0x00E4),              // adieresis
            181 => Some(0x00E0),              // agrave
            182 => Some(0x00E5),              // aring
            183 => Some(0x00E3),              // atilde
            184 => Some(0x00E7),              // ccedilla
            185 => Some(0x00E9),              // eacute
            186 => Some(0x00EA),              // ecircumflex
            187 => Some(0x00EB),              // edieresis
            188 => Some(0x00E8),              // egrave
            189 => Some(0x00ED),              // iacute
            190 => Some(0x00EE),              // icircumflex
            191 => Some(0x00EF),              // idieresis
            192 => Some(0x00EC),              // igrave
            193 => Some(0x00F1),              // ntilde
            194 => Some(0x00F3),              // oacute
            195 => Some(0x00F4),              // ocircumflex
            196 => Some(0x00F6),              // odieresis
            197 => Some(0x00F2),              // ograve
            198 => Some(0x00F5),              // otilde
            199 => Some(0x0161),              // scaron
            200 => Some(0x00FA),              // uacute
            201 => Some(0x00FB),              // ucircumflex
            202 => Some(0x00FC),              // udieresis
            203 => Some(0x00F9),              // ugrave
            204 => Some(0x00FF),              // ydieresis
            205 => Some(0x017E),              // zcaron
            206 => Some(0x00A0),              // nbspace (non-breaking space)
            207 => Some(0x00AC),              // logicalnot
            208 => Some(0x00A6),              // brokenbar
            209 => Some(0x00A9),              // copyright
            210 => Some(0x00AE),              // registered
            211 => Some(0x2122),              // trademark
            212 => Some(0x00B0),              // degree
            213 => Some(0x00B1),              // plusminus
            214 => Some(0x00B2),              // twosuperior
            215 => Some(0x00B3),              // threesuperior
            216 => Some(0x00D7),              // multiply
            217 => Some(0x00B9),              // onesuperior
            218 => Some(0x00F7),              // divide
            219 => Some(0x00BC),              // onequarter
            220 => Some(0x00BD),              // onehalf
            221 => Some(0x00BE),              // threequarters
            222 => Some(0x20AC),              // Euro
            _ => None,
        }
    }

    fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
        Some(u16::from_be_bytes([
            *data.get(offset)?,
            *data.get(offset + 1)?,
        ]))
    }

    fn table_checksum(data: &[u8]) -> u32 {
        let mut sum = 0u32;
        for chunk in data.chunks(4) {
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            sum = sum.wrapping_add(u32::from_be_bytes(word));
        }
        sum
    }

    fn push_u16(output: &mut Vec<u8>, value: u16) {
        output.extend_from_slice(&value.to_be_bytes());
    }

    fn push_i16(output: &mut Vec<u8>, value: i16) {
        output.extend_from_slice(&value.to_be_bytes());
    }

    fn push_u32(output: &mut Vec<u8>, value: u32) {
        output.extend_from_slice(&value.to_be_bytes());
    }
}

fn font_family_name(document: &Document, font: &Dictionary, resource_name: &str) -> String {
    font_base_name(document, font)
        .map(|name| strip_subset_prefix(&name).to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| resource_name.to_string())
}

fn font_weight(document: &Document, font: &Dictionary) -> Option<u16> {
    let name = font_family_name(document, font, "");
    let inferred = infer_font_weight_from_name(&name);
    let descriptor_weight = font_descriptor(document, font)
        .and_then(|descriptor| descriptor.get(b"FontWeight").ok())
        .and_then(object_to_f32)
        .map(|value| value.round().clamp(1.0, 1000.0) as u16);
    descriptor_weight.or(inferred)
}

fn infer_font_weight_from_name(name: &str) -> Option<u16> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("extrablack") || lower.contains("ultrablack") {
        return Some(950);
    }
    if lower.contains("black") || lower.contains("heavy") {
        return Some(900);
    }
    if lower.contains("extrabold") || lower.contains("ultrabold") {
        return Some(800);
    }
    if lower.contains("semibold") || lower.contains("demibold") {
        return Some(600);
    }
    if lower.contains("bold") {
        return Some(700);
    }
    None
}

fn font_base_name(document: &Document, font: &Dictionary) -> Option<String> {
    if let Some(name) = font.get(b"BaseFont").ok().and_then(object_plain_text) {
        return Some(name);
    }

    let descendants = font
        .get(b"DescendantFonts")
        .ok()
        .and_then(|object| array_from_object(document, object))?;
    descendants
        .first()
        .and_then(|object| dictionary_from_object(document, object))
        .and_then(|dict| dict.get(b"BaseFont").ok())
        .and_then(object_plain_text)
}

fn array_from_object<'a>(document: &'a Document, object: &'a Object) -> Option<&'a Vec<Object>> {
    match object {
        Object::Reference(id) => document.get_object(*id).ok()?.as_array().ok(),
        Object::Array(array) => Some(array),
        _ => None,
    }
}

fn dictionary_from_object<'a>(
    document: &'a Document,
    object: &'a Object,
) -> Option<&'a Dictionary> {
    match object {
        Object::Reference(id) => document.get_dictionary(*id).ok(),
        Object::Dictionary(dictionary) => Some(dictionary),
        _ => None,
    }
}

fn dictionary_from_inline_object(object: &Object) -> Option<&Dictionary> {
    match object {
        Object::Dictionary(dictionary) => Some(dictionary),
        _ => None,
    }
}

fn cloned_dictionary_from_object(document: &Document, object: &Object) -> Option<Dictionary> {
    match object {
        Object::Reference(id) => document.get_dictionary(*id).ok().cloned(),
        Object::Dictionary(dictionary) => Some(dictionary.clone()),
        _ => None,
    }
}

fn stream_from_object<'a>(document: &'a Document, object: &'a Object) -> Option<&'a lopdf::Stream> {
    match object {
        Object::Reference(id) => document
            .get_object(*id)
            .ok()
            .and_then(|object| object.as_stream().ok()),
        Object::Stream(stream) => Some(stream),
        _ => None,
    }
}

fn strip_subset_prefix(name: &str) -> &str {
    let Some((prefix, rest)) = name.split_once('+') else {
        return name;
    };
    let is_subset_prefix =
        prefix.len() == 6 && prefix.bytes().all(|byte| byte.is_ascii_uppercase());
    if is_subset_prefix {
        rest
    } else {
        name
    }
}

fn sanitize_file_stem(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "font".to_string()
    } else {
        output
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
    identity_utf16: bool,
}

impl ToUnicodeMap {
    fn insert(&mut self, source: Vec<u8>, target: String) {
        if source.is_empty() {
            return;
        }
        self.max_code_len = self.max_code_len.max(source.len());
        if !target.is_empty() && !is_ascii_range_to_fullwidth(&source, &target) {
            self.reverse
                .entry(target.clone())
                .or_insert_with(|| source.clone());
        }
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
            } else if self.identity_utf16 && index + 1 < bytes.len() {
                output.push_str(&utf16be_to_string(&bytes[index..index + 2]));
                index += 2;
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
            if let Some(encoded) = self.reverse.get(&key) {
                output.extend_from_slice(encoded);
            } else if self.identity_utf16 {
                let unit = u16::try_from(character as u32).ok()?;
                output.extend_from_slice(&unit.to_be_bytes());
            } else {
                return None;
            }
        }
        Some(output)
    }

    fn supports_direct_utf16(&self) -> bool {
        self.identity_utf16
    }
}

fn parse_font_to_unicode(document: &Document, font: &Dictionary) -> Option<ToUnicodeMap> {
    let identity_utf16 = font
        .get(b"Encoding")
        .ok()
        .and_then(object_name_bytes)
        .as_deref()
        .is_some_and(uses_direct_utf16_encoding);

    if let Ok(to_unicode) = font.get(b"ToUnicode") {
        if let Some(stream) = match to_unicode {
            Object::Reference(id) => document
                .get_object(*id)
                .ok()
                .and_then(|o| o.as_stream().ok()),
            Object::Stream(s) => Some(s),
            _ => None,
        } {
            if let Ok(content) = stream.decompressed_content() {
                let mut map = parse_to_unicode_cmap(&content);
                map.identity_utf16 = identity_utf16;
                return Some(map);
            }
        }
    }

    if font
        .get(b"Subtype")
        .ok()
        .and_then(object_name_bytes)
        .as_deref()
        == Some("Type3")
    {
        let encoding = type3_encoding(font);
        if !encoding.is_empty() {
            let mut map = ToUnicodeMap::default();
            for (code, name) in encoding {
                if let Some(text) = glyph_name_to_unicode(&name) {
                    map.insert(vec![code], text);
                }
            }
            if !map.forward.is_empty() {
                return Some(map);
            }
        }
    }

    // Fallback: Check Encoding entry
    let mut use_win_ansi = !identity_utf16; // Default to WinAnsi for simple fonts without ToUnicode
    if let Ok(encoding_obj) = font.get(b"Encoding") {
        let encoding_name = match encoding_obj {
            Object::Name(name) => Some(name.as_slice()),
            Object::Reference(id) => document.get_object(*id).ok().and_then(|o| o.as_name().ok()),
            _ => None, // If it's a dict, it might be BaseEncoding WinAnsi
        };
        if let Some(name) = encoding_name {
            if uses_direct_utf16_encoding(&String::from_utf8_lossy(name)) {
                use_win_ansi = false;
            } else if name == b"MacRomanEncoding" {
                use_win_ansi = false; // We don't support MacRoman fallback yet
            }
        }
    }

    if use_win_ansi {
        let mut map = ToUnicodeMap::default();
        for code in 0..=255u8 {
            if let Some(unicode) = win_ansi_to_unicode(code) {
                if let Some(ch) = char::from_u32(unicode as u32) {
                    map.insert(vec![code], ch.to_string());
                }
            }
        }
        return Some(map);
    }

    if identity_utf16 {
        return Some(ToUnicodeMap {
            identity_utf16: true,
            ..ToUnicodeMap::default()
        });
    }

    None
}

fn uses_direct_utf16_encoding(name: &str) -> bool {
    // Identity-H/V fonts use glyph IDs as CIDs, not Unicode code points,
    // so direct UTF-16 encoding would produce wrong glyph indices.
    // Only CMaps that truly map CID = Unicode are valid here (e.g. UniGB-UCS2-H).
    (name.starts_with("Uni") && name.contains("UCS2"))
        || name.contains("UTF16")
}

fn is_ascii_range_to_fullwidth(source: &[u8], target: &str) -> bool {
    // Some simple (single-byte) fonts incorrectly map printable ASCII byte values to
    // CJK or fullwidth Unicode codepoints in their ToUnicode CMap — for example
    // 0x2C (',') → U+FF0C ('，'), 0x2E ('.') → U+3002 ('。').
    // The actual glyph is a narrow ASCII design, so using these bytes to encode a
    // fullwidth/CJK character would produce a narrow glyph.  Suppress such entries
    // from the reverse map so encoding falls back to the STSong-Light font which
    // carries the correct fullwidth glyphs.
    //
    // This check is intentionally restricted to single-byte sources.  In 2-byte CID
    // fonts (e.g. Adobe-GB1), small CID values like [0x00, 0x02] are legitimate
    // positions for CJK characters (顿号, 句号, etc.) with correct full-width metrics.
    // Suppressing those mappings would break advance-width calculation and re-encoding.
    //
    // The forward map (decoding) is intentionally left unchanged.
    if source.len() != 1 {
        return false;
    }
    let source_byte = source[0] as u32;
    // Only printable ASCII range (0x20–0x7E); control bytes are not ASCII glyph codes.
    (0x20..=0x7E).contains(&source_byte) && target.chars().all(is_cjk_or_fullwidth)
}

fn win_ansi_to_unicode(code: u8) -> Option<u16> {
    if code < 128 {
        return Some(code as u16);
    }
    match code {
        128 => Some(0x20AC),            // Euro
        130 => Some(0x201A),            // quotesinglbase
        131 => Some(0x0192),            // florin
        132 => Some(0x201E),            // quotedblbase
        133 => Some(0x2026),            // ellipsis
        134 => Some(0x2020),            // dagger
        135 => Some(0x2021),            // daggerdbl
        136 => Some(0x02C6),            // circumflex
        137 => Some(0x2030),            // perthousand
        138 => Some(0x0160),            // Scaron
        139 => Some(0x2039),            // guilsinglleft
        140 => Some(0x0152),            // OE
        142 => Some(0x017D),            // Zcaron
        145 => Some(0x2018),            // quoteleft
        146 => Some(0x2019),            // quoteright
        147 => Some(0x201C),            // quotedblleft
        148 => Some(0x201D),            // quotedblright
        149 => Some(0x2022),            // bullet
        150 => Some(0x2013),            // endash
        151 => Some(0x2014),            // emdash
        152 => Some(0x02DC),            // tilde
        153 => Some(0x2122),            // trademark
        154 => Some(0x0161),            // scaron
        155 => Some(0x203A),            // guilsinglright
        156 => Some(0x0153),            // oe
        158 => Some(0x017E),            // zcaron
        159 => Some(0x0178),            // Ydieresis
        160..=255 => Some(code as u16), // ISO-8859-1
        _ => None,
    }
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
    sanitize_cmap_unicode(&String::from_utf16_lossy(&bytes_to_u16_units(bytes)))
}

fn sanitize_cmap_unicode(value: &str) -> String {
    let cleaned = value
        .chars()
        .filter(|character| !matches!(character, '\0' | '\u{0001}'..='\u{0008}' | '\u{000B}' | '\u{000C}' | '\u{000E}'..='\u{001F}' | '\u{007F}'))
        .collect::<String>();
    normalize_compatibility_text(&cleaned)
}

fn normalize_compatibility_text(value: &str) -> String {
    value.chars().map(normalize_compatibility_char).collect()
}

fn normalize_compatibility_char(character: char) -> char {
    let codepoint = character as u32;
    if let Some(offset) = codepoint.checked_sub(0x2F00).filter(|offset| *offset < KANGXI_RADICAL_EQUIVALENTS.len() as u32) {
        return char::from_u32(KANGXI_RADICAL_EQUIVALENTS[offset as usize]).unwrap_or(character);
    }
    cjk_radical_supplement_equivalent(codepoint)
        .and_then(char::from_u32)
        .unwrap_or(character)
}

fn cjk_radical_supplement_equivalent(codepoint: u32) -> Option<u32> {
    Some(match codepoint {
        0x2E81 => 0x5382,
        0x2E82 => 0x4E5B,
        0x2E83 => 0x4E5A,
        0x2E84 => 0x4E59,
        0x2E85 => 0x4EBB,
        0x2E86 => 0x5182,
        0x2E87 => 0x20628,
        0x2E88 => 0x5200,
        0x2E89 => 0x5202,
        0x2E8A => 0x535C,
        0x2E8B => 0x353E,
        0x2E8C..=0x2E8D => 0x5C0F,
        0x2E8E => 0x5140,
        0x2E8F => 0x5C23,
        0x2E90 => 0x5C22,
        0x2E91 => 0x21BC2,
        0x2E92 => 0x5DF3,
        0x2E93 => 0x5E7A,
        0x2E94 => 0x5F51,
        0x2E95 => 0x2B739,
        0x2E96 => 0x5FC4,
        0x2E97 => 0x5FC3,
        0x2E98 => 0x624C,
        0x2E99 => 0x6535,
        0x2E9B => 0x65E1,
        0x2E9C => 0x65E5,
        0x2E9D => 0x6708,
        0x2E9E => 0x6B7A,
        0x2E9F => 0x6BCD,
        0x2EA0 => 0x6C11,
        0x2EA1 => 0x6C35,
        0x2EA2 => 0x6C3A,
        0x2EA3 => 0x706C,
        0x2EA4..=0x2EA5 => 0x722B,
        0x2EA6 => 0x4E2C,
        0x2EA7 => 0x725B,
        0x2EA8 => 0x72AD,
        0x2EA9 => 0x738B,
        0x2EAA => 0x24D14,
        0x2EAB => 0x76EE,
        0x2EAC => 0x793A,
        0x2EAD => 0x793B,
        0x2EAE => 0x25AD7,
        0x2EAF => 0x7CF9,
        0x2EB0 => 0x7E9F,
        0x2EB1 => 0x7F53,
        0x2EB2 => 0x7F52,
        0x2EB3 => 0x34C1,
        0x2EB4 => 0x5197,
        0x2EB5 => 0x2626B,
        0x2EB6 => 0x7F8A,
        0x2EB7 => 0x2634C,
        0x2EB8 => 0x2634B,
        0x2EB9 => 0x8002,
        0x2EBA => 0x8080,
        0x2EBB => 0x807F,
        0x2EBC => 0x8089,
        0x2EBD => 0x26951,
        0x2EBE..=0x2EC0 => 0x8279,
        0x2EC1 => 0x864E,
        0x2EC2 => 0x8864,
        0x2EC3 => 0x8980,
        0x2EC4 => 0x897F,
        0x2EC5 => 0x89C1,
        0x2EC6 => 0x89D2,
        0x2EC7 => 0x278B2,
        0x2EC8 => 0x8BA0,
        0x2EC9 => 0x8D1D,
        0x2ECA => 0x27FB7,
        0x2ECB => 0x8F66,
        0x2ECC..=0x2ECE => 0x8FB6,
        0x2ECF => 0x9091,
        0x2ED0 => 0x9485,
        0x2ED1 => 0x9577,
        0x2ED2 => 0x9578,
        0x2ED3 => 0x957F,
        0x2ED4 => 0x95E8,
        0x2ED5 => 0x28E0F,
        0x2ED6 => 0x961D,
        0x2ED7 => 0x96E8,
        0x2ED8 => 0x9752,
        0x2ED9 => 0x97E6,
        0x2EDA => 0x9875,
        0x2EDB => 0x98CE,
        0x2EDC => 0x98DE,
        0x2EDD => 0x98DF,
        0x2EDE => 0x2967F,
        0x2EDF => 0x98E0,
        0x2EE0 => 0x9963,
        0x2EE1 => 0x29810,
        0x2EE2 => 0x9A6C,
        0x2EE3 => 0x9AA8,
        0x2EE4 => 0x9B3C,
        0x2EE5 => 0x9C7C,
        0x2EE6 => 0x9E1F,
        0x2EE7 => 0x5364,
        0x2EE8 => 0x9EA6,
        0x2EE9 => 0x9EC4,
        0x2EEA => 0x9EFE,
        0x2EEB => 0x6589,
        0x2EEC => 0x9F50,
        0x2EED => 0x6B6F,
        0x2EEE => 0x9F7F,
        0x2EEF => 0x7ADC,
        0x2EF0 => 0x9F99,
        0x2EF1 => 0x9F9C,
        0x2EF2 => 0x4E80,
        0x2EF3 => 0x9F9F,
        _ => return None,
    })
}

const KANGXI_RADICAL_EQUIVALENTS: [u32; 214] = [
    0x4E00, 0x4E28, 0x4E36, 0x4E3F, 0x4E59, 0x4E85, 0x4E8C, 0x4EA0,
    0x4EBA, 0x513F, 0x5165, 0x516B, 0x5182, 0x5196, 0x51AB, 0x51E0,
    0x51F5, 0x5200, 0x529B, 0x52F9, 0x5315, 0x531A, 0x5338, 0x5341,
    0x535C, 0x5369, 0x5382, 0x53B6, 0x53C8, 0x53E3, 0x56D7, 0x571F,
    0x58EB, 0x5902, 0x590A, 0x5915, 0x5927, 0x5973, 0x5B50, 0x5B80,
    0x5BF8, 0x5C0F, 0x5C22, 0x5C38, 0x5C6E, 0x5C71, 0x5DDB, 0x5DE5,
    0x5DF1, 0x5DFE, 0x5E72, 0x5E7A, 0x5E7F, 0x5EF4, 0x5EFE, 0x5F0B,
    0x5F13, 0x5F50, 0x5F61, 0x5F73, 0x5FC3, 0x6208, 0x6236, 0x624B,
    0x652F, 0x6534, 0x6587, 0x6597, 0x65A4, 0x65B9, 0x65E0, 0x65E5,
    0x66F0, 0x6708, 0x6728, 0x6B20, 0x6B62, 0x6B79, 0x6BB3, 0x6BCB,
    0x6BD4, 0x6BDB, 0x6C0F, 0x6C14, 0x6C34, 0x706B, 0x722A, 0x7236,
    0x723B, 0x723F, 0x7247, 0x7259, 0x725B, 0x72AC, 0x7384, 0x7389,
    0x74DC, 0x74E6, 0x7518, 0x751F, 0x7528, 0x7530, 0x758B, 0x7592,
    0x7676, 0x767D, 0x76AE, 0x76BF, 0x76EE, 0x77DB, 0x77E2, 0x77F3,
    0x793A, 0x79B8, 0x79BE, 0x7A74, 0x7ACB, 0x7AF9, 0x7C73, 0x7CF8,
    0x7F36, 0x7F51, 0x7F8A, 0x7FBD, 0x8001, 0x800C, 0x8012, 0x8033,
    0x807F, 0x8089, 0x81E3, 0x81EA, 0x81F3, 0x81FC, 0x820C, 0x821B,
    0x821F, 0x826E, 0x8272, 0x8278, 0x864D, 0x866B, 0x8840, 0x884C,
    0x8863, 0x897E, 0x898B, 0x89D2, 0x8A00, 0x8C37, 0x8C46, 0x8C55,
    0x8C78, 0x8C9D, 0x8D64, 0x8D70, 0x8DB3, 0x8EAB, 0x8ECA, 0x8F9B,
    0x8FB0, 0x8FB5, 0x9091, 0x9149, 0x91C6, 0x91CC, 0x91D1, 0x9577,
    0x9580, 0x961C, 0x96B6, 0x96B9, 0x96E8, 0x9751, 0x975E, 0x9762,
    0x9769, 0x97CB, 0x97ED, 0x97F3, 0x9801, 0x98A8, 0x98DB, 0x98DF,
    0x9996, 0x9999, 0x99AC, 0x9AA8, 0x9AD8, 0x9ADF, 0x9B25, 0x9B2F,
    0x9B32, 0x9B3C, 0x9B5A, 0x9CE5, 0x9E75, 0x9E7F, 0x9EA5, 0x9EBB,
    0x9EC3, 0x9ECD, 0x9ED1, 0x9EF9, 0x9EFD, 0x9F0E, 0x9F13, 0x9F20,
    0x9F3B, 0x9F4A, 0x9F52, 0x9F8D, 0x9F9C, 0x9FA0,
];

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

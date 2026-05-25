use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
pub struct PageIndex(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PdfObjectId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct TextObjectId(pub PdfObjectId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct ImageObjectId(pub PdfObjectId);

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            origin: Point::new(x, y),
            size: Size::new(width, height),
        }
    }

    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.origin.x
            && point.y >= self.origin.y
            && point.x <= self.origin.x + self.size.width
            && point.y <= self.origin.y + self.size.height
    }

    pub fn translated(&self, delta: Point) -> Self {
        Self {
            origin: Point::new(self.origin.x + delta.x, self.origin.y + delta.y),
            size: self.size,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Self = Self::rgba(0, 0, 0, 255);

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct PageInfo {
    pub index: PageIndex,
    pub size: Size,
    #[serde(default)]
    pub rotation: i32,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TextRun {
    pub content: String,
    pub font_name: Option<String>,
    pub font_size: f32,
    pub color: Color,
}

impl TextRun {
    pub fn new(content: String, font_name: Option<String>, font_size: f32, color: Color) -> Self {
        Self {
            content,
            font_name,
            font_size,
            color,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TextObject {
    pub id: TextObjectId,
    pub page: PageIndex,
    pub bounds: Rect,
    pub content: String,
    pub font_name: Option<String>,
    pub font_size: f32,
    pub color: Color,
    pub runs: Vec<TextRun>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ImageObject {
    pub id: ImageObjectId,
    pub page: PageIndex,
    pub bounds: Rect,
    pub format: String,
    pub byte_len: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedPage {
    pub page: PageIndex,
    pub width_px: u32,
    pub height_px: u32,
    pub scale: f32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct PageStructure {
    pub page: PageInfo,
    pub text: Vec<StructuredTextObject>,
    #[serde(default)]
    pub visual_text: Vec<StructuredVisualTextObject>,
    pub images: Vec<StructuredImageObject>,
    pub watermarks: Vec<StructuredWatermark>,
    pub annotations: Vec<StructuredAnnotation>,
    pub bookmarks: Vec<BookmarkItem>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct StructuredTextObject {
    pub id: TextObjectId,
    pub bounds: Rect,
    pub content: String,
    pub font_name: Option<String>,
    pub font_size: f32,
    pub color: Color,
    #[serde(default = "default_stroke_color")]
    pub stroke_color: Color,
    #[serde(default)]
    pub stroke_width: f32,
    #[serde(default)]
    pub rendering_mode: i32,
    #[serde(default)]
    pub char_spacing: f32,
    #[serde(default)]
    pub word_spacing: f32,
    #[serde(default = "default_horizontal_scaling")]
    pub horizontal_scaling: f32,
    pub transform: [f32; 6],
    pub angle_degrees: f32,
    pub z_index: usize,
    #[serde(default)]
    pub glyphs: Vec<LayoutGlyph>,
    pub runs: Vec<TextRun>,
    /// True when the font defines reduced advance widths for fullwidth CJK punctuation
    /// (e.g. "，" stored as 500/1000 units instead of the default 1000/1000).
    /// Consumers should rely on per-glyph `advance` values rather than assuming
    /// uniform full-width spacing for such objects.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub punct_width_squeeze: bool,
    /// OpenType layout feature tags detected in the font binary (e.g. "palt", "kern", "liga").
    /// Only the subset of [palt, halt, kern, liga, fwid, hwid] that are present is listed.
    /// Empty when the font has none of these features or is not an SFNT font.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub font_features: Vec<String>,
}

const fn default_stroke_color() -> Color {
    Color::BLACK
}

const fn default_horizontal_scaling() -> f32 {
    100.0
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct StructuredVisualTextObject {
    pub id: TextObjectId,
    pub bounds: Rect,
    pub font_name: Option<String>,
    pub font_size: f32,
    pub transform: [f32; 6],
    pub angle_degrees: f32,
    pub z_index: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct StructuredImageObject {
    pub id: ImageObjectId,
    pub name: Option<String>,
    pub source_file: Option<String>,
    pub bounds: Rect,
    pub transform: [f32; 6],
    pub angle_degrees: f32,
    pub width_px: Option<u32>,
    pub height_px: Option<u32>,
    pub color_space: Option<String>,
    pub bits_per_component: Option<u8>,
    pub filters: Vec<String>,
    pub byte_len: usize,
    pub z_index: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct HitTestResult {
    pub object_id: PdfObjectId,
    pub object_type: String,
    pub page: PageIndex,
    pub local_position: Point,
    pub text_run_index: Option<usize>,
    pub glyph_index: Option<usize>,
    pub bbox: Rect,
    pub matrix: [f32; 6],
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct LayoutGlyph {
    pub ch: String,
    pub glyph_id: Option<u32>,
    pub font_name: Option<String>,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    pub width: f32,
    pub bbox: Rect,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TextEditSessionInfo {
    pub object_id: TextObjectId,
    pub page: PageIndex,
    pub original_text: String,
    #[serde(default)]
    pub group_object_ids: Vec<TextObjectId>,
    pub bbox: Rect,
    pub matrix: [f32; 6],
    pub font_id: Option<String>,
    pub font_size: f32,
    pub writing_mode: Option<String>,
    pub glyphs: Vec<LayoutGlyph>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TextLayoutPreview {
    pub object_id: TextObjectId,
    pub text: String,
    #[serde(default)]
    pub group_object_ids: Vec<TextObjectId>,
    pub glyphs: Vec<LayoutGlyph>,
    pub bbox: Rect,
    pub overflow: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct StructuredAnnotation {
    pub id: Option<PdfObjectId>,
    pub subtype: Option<String>,
    pub bounds: Option<Rect>,
    pub contents: Option<String>,
    pub name: Option<String>,
    pub flags: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct StructuredWatermark {
    pub kind: String,
    pub object_id: PdfObjectId,
    pub bounds: Rect,
    pub content: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct BookmarkItem {
    pub title: String,
    pub page: Option<PageIndex>,
    pub level: usize,
}

impl RenderedPage {
    pub fn estimated_bytes(&self) -> usize {
        self.rgba.len()
    }
}

use crate::{
    Color, CoreError, CoreResult, EngineDocument, ImageObject, PageIndex, PageInfo, PdfEngine,
    PdfObjectId, Rect, RenderedPage, Size, TextObject, TextObjectId, TextRun, TextStyle,
};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct LopdfEngine;

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
        Ok(Vec::new())
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
}

#[derive(Debug, Clone)]
struct TextParseState {
    x: f32,
    y: f32,
    font_name: Option<String>,
    font_size: f32,
    color: Color,
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

fn object_name(object: &Object) -> Option<String> {
    match object {
        Object::Name(value) => Some(String::from_utf8_lossy(value).into_owned()),
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

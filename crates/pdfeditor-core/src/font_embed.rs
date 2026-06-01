#![allow(dead_code)] // public API — used from wasm_api.rs via the wasm feature flag
use flate2::read::ZlibDecoder;
use std::collections::HashMap;
use std::io::Read;

/// Parsed data from an embedded TrueType font used through Identity-H.
#[derive(Clone)]
pub struct CjkFontData {
    pub sfnt_bytes: Vec<u8>,
    /// Unicode char → glyph ID (from the font's cmap).
    pub unicode_to_gid: HashMap<char, u16>,
    /// GID → horizontal advance in font units (indexed by GID, length = number_of_glyphs).
    pub gid_to_advance: Vec<u16>,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
}

pub fn sfnt_is_truetype(sfnt: &[u8]) -> bool {
    matches!(sfnt.get(0..4), Some([0x00, 0x01, 0x00, 0x00]) | Some(b"true"))
}

pub fn font_bytes_to_truetype_sfnt(bytes: &[u8]) -> Option<Vec<u8>> {
    let sfnt = woff1_to_sfnt(bytes).unwrap_or_else(|| bytes.to_vec());
    if sfnt_is_truetype(&sfnt) {
        return Some(sfnt);
    }
    if sfnt.get(0..4) == Some(b"ttcf") {
        return ttc_first_truetype_face_to_sfnt(&sfnt);
    }
    None
}

fn ttc_first_truetype_face_to_sfnt(ttc: &[u8]) -> Option<Vec<u8>> {
    if ttc.len() < 12 || ttc.get(0..4)? != b"ttcf" {
        return None;
    }
    let num_fonts = read_u32(ttc, 8)? as usize;
    if num_fonts == 0 || 12 + num_fonts * 4 > ttc.len() {
        return None;
    }

    for index in 0..num_fonts {
        let offset = read_u32(ttc, 12 + index * 4)? as usize;
        if let Some(sfnt) = extract_sfnt_at(ttc, offset) {
            if sfnt_is_truetype(&sfnt) {
                return Some(sfnt);
            }
        }
    }
    None
}

fn extract_sfnt_at(data: &[u8], font_offset: usize) -> Option<Vec<u8>> {
    if font_offset + 12 > data.len() {
        return None;
    }
    let flavor = data.get(font_offset..font_offset + 4)?;
    if !matches!(flavor, [0x00, 0x01, 0x00, 0x00] | b"true") {
        return None;
    }
    let num_tables = read_u16(data, font_offset + 4)? as usize;
    let dir_end = font_offset + 12 + num_tables * 16;
    if dir_end > data.len() {
        return None;
    }

    struct TableEntry {
        tag: [u8; 4],
        checksum: u32,
        offset: usize,
        length: usize,
    }

    let mut entries = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let base = font_offset + 12 + i * 16;
        let tag = data.get(base..base + 4)?.try_into().ok()?;
        let checksum = read_u32(data, base + 4)?;
        let offset = read_u32(data, base + 8)? as usize;
        let length = read_u32(data, base + 12)? as usize;
        if offset.checked_add(length)? > data.len() {
            return None;
        }
        entries.push(TableEntry {
            tag,
            checksum,
            offset,
            length,
        });
    }

    let header_size = 12 + num_tables * 16;
    let first_table_start = align4(header_size);
    let mut table_offsets = Vec::with_capacity(num_tables);
    let mut cursor = first_table_start;
    for entry in &entries {
        table_offsets.push(cursor);
        cursor = cursor.checked_add(align4(entry.length))?;
    }

    let mut sfnt = vec![0u8; cursor];
    sfnt[0..4].copy_from_slice(flavor);
    let n = num_tables as u16;
    sfnt[4..6].copy_from_slice(&n.to_be_bytes());
    let max_pow2 = if n == 0 {
        0u16
    } else {
        1u16 << (15 - n.leading_zeros())
    };
    let search_range = max_pow2 * 16;
    let entry_selector = max_pow2.trailing_zeros() as u16;
    let range_shift = n * 16 - search_range;
    sfnt[6..8].copy_from_slice(&search_range.to_be_bytes());
    sfnt[8..10].copy_from_slice(&entry_selector.to_be_bytes());
    sfnt[10..12].copy_from_slice(&range_shift.to_be_bytes());

    for (i, entry) in entries.iter().enumerate() {
        let base = 12 + i * 16;
        sfnt[base..base + 4].copy_from_slice(&entry.tag);
        sfnt[base + 4..base + 8].copy_from_slice(&entry.checksum.to_be_bytes());
        sfnt[base + 8..base + 12].copy_from_slice(&(table_offsets[i] as u32).to_be_bytes());
        sfnt[base + 12..base + 16].copy_from_slice(&(entry.length as u32).to_be_bytes());
        sfnt[table_offsets[i]..table_offsets[i] + entry.length]
            .copy_from_slice(&data[entry.offset..entry.offset + entry.length]);
    }

    Some(sfnt)
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(offset..offset + 2)?.try_into().ok()?))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(offset..offset + 4)?.try_into().ok()?))
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

impl CjkFontData {
    /// Advance width for a GID, normalised to 1/1000-em units.
    pub fn advance_thousandths(&self, gid: u16) -> u32 {
        let raw = self
            .gid_to_advance
            .get(gid as usize)
            .copied()
            .unwrap_or(1000);
        (raw as u64 * 1000 / self.units_per_em.max(1) as u64) as u32
    }

    /// Encode a Unicode character to the 2-byte GID that Identity-H expects.
    /// Returns `None` if the character is not present in this font.
    pub fn encode_char(&self, ch: char) -> Option<[u8; 2]> {
        let gid = *self.unicode_to_gid.get(&ch)?;
        Some(gid.to_be_bytes())
    }
}

/// Decode a WOFF1 file into raw sfnt (TrueType/OpenType) bytes.
///
/// WOFF1 spec: <https://www.w3.org/TR/WOFF/>
pub fn woff1_to_sfnt(woff: &[u8]) -> Option<Vec<u8>> {
    if woff.len() < 44 {
        return None;
    }
    if woff[0..4] != [0x77, 0x4F, 0x46, 0x46] {
        return None; // not WOFF1 magic
    }

    let sfnt_flavor = &woff[4..8]; // 0x00010000 = TrueType, "OTTO" = CFF
    let num_tables = u16::from_be_bytes([woff[12], woff[13]]) as usize;

    let dir_end = 44 + num_tables * 20;
    if dir_end > woff.len() {
        return None;
    }

    struct WoffEntry {
        tag: [u8; 4],
        offset: usize,
        comp_len: usize,
        orig_len: usize,
        checksum: u32,
    }

    let mut entries = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let b = 44 + i * 20;
        let tag = [woff[b], woff[b + 1], woff[b + 2], woff[b + 3]];
        let offset = u32::from_be_bytes([woff[b + 4], woff[b + 5], woff[b + 6], woff[b + 7]]) as usize;
        let comp_len = u32::from_be_bytes([woff[b + 8], woff[b + 9], woff[b + 10], woff[b + 11]]) as usize;
        let orig_len = u32::from_be_bytes([woff[b + 12], woff[b + 13], woff[b + 14], woff[b + 15]]) as usize;
        let checksum = u32::from_be_bytes([woff[b + 16], woff[b + 17], woff[b + 18], woff[b + 19]]);
        entries.push(WoffEntry { tag, offset, comp_len, orig_len, checksum });
    }

    // Compute sfnt table offsets (4-byte aligned, after the sfnt header).
    let sfnt_header_size = 12 + num_tables * 16;
    let first_table_start = (sfnt_header_size + 3) & !3;
    let mut table_offsets = Vec::with_capacity(num_tables);
    let mut cursor = first_table_start;
    for entry in &entries {
        table_offsets.push(cursor);
        cursor += (entry.orig_len + 3) & !3;
    }
    let total_size = cursor;

    let mut sfnt = vec![0u8; total_size];

    // sfnt offset table (12 bytes)
    sfnt[0..4].copy_from_slice(sfnt_flavor);
    let n = num_tables as u16;
    sfnt[4..6].copy_from_slice(&n.to_be_bytes());
    // searchRange = (2^floor(log2(n))) * 16
    let max_pow2 = if n == 0 {
        0u16
    } else {
        1u16 << (15 - n.leading_zeros())
    };
    let search_range = max_pow2 * 16;
    let entry_selector = max_pow2.trailing_zeros() as u16;
    let range_shift = n * 16 - search_range;
    sfnt[6..8].copy_from_slice(&search_range.to_be_bytes());
    sfnt[8..10].copy_from_slice(&entry_selector.to_be_bytes());
    sfnt[10..12].copy_from_slice(&range_shift.to_be_bytes());

    // sfnt table directory (16 bytes each)
    for (i, entry) in entries.iter().enumerate() {
        let base = 12 + i * 16;
        sfnt[base..base + 4].copy_from_slice(&entry.tag);
        sfnt[base + 4..base + 8].copy_from_slice(&entry.checksum.to_be_bytes());
        sfnt[base + 8..base + 12].copy_from_slice(&(table_offsets[i] as u32).to_be_bytes());
        sfnt[base + 12..base + 16].copy_from_slice(&(entry.orig_len as u32).to_be_bytes());

        // Table data
        let end = entry.offset + entry.comp_len;
        if end > woff.len() {
            return None;
        }
        let woff_data = &woff[entry.offset..end];
        let table_data: Vec<u8> = if entry.comp_len < entry.orig_len {
            // zlib compressed
            let mut dec = ZlibDecoder::new(woff_data);
            let mut buf = Vec::with_capacity(entry.orig_len);
            dec.read_to_end(&mut buf).ok()?;
            if buf.len() != entry.orig_len {
                return None;
            }
            buf
        } else {
            woff_data.to_vec()
        };

        sfnt[table_offsets[i]..table_offsets[i] + entry.orig_len].copy_from_slice(&table_data);
        // padding bytes remain zero from the initialised vec
    }

    Some(sfnt)
}

/// Parse a raw sfnt (TrueType) byte slice into a [`CjkFontData`].
pub fn parse_cjk_font(sfnt_bytes: Vec<u8>) -> Option<CjkFontData> {
    let face = ttf_parser::Face::parse(&sfnt_bytes, 0).ok()?;

    let units_per_em = face.units_per_em();
    let ascender = face.ascender();
    let descender = face.descender();
    let bbox = face.tables().head.global_bbox;

    // Build Unicode → GID map from the first Unicode cmap subtable.
    let mut unicode_to_gid: HashMap<char, u16> = HashMap::new();
    if let Some(cmap) = face.tables().cmap {
        for subtable in cmap.subtables {
            if !subtable.is_unicode() {
                continue;
            }
            subtable.codepoints(|cp| {
                if let Some(ch) = char::from_u32(cp) {
                    if let Some(gid) = face.glyph_index(ch) {
                        unicode_to_gid.entry(ch).or_insert(gid.0);
                    }
                }
            });
            break; // use the first Unicode subtable only
        }
    }

    if unicode_to_gid.is_empty() {
        return None;
    }

    // Advance widths indexed by GID.
    let num_glyphs = face.number_of_glyphs() as usize;
    let mut gid_to_advance = vec![units_per_em; num_glyphs];
    for i in 0..num_glyphs {
        if let Some(adv) = face.glyph_hor_advance(ttf_parser::GlyphId(i as u16)) {
            gid_to_advance[i] = adv;
        }
    }

    Some(CjkFontData {
        sfnt_bytes,
        unicode_to_gid,
        gid_to_advance,
        units_per_em,
        ascender,
        descender,
        x_min: bbox.x_min,
        y_min: bbox.y_min,
        x_max: bbox.x_max,
        y_max: bbox.y_max,
    })
}

/// Build the compact PDF `/W` width array for a CIDFont.
///
/// Format: `[firstGid [w0 w1 …] firstGid [w0 w1 …] …]`
/// We only include GIDs whose advance differs from the default (1000).
pub fn build_w_array(data: &CjkFontData) -> Vec<lopdf::Object> {
    let default_w: u32 = 1000;
    let mut out: Vec<lopdf::Object> = Vec::new();

    let mut run_start: Option<usize> = None;
    let mut run: Vec<lopdf::Object> = Vec::new();

    let flush = |out: &mut Vec<lopdf::Object>, run_start: &mut Option<usize>, run: &mut Vec<lopdf::Object>| {
        if let Some(start) = run_start.take() {
            out.push(lopdf::Object::Integer(start as i64));
            out.push(lopdf::Object::Array(std::mem::take(run)));
        }
    };

    for (gid, &_advance) in data.gid_to_advance.iter().enumerate() {
        let w = data.advance_thousandths(gid as u16);
        if w != default_w {
            if run_start.is_none() {
                run_start = Some(gid);
            }
            run.push(lopdf::Object::Integer(w as i64));
        } else {
            flush(&mut out, &mut run_start, &mut run);
        }
    }
    flush(&mut out, &mut run_start, &mut run);
    let _ = default_w; // suppress warning
    out
}

/// Build a compact ToUnicode CMap stream covering only the supplied (gid → unicode) pairs.
pub fn build_to_unicode_cmap(gid_to_unicode: &[(u16, char)]) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n\
12 dict begin\nbegincmap\n\
/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def\n\
/CMapName /Adobe-Identity-UCS def\n\
/CMapType 2 def\n\
1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );

    for chunk in gid_to_unicode.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (gid, ch) in chunk {
            // Each Unicode BMP char is one UTF-16 code unit.
            let unit = *ch as u32;
            if unit <= 0xFFFF {
                cmap.push_str(&format!("<{gid:04X}> <{unit:04X}>\n"));
            } else {
                // Supplementary plane: encode as surrogate pair.
                let code = unit - 0x10000;
                let hi = 0xD800 + (code >> 10);
                let lo = 0xDC00 + (code & 0x3FF);
                cmap.push_str(&format!("<{gid:04X}> <{hi:04X}{lo:04X}>\n"));
            }
        }
        cmap.push_str("endbfchar\n");
    }

    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend");
    cmap
}

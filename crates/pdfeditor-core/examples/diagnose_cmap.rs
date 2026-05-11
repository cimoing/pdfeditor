use pdfeditor_core::{page_font_assets_from_pdf_bytes, PageIndex};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        r"d:\Src\pdfeditor\target\debug\in2.pdf".to_string()
    });
    let bytes = std::fs::read(&path).expect("failed to read PDF");
    
    let assets = page_font_assets_from_pdf_bytes(&bytes, PageIndex(0)).unwrap();
    for asset in assets {
        if asset.format == "opentype" {
            println!("Font: {}", asset.resource_name);
            // Let's dump the cmap table of the generated OTF
            if let Some(cmap_data) = find_table(&asset.bytes, b"cmap") {
                println!("  cmap size: {}", cmap_data.len());
                // Let's look for U+2019 (quoteright)
                // Format 4 cmap parsing is complex, let's just see if 20 19 is in there
                // We'll just print out a few things.
            }
        }
    }
}

fn find_table<'a>(data: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    if data.len() < 12 { return None; }
    let num_tables = u16::from_be_bytes([data[4], data[5]]) as usize;
    for i in 0..num_tables {
        let offset = 12 + i * 16;
        if offset + 16 > data.len() { break; }
        if &data[offset..offset+4] == tag {
            let table_offset = u32::from_be_bytes([data[offset+8], data[offset+9], data[offset+10], data[offset+11]]) as usize;
            let table_length = u32::from_be_bytes([data[offset+12], data[offset+13], data[offset+14], data[offset+15]]) as usize;
            return data.get(table_offset..table_offset + table_length);
        }
    }
    None
}

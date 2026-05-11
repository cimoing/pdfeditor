use pdfeditor_core::{page_font_assets_from_pdf_bytes, PageIndex};
use lopdf::Document;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        r"d:\Src\pdfeditor\target\debug\in2.pdf".to_string()
    });
    let bytes = std::fs::read(&path).expect("failed to read PDF");
    let doc = Document::load_mem(&bytes).expect("failed to parse pdf");
    
    for (page_num, page_id) in doc.get_pages().into_iter() {
        let Ok(fonts) = doc.get_page_fonts(page_id) else { continue };
        println!("=== Page {} ===", page_num);
        for (name, font) in fonts {
            let name_str = String::from_utf8_lossy(&name);
            println!("Font: {}", name_str);
            
            if let Ok(to_unicode_obj) = font.get(b"ToUnicode") {
                let stream = match to_unicode_obj {
                    lopdf::Object::Reference(id) => doc.get_object(*id).unwrap().as_stream().unwrap(),
                    lopdf::Object::Stream(s) => s,
                    _ => continue,
                };
                let content = stream.decompressed_content().unwrap();
                let text = String::from_utf8_lossy(&content);
                println!("ToUnicode CMap:");
                for line in text.lines().take(20) {
                    println!("  {}", line);
                }
                println!("  ...");
            } else {
                println!("  No ToUnicode map");
            }
        }
        break; // just check first page
    }
}

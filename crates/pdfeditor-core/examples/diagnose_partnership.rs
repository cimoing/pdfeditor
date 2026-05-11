use pdfeditor_core::{page_structure_from_pdf_bytes, PageIndex};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| r"d:\Src\pdfeditor\target\debug\in2.pdf".to_string());
    let bytes = std::fs::read(&path).expect("failed to read PDF");
    let structure =
        page_structure_from_pdf_bytes(&bytes, PageIndex(0)).expect("failed to get structure");

    // look for Partnership in the structure
    for text in &structure.text {
        if text.content.contains("Partnership") {
            println!("Found text object: {:?}", text);
        }
    }
}

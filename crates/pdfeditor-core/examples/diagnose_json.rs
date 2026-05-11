use pdfeditor_core::{PageIndex, page_structure_from_pdf_bytes};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        r"d:\Src\pdfeditor\target\debug\in2.pdf".to_string()
    });
    let bytes = std::fs::read(&path).expect("failed to read PDF");
    let structure = page_structure_from_pdf_bytes(&bytes, PageIndex(0)).expect("failed to get structure");
    let json = format!("{:#?}", structure);
    
    let lines: Vec<&str> = json.lines().filter(|l| l.contains("LLC")).collect();
    for line in lines.iter().take(5) {
        println!("Found: {}", line);
    }
}

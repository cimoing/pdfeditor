use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream, StringFormat};
use pdfeditor_core::{
    Color, DocumentSession, LopdfEngine, MockPdfEngine, OpenOptions, PageBitmapCache, PageIndex,
    Point, Rect, RenderedPage, ResourceBudget, SaveOptions, TextRun, TextStyle,
};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn opens_minimal_pdf_and_reports_pages() {
    let path = write_temp_pdf("open");
    let engine = MockPdfEngine;
    let session = DocumentSession::open(&engine, &path, OpenOptions::default()).unwrap();

    assert_eq!(session.page_count(), 1);
    assert_eq!(session.page_info(PageIndex(0)).unwrap().index, PageIndex(0));
    assert!(!session.text_objects(PageIndex(0)).unwrap().is_empty());
}

#[test]
fn records_text_edits_and_clears_dirty_state_after_save() {
    let source = write_temp_pdf("edit");
    let target = temp_path("saved", "pdf");
    let engine = MockPdfEngine;
    let mut session = DocumentSession::open(&engine, &source, OpenOptions::default()).unwrap();

    session
        .add_text(
            PageIndex(0),
            Rect::new(10.0, 10.0, 100.0, 24.0),
            "hello".to_string(),
            TextStyle {
                font_name: Some("Helvetica".to_string()),
                font_size: 12.0,
                color: Color::BLACK,
            },
        )
        .unwrap();

    assert!(session.is_dirty());
    assert_eq!(session.edits().pending().len(), 1);

    session
        .save_as(&target, SaveOptions { overwrite: true })
        .unwrap();

    assert!(!session.is_dirty());
    assert!(target.exists());
}

#[test]
fn undo_redo_respects_resource_budget() {
    let source = write_temp_pdf("undo");
    let engine = MockPdfEngine;
    let mut session = DocumentSession::open(
        &engine,
        &source,
        OpenOptions {
            resource_budget: ResourceBudget {
                undo_steps: 1,
                ..ResourceBudget::LOW_RESOURCE
            },
        },
    )
    .unwrap();

    let style = TextStyle {
        font_name: None,
        font_size: 10.0,
        color: Color::BLACK,
    };

    session
        .add_text(
            PageIndex(0),
            Rect::new(0.0, 0.0, 10.0, 10.0),
            "a".into(),
            style.clone(),
        )
        .unwrap();
    session
        .add_text(
            PageIndex(0),
            Rect::new(0.0, 0.0, 10.0, 10.0),
            "b".into(),
            style,
        )
        .unwrap();

    assert_eq!(session.edits().pending().len(), 1);
    assert!(session.edits_mut().undo().is_some());
    assert_eq!(session.edits().pending().len(), 0);
    assert!(session.edits_mut().redo().is_some());
    assert_eq!(session.edits().pending().len(), 1);
}

#[test]
fn page_cache_evicts_old_entries() {
    let mut cache = PageBitmapCache::new(16);
    cache.insert(rendered(PageIndex(0), 8));
    cache.insert(rendered(PageIndex(1), 8));
    cache.insert(rendered(PageIndex(2), 8));

    assert_eq!(cache.stats().entries, 2);
    assert!(cache.get(PageIndex(0)).is_none());
    assert!(cache.get(PageIndex(1)).is_some());
    assert!(cache.get(PageIndex(2)).is_some());
}

#[test]
fn render_uses_cache_budget() {
    let source = write_temp_pdf("render");
    let engine = MockPdfEngine;
    let mut session = DocumentSession::open(
        &engine,
        &source,
        OpenOptions {
            resource_budget: ResourceBudget {
                page_cache_bytes: 4 * 1024 * 1024,
                ..ResourceBudget::LOW_RESOURCE
            },
        },
    )
    .unwrap();

    let rendered = session.render_page(PageIndex(0), 1.0).unwrap();
    assert_eq!(rendered.page, PageIndex(0));
    assert_eq!(session.cache_stats().entries, 1);
}

#[test]
fn hit_tests_and_updates_existing_pdf_text() {
    let source = write_temp_pdf("update-text");
    let target = temp_path("updated-text", "pdf");
    let engine = MockPdfEngine;
    let mut session = DocumentSession::open(&engine, &source, OpenOptions::default()).unwrap();

    let hit = session
        .hit_test_text(PageIndex(0), Point::new(80.0, 80.0))
        .unwrap()
        .expect("expected sample text object");

    let updated = session
        .update_text(hit.id, "Updated title".to_string(), None)
        .unwrap();

    assert_eq!(updated.content, "Updated title");
    assert!(session.is_dirty());

    let page_text = session.text_objects(PageIndex(0)).unwrap();
    assert!(page_text
        .iter()
        .any(|object| object.id == hit.id && object.content == "Updated title"));

    session
        .save_as(&target, SaveOptions { overwrite: true })
        .unwrap();

    let saved = fs::read_to_string(target).unwrap();
    assert!(saved.contains("content=Updated title"));
}

#[test]
fn rejects_text_updates_that_exceed_original_bounds() {
    let source = write_temp_pdf("update-text-bounds");
    let engine = MockPdfEngine;
    let mut session = DocumentSession::open(&engine, &source, OpenOptions::default()).unwrap();

    let hit = session
        .hit_test_text(PageIndex(0), Point::new(80.0, 80.0))
        .unwrap()
        .expect("expected sample text object");

    let original_content = hit.content.clone();
    let too_long = "This replacement text is intentionally far too long to fit inside the existing PDF text rectangle, so it must be rejected by the core layer before the engine mutates anything.";

    let result = session.update_text(hit.id, too_long.to_string(), None);

    assert!(result.is_err());
    assert!(!session.is_dirty());
    assert_eq!(session.edits().pending().len(), 0);

    let unchanged = session
        .text_objects(PageIndex(0))
        .unwrap()
        .into_iter()
        .find(|object| object.id == hit.id)
        .unwrap();
    assert_eq!(unchanged.content, original_content);
}

#[test]
fn updates_text_bounds_before_replacing_longer_text() {
    let source = write_temp_pdf("update-text-bounds-first");
    let engine = MockPdfEngine;
    let mut session = DocumentSession::open(&engine, &source, OpenOptions::default()).unwrap();

    let hit = session
        .hit_test_text(PageIndex(0), Point::new(80.0, 80.0))
        .unwrap()
        .expect("expected sample text object");

    let long_text = "This replacement fits after expanding the text object bounds";
    assert!(session
        .update_text(hit.id, long_text.to_string(), None)
        .is_err());

    session
        .update_text_bounds(hit.id, Rect::new(72.0, 72.0, 800.0, 80.0))
        .unwrap();
    let updated = session
        .update_text(hit.id, long_text.to_string(), None)
        .unwrap();

    assert_eq!(updated.content, long_text);
    assert_eq!(updated.bounds, Rect::new(72.0, 72.0, 800.0, 80.0));
}

#[test]
fn updates_existing_text_with_multiple_styles() {
    let source = write_temp_pdf("update-text-runs");
    let target = temp_path("updated-text-runs", "pdf");
    let engine = MockPdfEngine;
    let mut session = DocumentSession::open(&engine, &source, OpenOptions::default()).unwrap();

    let hit = session
        .hit_test_text(PageIndex(0), Point::new(80.0, 80.0))
        .unwrap()
        .expect("expected sample text object");

    let runs = vec![
        TextRun::new(
            "New ".to_string(),
            Some("Helvetica".to_string()),
            12.0,
            Color::BLACK,
        ),
        TextRun::new(
            "Title".to_string(),
            Some("Helvetica-Bold".to_string()),
            14.0,
            Color::rgba(180, 0, 0, 255),
        ),
    ];

    let updated = session.update_text_runs(hit.id, runs).unwrap();

    assert_eq!(updated.content, "New Title");
    assert_eq!(updated.runs.len(), 2);
    assert_eq!(updated.runs[1].font_name.as_deref(), Some("Helvetica-Bold"));
    assert_eq!(updated.runs[1].color, Color::rgba(180, 0, 0, 255));

    session
        .save_as(&target, SaveOptions { overwrite: true })
        .unwrap();

    let saved = fs::read_to_string(target).unwrap();
    assert!(saved.contains("content=New Title runs=2"));
    assert!(saved.contains("run=1 font=Helvetica-Bold"));
}

#[test]
fn lopdf_backend_updates_existing_pdf_text_and_saves() {
    let source = write_lopdf_text_pdf("lopdf-source", "Hello");
    let target = temp_path("lopdf-updated", "pdf");
    let engine = LopdfEngine;
    let mut session = DocumentSession::open(&engine, &source, OpenOptions::default()).unwrap();

    let hit = session
        .hit_test_text(PageIndex(0), Point::new(75.0, 75.0))
        .unwrap()
        .expect("expected text object from real PDF backend");

    assert_eq!(hit.content, "Hello");

    session
        .update_text(hit.id, "World".to_string(), None)
        .unwrap();
    session
        .save_as(&target, SaveOptions { overwrite: true })
        .unwrap();

    let reopened = DocumentSession::open(&engine, &target, OpenOptions::default()).unwrap();
    let texts = reopened.text_objects(PageIndex(0)).unwrap();

    assert!(texts.iter().any(|object| object.content == "World"));
    assert!(!texts.iter().any(|object| object.content == "Hello"));
}

#[test]
fn lopdf_backend_decodes_to_unicode_cmap_and_replaces_text() {
    let source = write_lopdf_cmap_text_pdf("lopdf-cmap-source");
    let target = temp_path("lopdf-cmap-updated", "pdf");
    let engine = LopdfEngine;
    let mut session = DocumentSession::open(&engine, &source, OpenOptions::default()).unwrap();

    let texts = session.text_objects(PageIndex(0)).unwrap();
    let object = texts
        .iter()
        .find(|object| object.content == "24.8")
        .expect("expected ToUnicode decoded text")
        .clone();

    session
        .update_text_preserving_layout(object.id, "25".to_string(), None)
        .unwrap();
    session
        .save_as(&target, SaveOptions { overwrite: true })
        .unwrap();

    let reopened = DocumentSession::open(&engine, &target, OpenOptions::default()).unwrap();
    let texts = reopened.text_objects(PageIndex(0)).unwrap();

    assert!(texts.iter().any(|object| object.content == "25"));
    assert!(!texts.iter().any(|object| object.content == "24.8"));
}

fn rendered(page: PageIndex, byte_len: usize) -> RenderedPage {
    RenderedPage {
        page,
        width_px: 1,
        height_px: (byte_len / 4).max(1) as u32,
        scale: 1.0,
        rgba: vec![0; byte_len],
    }
}

fn write_temp_pdf(name: &str) -> PathBuf {
    let path = temp_path(name, "pdf");
    fs::write(
        &path,
        b"%PDF-1.7
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>
endobj
%%EOF",
    )
    .unwrap();
    path
}

fn write_lopdf_text_pdf(name: &str, text: &str) -> PathBuf {
    let path = temp_path(name, "pdf");
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = document.add_object(dictionary! {
        "Font" => dictionary! {
            "F1" => font_id,
        },
    });
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new(
                "Tf",
                vec![Object::Name(b"F1".to_vec()), Object::Integer(12)],
            ),
            Operation::new("Td", vec![Object::Integer(72), Object::Integer(72)]),
            Operation::new("Tj", vec![Object::string_literal(text)]),
            Operation::new("ET", vec![]),
        ],
    };
    let content_id = document.add_object(Stream::new(
        dictionary! {},
        content.encode().expect("encode PDF content stream"),
    ));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)],
    });
    document.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        }
        .into(),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);
    document.compress();
    document.save(&path).expect("save generated PDF");
    path
}

fn write_lopdf_cmap_text_pdf(name: &str) -> PathBuf {
    let path = temp_path(name, "pdf");
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let cmap_stream_id = document.add_object(Stream::new(
        dictionary! {},
        b"/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo
<< /Registry (Adobe)
/Ordering (UCS)
/Supplement 0
>> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
5 beginbfchar
<0015> <0032>
<0017> <0034>
<0011> <002E>
<001B> <0038>
<0018> <0035>
endbfchar
endcmap
CMapName currentdict /CMap defineresource pop
end
end"
        .to_vec(),
    ));
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "TestFont",
        "Encoding" => "Identity-H",
        "ToUnicode" => Object::Reference(cmap_stream_id),
    });
    let resources_id = document.add_object(dictionary! {
        "Font" => dictionary! {
            "F1" => font_id,
        },
    });
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new(
                "Tf",
                vec![Object::Name(b"F1".to_vec()), Object::Integer(12)],
            ),
            Operation::new("Td", vec![Object::Integer(72), Object::Integer(72)]),
            Operation::new(
                "Tj",
                vec![Object::String(
                    vec![0x00, 0x15, 0x00, 0x17, 0x00, 0x11, 0x00, 0x1B],
                    StringFormat::Hexadecimal,
                )],
            ),
            Operation::new("ET", vec![]),
        ],
    };
    let content_id = document.add_object(Stream::new(
        dictionary! {},
        content.encode().expect("encode PDF content stream"),
    ));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)],
    });
    document.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        }
        .into(),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);
    document.compress();
    document.save(&path).expect("save generated PDF");
    path
}

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("pdfeditor-core-{name}-{nanos}.{extension}"))
}

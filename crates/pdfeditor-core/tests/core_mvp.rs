use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream, StringFormat};
use pdfeditor_core::{
    open_lopdf_document_from_bytes, page_background_png_from_pdf_bytes,
    page_structure_from_pdf_bytes, save_pdf_document_to_bytes, write_page_structure_pdf,
    write_pdf_background_png, write_pdf_page_images, BackgroundRenderOptions, Color,
    DocumentSession, EngineDocument, LopdfEngine, MockPdfEngine, OpenOptions, PageBitmapCache,
    PageIndex, PageInfo, PageStructure, Point, Rect, RenderedPage, ResourceBudget, SaveOptions,
    Size, StructuredTextObject, TextObjectId, TextRun, TextStyle,
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

    // scatter stores one char per text object; concatenate to verify full content
    let all_content: String = texts.iter().map(|o| o.content.as_str()).collect();
    assert_eq!(all_content, "World");
    assert!(!texts.iter().any(|object| object.content == "Hello"));
}

#[test]
fn lopdf_backend_uses_standard_latin_widths_for_uniform_helvetica_widths() {
    let source = write_lopdf_text_pdf("lopdf-standard-latin-widths", "Alibaba");
    let document = open_lopdf_document_from_bytes(&fs::read(source).unwrap()).unwrap();
    let text = document
        .page_structure(PageIndex(0))
        .unwrap()
        .text
        .into_iter()
        .find(|object| object.content == "Alibaba")
        .unwrap();

    let advances = text
        .glyphs
        .iter()
        .map(|glyph| (glyph.ch.as_str(), glyph.advance))
        .collect::<Vec<_>>();
    let advance = |needle: &str| {
        advances
            .iter()
            .find(|(ch, _)| *ch == needle)
            .map(|(_, advance)| *advance)
            .unwrap()
    };

    assert!(advance("l") < advance("A"));
    assert!(advance("i") < advance("b"));
    assert!(advance("A") > 0.6);
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

    // scatter stores one char per text object; concatenate to verify full content
    let all_content: String = texts.iter().map(|o| o.content.as_str()).collect();
    assert_eq!(all_content, "25");
    assert!(!texts.iter().any(|object| object.content == "24.8"));
}

#[test]
fn lopdf_backend_structures_page_content_for_json_export() {
    let source = write_lopdf_text_pdf("lopdf-page-json", "Hello");
    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &source, OpenOptions::default()).unwrap();

    let structure = session.page_structure(PageIndex(0)).unwrap();

    assert_eq!(structure.page.index, PageIndex(0));
    assert_eq!(structure.text.len(), 1);
    assert_eq!(structure.text[0].content, "Hello");
    assert_eq!(structure.text[0].font_name.as_deref(), Some("F1"));
    assert_eq!(structure.text[0].font_size, 12.0);
    assert_eq!(structure.text[0].angle_degrees, 0.0);
    assert!(structure.annotations.is_empty());
    assert!(structure.bookmarks.is_empty());
}

#[test]
fn lopdf_backend_structures_page_content_from_bytes_for_wasm() {
    let source = write_lopdf_text_pdf("lopdf-page-json-bytes", "Hello");
    let bytes = fs::read(source).unwrap();

    let structure = page_structure_from_pdf_bytes(&bytes, PageIndex(0)).unwrap();

    assert_eq!(structure.page.index, PageIndex(0));
    assert_eq!(structure.text.len(), 1);
    assert_eq!(structure.text[0].content, "Hello");
}

#[test]
fn lopdf_backend_reports_page_rotation() {
    let source = write_rotated_pdf("lopdf-rotated-page", 90);
    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &source, OpenOptions::default()).unwrap();

    let page = session.page_info(PageIndex(0)).unwrap();

    assert_eq!(page.rotation, 90);
}

#[test]
fn lopdf_backend_hit_test_returns_structured_result() {
    let source = write_lopdf_text_pdf("lopdf-structured-hit-test", "Hello");
    let bytes = fs::read(source).unwrap();
    let document = open_lopdf_document_from_bytes(&bytes).unwrap();

    let hit = document
        .hit_test(PageIndex(0), Point::new(75.0, 75.0))
        .unwrap()
        .expect("text object hit");

    assert_eq!(hit.object_type, "text");
    assert_eq!(hit.page, PageIndex(0));
    assert_eq!(hit.text_run_index, Some(0));
    assert!(hit.local_position.x >= 0.0);
    assert!(hit.local_position.y >= 0.0);
}

#[test]
fn lopdf_backend_previews_text_layout_without_committing() {
    let source = write_lopdf_text_pdf("lopdf-text-preview", "Hello");
    let bytes = fs::read(source).unwrap();
    let document = open_lopdf_document_from_bytes(&bytes).unwrap();
    let object = document.text_objects(PageIndex(0)).unwrap()[0].clone();

    let session = document.start_text_edit(object.id).unwrap();
    let preview = document
        .preview_text_layout(object.id, "Hello world".to_string())
        .unwrap();
    let unchanged = document.text_objects(PageIndex(0)).unwrap()[0].clone();

    assert_eq!(session.original_text, "Hello");
    assert_eq!(preview.text, "Hello world");
    assert_eq!(preview.glyphs.len(), "Hello world".chars().count());
    assert!(preview.bbox.size.width >= session.bbox.size.width);
    assert_eq!(unchanged.content, "Hello");
}

#[test]
fn lopdf_backend_commits_text_after_preview() {
    let source = write_lopdf_text_pdf("lopdf-text-preview-commit", "Hello");
    let bytes = fs::read(source).unwrap();
    let mut document = open_lopdf_document_from_bytes(&bytes).unwrap();
    let object = document.text_objects(PageIndex(0)).unwrap()[0].clone();

    let preview = document
        .preview_text_layout(object.id, "World".to_string())
        .unwrap();
    let committed = document
        .update_text_object(object.id, preview.text.clone(), None)
        .unwrap();

    assert_eq!(committed.content, "World");
    // scatter stores one char per text object; concatenate to verify full content
    let texts = document.text_objects(PageIndex(0)).unwrap();
    let all_content: String = texts.iter().map(|o| o.content.as_str()).collect();
    assert_eq!(all_content, "World");
}

#[test]
fn lopdf_backend_groups_consecutive_text_for_editing() {
    let source = write_lopdf_consecutive_text_runs_pdf("lopdf-group-text-edit");
    let bytes = fs::read(source).unwrap();
    let document = open_lopdf_document_from_bytes(&bytes).unwrap();
    let objects = document.text_objects(PageIndex(0)).unwrap();

    let session = document.start_text_edit(objects[1].id).unwrap();
    let preview = document
        .preview_text_layout(objects[1].id, "Go to openai.com now".to_string())
        .unwrap();

    assert_eq!(session.group_object_ids.len(), 3);
    assert_eq!(session.original_text, "Go to www.example.test now");
    assert_eq!(preview.group_object_ids.len(), 3);
    assert_eq!(preview.text, "Go to openai.com now");
    assert_eq!(
        preview.glyphs.len(),
        "Go to openai.com now".chars().count()
    );
}

#[test]
fn lopdf_backend_commits_grouped_text_edits_across_operations() {
    let source = write_lopdf_consecutive_text_runs_pdf("lopdf-group-commit");
    let bytes = fs::read(source).unwrap();
    let mut document = open_lopdf_document_from_bytes(&bytes).unwrap();
    let objects = document.text_objects(PageIndex(0)).unwrap();
    let replacement = "Go to openai.com now".to_string();

    document
        .update_text_object(objects[0].id, replacement.clone(), None)
        .unwrap();

    let updated_objects = document.text_objects(PageIndex(0)).unwrap();
    let combined = updated_objects
        .iter()
        .map(|object| object.content.as_str())
        .collect::<String>();
    assert_eq!(combined, replacement);
}

#[test]
fn lopdf_backend_advances_consecutive_structured_text_runs() {
    let source = write_lopdf_consecutive_text_runs_pdf("lopdf-consecutive-runs");
    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &source, OpenOptions::default()).unwrap();

    let structure = session.page_structure(PageIndex(0)).unwrap();

    assert_eq!(structure.text.len(), 1);
    assert_eq!(structure.text[0].content, "Go to www.example.test now");
    assert!(structure.text[0].bounds.size.width > 0.0);
    assert_eq!(structure.text[0].glyphs.len(), "Go to www.example.test now".chars().count());
}

#[test]
fn lopdf_backend_exports_background_png_from_bytes_for_wasm() {
    let source = write_lopdf_background_pdf("lopdf-background-bytes");
    let bytes = fs::read(source).unwrap();

    let png = page_background_png_from_pdf_bytes(
        &bytes,
        PageIndex(0),
        BackgroundRenderOptions::default(),
    )
    .unwrap();

    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
}

#[test]
fn lopdf_backend_updates_text_and_saves_bytes_for_wasm() {
    let source = write_lopdf_text_pdf("lopdf-update-bytes", "Hello");
    let bytes = fs::read(source).unwrap();
    let mut document = open_lopdf_document_from_bytes(&bytes).unwrap();
    let object = document.text_objects(PageIndex(0)).unwrap()[0].clone();

    document
        .update_text_object(TextObjectId(object.id.0), "World".to_string(), None)
        .unwrap();
    let updated_bytes = save_pdf_document_to_bytes(&document).unwrap();

    let reopened = open_lopdf_document_from_bytes(&updated_bytes).unwrap();
    let texts = reopened.text_objects(PageIndex(0)).unwrap();
    // scatter stores one char per text object; concatenate to verify full content
    let all_content: String = texts.iter().map(|o| o.content.as_str()).collect();
    assert_eq!(all_content, "World");
}

#[test]
fn lopdf_backend_writes_basic_background_png() {
    let source = write_lopdf_background_pdf("lopdf-background");
    let target = temp_path("lopdf-background", "png");

    let report = write_pdf_background_png(
        &source,
        PageIndex(0),
        &target,
        BackgroundRenderOptions {
            scale: 1.0,
            max_pixels: 1_000_000,
        },
    )
    .unwrap();

    let bytes = fs::read(&target).unwrap();
    assert_eq!(report.width_px, 200);
    assert_eq!(report.height_px, 200);
    assert!(report.drawn_operations >= 2);
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
}

#[test]
fn lopdf_backend_exports_image_xobject_separately_from_background_png() {
    let source = write_lopdf_image_background_pdf("lopdf-image-background");
    let background = temp_path("lopdf-image-background", "png");
    let image_dir = temp_dir("lopdf-image-background-images");

    let report = write_pdf_background_png(
        &source,
        PageIndex(0),
        &background,
        BackgroundRenderOptions {
            scale: 1.0,
            max_pixels: 1_000_000,
        },
    )
    .unwrap();
    let images = write_pdf_page_images(&source, PageIndex(0), &image_dir).unwrap();

    let bytes = fs::read(&background).unwrap();
    let pixmap = tiny_skia::Pixmap::load_png(&background).unwrap();
    let center = pixmap.pixel(80, 80).unwrap();
    assert_eq!(report.width_px, 200);
    assert_eq!(report.height_px, 200);
    assert_eq!(report.drawn_operations, 0);
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(center.red(), 255);
    assert_eq!(center.green(), 255);
    assert_eq!(center.blue(), 255);
    assert_eq!(images.len(), 1);
    assert!(images[0].file_name.ends_with(".image.png"));
    assert!(image_dir.join(&images[0].file_name).exists());
}

#[test]
fn lopdf_backend_applies_image_soft_mask_to_exported_image_png() {
    let source = write_lopdf_smask_image_background_pdf("lopdf-smask-image-background");
    let background = temp_path("lopdf-smask-image-background", "png");
    let image_dir = temp_dir("lopdf-smask-image-background-images");

    let report = write_pdf_background_png(
        &source,
        PageIndex(0),
        &background,
        BackgroundRenderOptions {
            scale: 1.0,
            max_pixels: 1_000_000,
        },
    )
    .unwrap();
    let images = write_pdf_page_images(&source, PageIndex(0), &image_dir).unwrap();
    let image_path = image_dir.join(&images[0].file_name);

    let bytes = fs::read(&background).unwrap();
    let pixmap = tiny_skia::Pixmap::load_png(&image_path).unwrap();
    let transparent_corner = pixmap.pixel(0, 0).unwrap();
    let opaque_corner = pixmap.pixel(1, 0).unwrap();
    assert_eq!(report.drawn_operations, 0);
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(transparent_corner.alpha(), 0);
    assert_eq!(opaque_corner.alpha(), 255);
    assert!(opaque_corner.red() > opaque_corner.green());
}

#[test]
fn lopdf_backend_applies_dct_image_soft_mask_to_background_png() {
    let source = write_lopdf_dct_smask_image_background_pdf("lopdf-dct-smask-image-background");
    let image_dir = temp_dir("lopdf-dct-smask-image-background-images");

    let images = write_pdf_page_images(&source, PageIndex(0), &image_dir).unwrap();

    let pixmap = tiny_skia::Pixmap::load_png(image_dir.join(&images[0].file_name)).unwrap();
    let transparent_corner = pixmap.pixel(0, 0).unwrap();
    assert_eq!(transparent_corner.alpha(), 0);
}

#[test]
fn writes_single_page_pdf_from_page_structure() {
    let target = temp_path("json-to-page", "pdf");
    let structure = PageStructure {
        page: PageInfo {
            index: PageIndex(42),
            size: Size::new(300.0, 300.0),
            rotation: 0,
        },
        text: vec![StructuredTextObject {
            id: pdfeditor_core::TextObjectId(pdfeditor_core::PdfObjectId(1)),
            bounds: Rect::new(72.0, 72.0, 120.0, 20.0),
            content: "Hello JSON".to_string(),
            font_name: Some("Helvetica".to_string()),
            font_size: 12.0,
            color: Color::BLACK,
            stroke_color: Color::BLACK,
            stroke_width: 0.0,
            rendering_mode: 0,
            transform: [12.0, 0.0, 0.0, 12.0, 72.0, 72.0],
            angle_degrees: 0.0,
            z_index: 0,
            glyphs: Vec::new(),
            runs: Vec::new(),
        }],
        visual_text: Vec::new(),
        images: Vec::new(),
        watermarks: Vec::new(),
        annotations: Vec::new(),
        bookmarks: Vec::new(),
    };

    write_page_structure_pdf(&structure, &target).unwrap();

    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &target, OpenOptions::default()).unwrap();
    assert_eq!(session.page_count(), 1);
    assert_eq!(
        session.page_info(PageIndex(0)).unwrap().size,
        Size::new(300.0, 300.0)
    );
    assert!(session
        .text_objects(PageIndex(0))
        .unwrap()
        .iter()
        .any(|object| object.content == "Hello JSON"));
}

#[test]
fn writes_single_page_pdf_from_page_structure_with_chinese_text() {
    let target = temp_path("json-to-page-chinese", "pdf");
    let structure = PageStructure {
        page: PageInfo {
            index: PageIndex(0),
            size: Size::new(300.0, 300.0),
            rotation: 0,
        },
        text: vec![StructuredTextObject {
            id: pdfeditor_core::TextObjectId(pdfeditor_core::PdfObjectId(1)),
            bounds: Rect::new(72.0, 120.0, 160.0, 24.0),
            content: "你好，PDF".to_string(),
            font_name: Some("STSong-Light".to_string()),
            font_size: 14.0,
            color: Color::BLACK,
            stroke_color: Color::BLACK,
            stroke_width: 0.0,
            rendering_mode: 0,
            transform: [14.0, 0.0, 0.0, 14.0, 72.0, 120.0],
            angle_degrees: 0.0,
            z_index: 0,
            glyphs: Vec::new(),
            runs: Vec::new(),
        }],
        visual_text: Vec::new(),
        images: Vec::new(),
        watermarks: Vec::new(),
        annotations: Vec::new(),
        bookmarks: Vec::new(),
    };

    write_page_structure_pdf(&structure, &target).unwrap();

    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &target, OpenOptions::default()).unwrap();
    assert_eq!(session.page_count(), 1);
    assert!(session
        .text_objects(PageIndex(0))
        .unwrap()
        .iter()
        .any(|object| object.content == "你好，PDF"));
}

#[test]
fn lopdf_backend_preserves_hex_unicode_text_when_updating_chinese() {
    let target = temp_path("lopdf-update-chinese-hex", "pdf");
    let structure = PageStructure {
        page: PageInfo {
            index: PageIndex(0),
            size: Size::new(300.0, 300.0),
            rotation: 0,
        },
        text: vec![StructuredTextObject {
            id: pdfeditor_core::TextObjectId(pdfeditor_core::PdfObjectId(1)),
            bounds: Rect::new(72.0, 120.0, 160.0, 24.0),
            content: "你好，PDF".to_string(),
            font_name: Some("STSong-Light".to_string()),
            font_size: 14.0,
            color: Color::BLACK,
            stroke_color: Color::BLACK,
            stroke_width: 0.0,
            rendering_mode: 0,
            transform: [14.0, 0.0, 0.0, 14.0, 72.0, 120.0],
            angle_degrees: 0.0,
            z_index: 0,
            glyphs: Vec::new(),
            runs: Vec::new(),
        }],
        visual_text: Vec::new(),
        images: Vec::new(),
        watermarks: Vec::new(),
        annotations: Vec::new(),
        bookmarks: Vec::new(),
    };
    write_page_structure_pdf(&structure, &target).unwrap();

    let bytes = fs::read(&target).unwrap();
    let mut document = open_lopdf_document_from_bytes(&bytes).unwrap();
    let object = document.text_objects(PageIndex(0)).unwrap()[0].clone();
    let original_structured = document.page_structure(PageIndex(0)).unwrap().text[0].clone();
    let original_font_size = object.font_size;
    let original_x_scale = original_structured.transform[0].hypot(original_structured.transform[1]);
    let original_y_scale = original_structured.transform[2].hypot(original_structured.transform[3]);

    document
        .update_text_object(object.id, "你好，世界".to_string(), None)
        .unwrap();
    let updated_bytes = save_pdf_document_to_bytes(&document).unwrap();

    let reopened = open_lopdf_document_from_bytes(&updated_bytes).unwrap();
    let texts = reopened.text_objects(PageIndex(0)).unwrap();
    let structured = reopened.page_structure(PageIndex(0)).unwrap().text;
    // scatter stores one char per text object; font_size is on each individual char
    let first_char = texts.iter().next().expect("expected scattered text objects");
    // page_structure uses merged_structured_text which groups adjacent scattered chars
    let updated_structured = structured
        .iter()
        .find(|item| item.content == "你好，世界")
        .unwrap();
    assert!((first_char.font_size - original_font_size).abs() < 0.01);
    assert!(
        (updated_structured.transform[0].hypot(updated_structured.transform[1]) - original_x_scale)
            .abs()
            < 0.01
    );
    assert!(
        (updated_structured.transform[2].hypot(updated_structured.transform[3]) - original_y_scale)
            .abs()
            < 0.01
    );
}

#[test]
fn lopdf_backend_uses_narrow_advances_for_ascii_inside_cjk_text() {
    let target = temp_path("lopdf-cjk-ascii-advances", "pdf");
    let replacement = "中文Alibaba Ltd. d/b/a";
    let structure = PageStructure {
        page: PageInfo {
            index: PageIndex(0),
            size: Size::new(300.0, 300.0),
            rotation: 0,
        },
        text: vec![StructuredTextObject {
            id: pdfeditor_core::TextObjectId(pdfeditor_core::PdfObjectId(1)),
            bounds: Rect::new(72.0, 120.0, 180.0, 24.0),
            content: "中文ABC123".to_string(),
            font_name: Some("STSong-Light".to_string()),
            font_size: 14.0,
            color: Color::BLACK,
            stroke_color: Color::BLACK,
            stroke_width: 0.0,
            rendering_mode: 0,
            transform: [14.0, 0.0, 0.0, 14.0, 72.0, 120.0],
            angle_degrees: 0.0,
            z_index: 0,
            glyphs: Vec::new(),
            runs: Vec::new(),
        }],
        visual_text: Vec::new(),
        images: Vec::new(),
        watermarks: Vec::new(),
        annotations: Vec::new(),
        bookmarks: Vec::new(),
    };
    write_page_structure_pdf(&structure, &target).unwrap();

    let bytes = fs::read(&target).unwrap();
    let mut document = open_lopdf_document_from_bytes(&bytes).unwrap();
    let object = document.text_objects(PageIndex(0)).unwrap()[0].clone();
    let preview = document
        .preview_text_layout(object.id, replacement.to_string())
        .unwrap();
    let chinese_advance = preview.glyphs[0].advance;
    let latin_advance = preview.glyphs[2].advance;
    let slash_advance = preview
        .glyphs
        .iter()
        .find(|glyph| glyph.ch == "/")
        .unwrap()
        .advance;
    assert!(latin_advance < chinese_advance * 0.75);
    assert!(slash_advance < latin_advance);

    document
        .update_text_object(object.id, replacement.to_string(), None)
        .unwrap();
    let updated_bytes = save_pdf_document_to_bytes(&document).unwrap();
    let updated_pdf = Document::load_mem(&updated_bytes).unwrap();
    let page_id = *updated_pdf.get_pages().values().next().unwrap();
    let page_dict = updated_pdf.get_dictionary(page_id).unwrap();
    let resources = test_dictionary_from_object(&updated_pdf, page_dict.get(b"Resources").unwrap());
    let fonts = test_dictionary_from_object(&updated_pdf, resources.get(b"Font").unwrap());
    let fallback_font = test_dictionary_from_object(
        &updated_pdf,
        fonts.get(b"PdfEditorFallbackCjk").unwrap(),
    );
    let descendant = fallback_font
        .get(b"DescendantFonts")
        .unwrap()
        .as_array()
        .unwrap()
        .first()
        .map(|object| test_dictionary_from_object(&updated_pdf, object))
        .unwrap();
    let fallback_widths = descendant.get(b"W").unwrap().as_array().unwrap();
    let ascii_widths = fallback_widths[1].as_array().unwrap();
    assert_eq!(test_integer(&fallback_widths[0]), 0);
    assert!(test_integer(&ascii_widths[b'l' as usize]) < test_integer(&ascii_widths[b'A' as usize]));
    assert!(test_integer(&ascii_widths[b'/' as usize]) < test_integer(&ascii_widths[b'A' as usize]));
    assert!(test_integer(&ascii_widths[b'W' as usize]) > test_integer(&ascii_widths[b'A' as usize]));

    let content = Content::decode(&updated_pdf.get_page_content(page_id).unwrap()).unwrap();
    let mut active_font = String::new();
    let mut fallback_replacement_tj_count = 0;
    for operation in &content.operations {
        if operation.operator == "Tf" {
            active_font = operation
                .operands
                .first()
                .and_then(|object| object.as_name().ok())
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .unwrap_or_default();
        } else if operation.operator == "Tj" {
            let Some(Object::String(bytes, _)) = operation.operands.first() else {
                continue;
            };
            let text = String::from_utf16(
                &bytes
                    .chunks_exact(2)
                    .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            if text == replacement {
                fallback_replacement_tj_count += 1;
                assert_eq!(active_font, "PdfEditorFallbackCjk");
            } else if replacement.contains(text.as_str()) {
                assert_eq!(active_font, "PdfEditorFallbackCjk");
            }
        }
    }
    assert_eq!(fallback_replacement_tj_count, 1);
    let reopened = open_lopdf_document_from_bytes(&updated_bytes).unwrap();
    let updated = reopened
        .page_structure(PageIndex(0))
        .unwrap()
        .text
        .into_iter()
        .find(|item| item.content == replacement)
        .unwrap();
    let glyph_chars = updated
        .glyphs
        .iter()
        .map(|glyph| glyph.ch.as_str())
        .collect::<Vec<_>>();
    let gap_after = |needle: &str| {
        let index = glyph_chars
            .iter()
            .position(|ch| *ch == needle)
            .expect("expected glyph");
        updated.glyphs[index + 1].x - updated.glyphs[index].x
    };
    let chinese_gap = updated.glyphs[1].x - updated.glyphs[0].x;
    let latin_gap = gap_after("A");
    let narrow_l_gap = gap_after("l");
    let slash_gap = gap_after("/");
    assert!(latin_gap < chinese_gap * 0.75);
    assert!(narrow_l_gap < latin_gap * 0.7);
    assert!(slash_gap < latin_gap * 0.7);
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

fn write_rotated_pdf(name: &str, rotation: i32) -> PathBuf {
    let path = temp_path(name, "pdf");
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(595), Object::Integer(842)],
        "Rotate" => rotation,
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
    document.save(&path).expect("save generated PDF");
    path
}

fn write_lopdf_background_pdf(name: &str) -> PathBuf {
    let path = temp_path(name, "pdf");
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let content = Content {
        operations: vec![
            Operation::new(
                "rg",
                vec![Object::Real(0.9), Object::Real(0.9), Object::Real(0.9)],
            ),
            Operation::new(
                "re",
                vec![
                    Object::Integer(20),
                    Object::Integer(20),
                    Object::Integer(80),
                    Object::Integer(40),
                ],
            ),
            Operation::new("f", vec![]),
            Operation::new("w", vec![Object::Integer(2)]),
            Operation::new(
                "RG",
                vec![Object::Real(0.1), Object::Real(0.2), Object::Real(0.8)],
            ),
            Operation::new("m", vec![Object::Integer(20), Object::Integer(120)]),
            Operation::new("l", vec![Object::Integer(180), Object::Integer(120)]),
            Operation::new("S", vec![]),
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
        "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(200), Object::Integer(200)],
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

fn write_lopdf_image_background_pdf(name: &str) -> PathBuf {
    let path = temp_path(name, "pdf");
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let image_id = document.add_object(Stream::new(
        dictionary! {
            "Type" => Object::Name(b"XObject".to_vec()),
            "Subtype" => Object::Name(b"Image".to_vec()),
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => Object::Name(b"DeviceRGB".to_vec()),
            "BitsPerComponent" => 8,
        },
        vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0],
    ));
    let resources_id = document.add_object(dictionary! {
        "XObject" => dictionary! {
            "Im1" => image_id,
        },
    });
    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    Object::Integer(80),
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(80),
                    Object::Integer(40),
                    Object::Integer(40),
                ],
            ),
            Operation::new("Do", vec![Object::Name(b"Im1".to_vec())]),
            Operation::new("Q", vec![]),
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
        "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(200), Object::Integer(200)],
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

fn write_lopdf_smask_image_background_pdf(name: &str) -> PathBuf {
    let path = temp_path(name, "pdf");
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let smask_id = document.add_object(Stream::new(
        dictionary! {
            "Type" => Object::Name(b"XObject".to_vec()),
            "Subtype" => Object::Name(b"Image".to_vec()),
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => Object::Name(b"DeviceGray".to_vec()),
            "BitsPerComponent" => 8,
            "Matte" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(0)],
        },
        vec![0, 255, 255, 0],
    ));
    let image_id = document.add_object(Stream::new(
        dictionary! {
            "Type" => Object::Name(b"XObject".to_vec()),
            "Subtype" => Object::Name(b"Image".to_vec()),
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => Object::Name(b"DeviceRGB".to_vec()),
            "BitsPerComponent" => 8,
            "SMask" => Object::Reference(smask_id),
        },
        vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 0],
    ));
    let resources_id = document.add_object(dictionary! {
        "XObject" => dictionary! {
            "Im1" => image_id,
        },
    });
    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    Object::Integer(80),
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(80),
                    Object::Integer(40),
                    Object::Integer(40),
                ],
            ),
            Operation::new("Do", vec![Object::Name(b"Im1".to_vec())]),
            Operation::new("Q", vec![]),
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
        "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(200), Object::Integer(200)],
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

fn write_lopdf_dct_smask_image_background_pdf(name: &str) -> PathBuf {
    let path = temp_path(name, "pdf");
    let jpeg = temp_path(&format!("{name}-source"), "jpg");
    image::save_buffer_with_format(
        &jpeg,
        &[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 0],
        2,
        2,
        image::ColorType::Rgb8,
        image::ImageFormat::Jpeg,
    )
    .unwrap();
    let jpeg_bytes = fs::read(jpeg).unwrap();

    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let smask_id = document.add_object(Stream::new(
        dictionary! {
            "Type" => Object::Name(b"XObject".to_vec()),
            "Subtype" => Object::Name(b"Image".to_vec()),
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => Object::Name(b"DeviceGray".to_vec()),
            "BitsPerComponent" => 8,
            "Matte" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(0)],
        },
        vec![0, 255, 255, 0],
    ));
    let image_id = document.add_object(Stream::new(
        dictionary! {
            "Type" => Object::Name(b"XObject".to_vec()),
            "Subtype" => Object::Name(b"Image".to_vec()),
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => Object::Name(b"DeviceRGB".to_vec()),
            "BitsPerComponent" => 8,
            "Filter" => Object::Name(b"DCTDecode".to_vec()),
            "SMask" => Object::Reference(smask_id),
        },
        jpeg_bytes,
    ));
    let resources_id = document.add_object(dictionary! {
        "XObject" => dictionary! {
            "Im1" => image_id,
        },
    });
    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    Object::Integer(80),
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(80),
                    Object::Integer(40),
                    Object::Integer(40),
                ],
            ),
            Operation::new("Do", vec![Object::Name(b"Im1".to_vec())]),
            Operation::new("Q", vec![]),
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
        "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(200), Object::Integer(200)],
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
    document.save(&path).unwrap();
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
        "FirstChar" => 0,
        "Widths" => Object::Array((0..=255).map(|_| Object::Integer(600)).collect()),
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

fn write_lopdf_consecutive_text_runs_pdf(name: &str) -> PathBuf {
    let path = temp_path(name, "pdf");
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "FirstChar" => 0,
        "Widths" => Object::Array((0..=255).map(|_| Object::Integer(600)).collect()),
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
            Operation::new("Td", vec![Object::Integer(40), Object::Integer(120)]),
            Operation::new("Tj", vec![Object::string_literal("Go to ")]),
            Operation::new(
                "TJ",
                vec![Object::Array(vec![
                    Object::string_literal("www.example.test"),
                    Object::Integer(-120),
                    Object::string_literal(" "),
                ])],
            ),
            Operation::new("Tj", vec![Object::string_literal("now")]),
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

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("pdfeditor-core-{name}-{nanos}"))
}

fn test_dictionary_from_object<'a>(
    document: &'a Document,
    object: &'a Object,
) -> &'a lopdf::Dictionary {
    match object {
        Object::Reference(id) => document.get_dictionary(*id).unwrap(),
        Object::Dictionary(dictionary) => dictionary,
        _ => panic!("expected dictionary object"),
    }
}

fn test_integer(object: &Object) -> i64 {
    match object {
        Object::Integer(value) => *value,
        _ => panic!("expected integer object"),
    }
}

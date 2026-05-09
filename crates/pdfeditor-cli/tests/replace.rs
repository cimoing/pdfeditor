use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};
use pdfeditor_core::{DocumentSession, LopdfEngine, OpenOptions, PageIndex};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn cli_replaces_pdf_text_with_count_limit() {
    let source = write_lopdf_text_pdf("cli-source", &["Hello", "Hello"]);
    let output = temp_path("cli-output", "pdf");

    let status = Command::new(env!("CARGO_BIN_EXE_pdfeditor"))
        .arg("replace")
        .arg("--file")
        .arg(&source)
        .arg("--find")
        .arg("Hello")
        .arg("--replace")
        .arg("World")
        .arg("--count")
        .arg("1")
        .arg("--output")
        .arg(&output)
        .status()
        .expect("run pdfeditor CLI");

    assert!(status.success());

    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &output, OpenOptions::default()).unwrap();
    let texts = session.text_objects(PageIndex(0)).unwrap();
    let world_count = texts
        .iter()
        .filter(|object| object.content == "World")
        .count();
    let hello_count = texts
        .iter()
        .filter(|object| object.content == "Hello")
        .count();

    assert_eq!(world_count, 1);
    assert_eq!(hello_count, 1);
}

#[test]
fn cli_replaces_substring_inside_decimal_text() {
    let source = write_lopdf_text_pdf("cli-decimal-source", &["24.8"]);
    let output = temp_path("cli-decimal-output", "pdf");

    let status = Command::new(env!("CARGO_BIN_EXE_pdfeditor"))
        .arg("replace")
        .arg("--file")
        .arg(&source)
        .arg("--find")
        .arg("24")
        .arg("--replace")
        .arg("25")
        .arg("--output")
        .arg(&output)
        .status()
        .expect("run pdfeditor CLI");

    assert!(status.success());

    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &output, OpenOptions::default()).unwrap();
    let texts = session.text_objects(PageIndex(0)).unwrap();

    assert!(texts.iter().any(|object| object.content == "25.8"));
}

#[test]
fn cli_allows_overflow_when_requested() {
    let source = write_lopdf_text_pdf("cli-overflow-source", &["Hi"]);
    let output = temp_path("cli-overflow-output", "pdf");

    let status = Command::new(env!("CARGO_BIN_EXE_pdfeditor"))
        .arg("replace")
        .arg("--file")
        .arg(&source)
        .arg("--find")
        .arg("Hi")
        .arg("--replace")
        .arg("This replacement is much wider than the original text")
        .arg("--allow-overflow")
        .arg("--output")
        .arg(&output)
        .status()
        .expect("run pdfeditor CLI");

    assert!(status.success());

    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &output, OpenOptions::default()).unwrap();
    let texts = session.text_objects(PageIndex(0)).unwrap();

    assert!(texts
        .iter()
        .any(|object| object.content == "This replacement is much wider than the original text"));
}

#[test]
fn cli_auto_expands_bounds_for_longer_replacement() {
    let source = write_lopdf_text_pdf("cli-auto-bounds-source", &["Hi"]);
    let output = temp_path("cli-auto-bounds-output", "pdf");

    let status = Command::new(env!("CARGO_BIN_EXE_pdfeditor"))
        .arg("replace")
        .arg("--file")
        .arg(&source)
        .arg("--find")
        .arg("Hi")
        .arg("--replace")
        .arg("This replacement is longer")
        .arg("--output")
        .arg(&output)
        .status()
        .expect("run pdfeditor CLI");

    assert!(status.success());

    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &output, OpenOptions::default()).unwrap();
    let texts = session.text_objects(PageIndex(0)).unwrap();

    assert!(texts
        .iter()
        .any(|object| object.content == "This replacement is longer"));
}

fn write_lopdf_text_pdf(name: &str, texts: &[&str]) -> PathBuf {
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
    let mut operations = vec![
        Operation::new("BT", vec![]),
        Operation::new(
            "Tf",
            vec![Object::Name(b"F1".to_vec()), Object::Integer(12)],
        ),
    ];
    for (index, text) in texts.iter().enumerate() {
        operations.push(Operation::new(
            "Td",
            vec![
                Object::Integer(72),
                Object::Integer(if index == 0 { 72 } else { 24 }),
            ],
        ));
        operations.push(Operation::new("Tj", vec![Object::string_literal(*text)]));
    }
    operations.push(Operation::new("ET", vec![]));

    let content = Content { operations };
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
    std::env::temp_dir().join(format!("pdfeditor-cli-{name}-{nanos}.{extension}"))
}

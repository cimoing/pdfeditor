use crate::{CoreError, CoreResult, PageIndex, PageStructure, Rect, StructuredAnnotation};
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Bookmark, Document, Object, Stream, StringFormat};
use std::collections::BTreeSet;
use std::path::Path;

pub fn write_page_structure_pdf(
    structure: &PageStructure,
    path: impl AsRef<Path>,
) -> CoreResult<()> {
    if structure.page.size.width <= 0.0 || structure.page.size.height <= 0.0 {
        return Err(CoreError::InvalidOperation(
            "page width and height must be positive".to_string(),
        ));
    }

    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let cjk_to_unicode_id = document.add_object(Stream::new(
        dictionary! {},
        build_to_unicode_cmap(structure).into_bytes(),
    ));
    let cid_font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType0",
        "BaseFont" => "STSong-Light",
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("GB1"),
            "Supplement" => 2,
        },
    });
    let cjk_font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "STSong-Light",
        "Encoding" => "UniGB-UCS2-H",
        "DescendantFonts" => vec![Object::Reference(cid_font_id)],
        "ToUnicode" => Object::Reference(cjk_to_unicode_id),
    });
    let resources_id = document.add_object(dictionary! {
        "Font" => dictionary! {
            "F1" => font_id,
            "F2" => cjk_font_id,
        },
    });

    let content = Content {
        operations: page_operations(structure),
    };
    let content_id = document.add_object(Stream::new(
        dictionary! {},
        content
            .encode()
            .map_err(|err| CoreError::Engine(format!("failed to encode page content: {err}")))?,
    ));

    let annotation_ids = structure
        .annotations
        .iter()
        .map(|annotation| document.add_object(annotation_dictionary(annotation)))
        .collect::<Vec<_>>();

    let mut page = dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Real(structure.page.size.width),
            Object::Real(structure.page.size.height),
        ],
    };
    if !annotation_ids.is_empty() {
        page.set(
            "Annots",
            annotation_ids
                .iter()
                .copied()
                .map(Object::Reference)
                .collect::<Vec<_>>(),
        );
    }

    let page_id = document.add_object(page);
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

    for bookmark in structure
        .bookmarks
        .iter()
        .filter(|bookmark| bookmark.page == Some(PageIndex(0)) || bookmark.page.is_none())
    {
        let bookmark = Bookmark::new(bookmark.title.clone(), [0.0, 0.0, 0.0], 0, page_id);
        document.add_bookmark(bookmark, None);
    }

    document.compress();
    document
        .save(path)
        .map_err(|err| CoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err)))?;
    Ok(())
}

fn page_operations(structure: &PageStructure) -> Vec<Operation> {
    let mut operations = Vec::new();
    let mut text = structure.text.clone();
    text.sort_by_key(|object| object.z_index);

    for object in text {
        let content = if object.runs.is_empty() {
            object.content
        } else {
            object
                .runs
                .iter()
                .map(|run| run.content.as_str())
                .collect::<String>()
        };
        if content.is_empty() {
            continue;
        }

        let font_size = object.font_size.max(1.0);
        let color = object.color;
        let is_unicode = needs_unicode_font(&content);
        let matrix = text_matrix_from_structure(object.transform, font_size, object.bounds);
        operations.push(Operation::new("BT", vec![]));
        operations.push(Operation::new(
            "Tf",
            vec![
                Object::Name(if is_unicode {
                    b"F2".to_vec()
                } else {
                    b"F1".to_vec()
                }),
                Object::Real(font_size),
            ],
        ));
        operations.push(Operation::new(
            "rg",
            vec![
                Object::Real(f32::from(color.r) / 255.0),
                Object::Real(f32::from(color.g) / 255.0),
                Object::Real(f32::from(color.b) / 255.0),
            ],
        ));
        operations.push(Operation::new(
            "Tm",
            matrix.iter().copied().map(Object::Real).collect(),
        ));
        operations.push(Operation::new(
            "Tj",
            vec![encoded_text_object(&content, is_unicode)],
        ));
        operations.push(Operation::new("ET", vec![]));
    }

    let mut images = structure.images.clone();
    images.sort_by_key(|object| object.z_index);
    for image in images {
        operations.push(Operation::new(
            "re",
            vec![
                Object::Real(image.bounds.origin.x),
                Object::Real(image.bounds.origin.y),
                Object::Real(image.bounds.size.width),
                Object::Real(image.bounds.size.height),
            ],
        ));
        operations.push(Operation::new("S", vec![]));
    }

    operations
}

fn text_matrix_from_structure(transform: [f32; 6], font_size: f32, bounds: Rect) -> [f32; 6] {
    if transform != [0.0; 6] {
        return [
            transform[0] / font_size,
            transform[1] / font_size,
            transform[2] / font_size,
            transform[3] / font_size,
            transform[4],
            transform[5],
        ];
    }

    [1.0, 0.0, 0.0, 1.0, bounds.origin.x, bounds.origin.y]
}

fn needs_unicode_font(content: &str) -> bool {
    !content.is_ascii()
}

fn encoded_text_object(content: &str, unicode: bool) -> Object {
    if unicode {
        Object::String(utf16be_bytes(content), StringFormat::Hexadecimal)
    } else {
        Object::string_literal(content.to_string())
    }
}

fn utf16be_bytes(content: &str) -> Vec<u8> {
    content
        .encode_utf16()
        .flat_map(|unit| unit.to_be_bytes())
        .collect()
}

fn build_to_unicode_cmap(structure: &PageStructure) -> String {
    let mut codes = BTreeSet::new();
    for object in &structure.text {
        let content = if object.runs.is_empty() {
            object.content.as_str().to_string()
        } else {
            object
                .runs
                .iter()
                .map(|run| run.content.as_str())
                .collect::<String>()
        };
        if needs_unicode_font(&content) {
            for code_unit in content.encode_utf16() {
                codes.insert(code_unit);
            }
        }
    }

    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n\
12 dict begin\n\
begincmap\n\
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
/CMapName /Adobe-Identity-UCS def\n\
/CMapType 2 def\n\
1 begincodespacerange\n\
<0000> <FFFF>\n\
endcodespacerange\n",
    );

    for chunk in codes.into_iter().collect::<Vec<_>>().chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for code in chunk {
            cmap.push_str(&format!("<{code:04X}> <{code:04X}>\n"));
        }
        cmap.push_str("endbfchar\n");
    }

    cmap.push_str(
        "endcmap\n\
CMapName currentdict /CMap defineresource pop\n\
end\n\
end",
    );
    cmap
}

fn annotation_dictionary(annotation: &StructuredAnnotation) -> lopdf::Dictionary {
    let subtype = annotation.subtype.as_deref().unwrap_or("Text");
    let rect = annotation
        .bounds
        .unwrap_or_else(|| Rect::new(0.0, 0.0, 24.0, 24.0));
    let mut dictionary = dictionary! {
        "Type" => "Annot",
        "Subtype" => Object::Name(subtype.as_bytes().to_vec()),
        "Rect" => vec![
            Object::Real(rect.origin.x),
            Object::Real(rect.origin.y),
            Object::Real(rect.origin.x + rect.size.width),
            Object::Real(rect.origin.y + rect.size.height),
        ],
    };
    if let Some(contents) = &annotation.contents {
        dictionary.set("Contents", Object::string_literal(contents.clone()));
    }
    if let Some(name) = &annotation.name {
        dictionary.set("NM", Object::string_literal(name.clone()));
    }
    if let Some(flags) = annotation.flags {
        dictionary.set("F", Object::Integer(flags));
    }
    dictionary
}

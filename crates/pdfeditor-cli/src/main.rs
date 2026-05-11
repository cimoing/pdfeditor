use pdfeditor_core::{
    write_page_structure_pdf, write_pdf_background_png, write_pdf_page_images,
    BackgroundRenderOptions, CoreError, DocumentSession, LopdfEngine, OpenOptions, PageIndex,
    PageStructure, Rect, SaveOptions,
};
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run(env::args_os()) {
        Ok(CommandReport::Replace(report)) => {
            println!(
                "Replaced {} occurrence(s). Wrote {}",
                report.replacements,
                report.output.display()
            );
            ExitCode::SUCCESS
        }
        Ok(CommandReport::DumpText) => ExitCode::SUCCESS,
        Ok(CommandReport::PageJson) => ExitCode::SUCCESS,
        Ok(CommandReport::JsonPage(report)) => {
            println!("Wrote {}", report.output.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ReplaceArgs {
    file: PathBuf,
    find: String,
    replacement: String,
    count: Option<usize>,
    output: Option<PathBuf>,
    in_place: bool,
    allow_overflow: bool,
    bounds: Option<Rect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplaceReport {
    replacements: usize,
    output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommandReport {
    Replace(ReplaceReport),
    DumpText,
    PageJson,
    JsonPage(JsonPageReport),
}

fn run<I>(args: I) -> Result<CommandReport, String>
where
    I: IntoIterator<Item = OsString>,
{
    match parse_args(args)? {
        CommandArgs::Replace(args) => replace_pdf_text(&args)
            .map(CommandReport::Replace)
            .map_err(|error| error.to_string()),
        CommandArgs::DumpText(args) => {
            dump_pdf_text(&args).map_err(|error| error.to_string())?;
            Ok(CommandReport::DumpText)
        }
        CommandArgs::PageJson(args) => {
            page_to_json(&args).map_err(|error| error.to_string())?;
            Ok(CommandReport::PageJson)
        }
        CommandArgs::JsonPage(args) => json_to_page(&args)
            .map(CommandReport::JsonPage)
            .map_err(|error| error.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DumpTextArgs {
    file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PageJsonArgs {
    file: PathBuf,
    page: PageIndex,
    output: Option<PathBuf>,
    bitmap_dir: Option<PathBuf>,
    bitmap_width: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JsonPageArgs {
    file: PathBuf,
    output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JsonPageReport {
    output: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
enum CommandArgs {
    Replace(ReplaceArgs),
    DumpText(DumpTextArgs),
    PageJson(PageJsonArgs),
    JsonPage(JsonPageArgs),
}

fn parse_args<I>(args: I) -> Result<CommandArgs, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let program = args.next().unwrap_or_else(|| OsString::from("pdfeditor"));
    let Some(command) = args.next() else {
        return Err(usage(&program));
    };

    match command.to_string_lossy().as_ref() {
        "replace" => parse_replace_args(program, args),
        "dump-text" => parse_dump_text_args(program, args),
        "page-json" => parse_page_json_args(program, args),
        "json-page" | "json-to-page" => parse_json_page_args(program, args),
        _ => Err(usage(&program)),
    }
}

fn parse_replace_args<I>(program: OsString, args: I) -> Result<CommandArgs, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut file = None;
    let mut find = None;
    let mut replacement = None;
    let mut count = None;
    let mut output = None;
    let mut in_place = false;
    let mut allow_overflow = false;
    let mut bounds = None;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--file" | "-f" => file = Some(next_path(&mut args, "--file")?),
            "--find" => find = Some(next_string(&mut args, "--find")?),
            "--replace" => replacement = Some(next_string(&mut args, "--replace")?),
            "--count" | "-n" => {
                let value = next_string(&mut args, "--count")?;
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| "--count must be a non-negative integer".to_string())?;
                count = Some(parsed);
            }
            "--output" | "-o" => output = Some(next_path(&mut args, "--output")?),
            "--in-place" => in_place = true,
            "--allow-overflow" => allow_overflow = true,
            "--bounds" => bounds = Some(parse_bounds(&next_string(&mut args, "--bounds")?)?),
            "--help" | "-h" => return Err(usage(&program)),
            other => return Err(format!("unknown argument: {other}\n\n{}", usage(&program))),
        }
    }

    if output.is_some() && in_place {
        return Err("--output and --in-place cannot be used together".to_string());
    }

    let file = file.ok_or_else(|| "--file is required".to_string())?;
    let find = find.ok_or_else(|| "--find is required".to_string())?;
    if find.is_empty() {
        return Err("--find cannot be empty".to_string());
    }
    let replacement = replacement.ok_or_else(|| "--replace is required".to_string())?;

    Ok(CommandArgs::Replace(ReplaceArgs {
        file,
        find,
        replacement,
        count,
        output,
        in_place,
        allow_overflow,
        bounds,
    }))
}

fn parse_dump_text_args<I>(program: OsString, args: I) -> Result<CommandArgs, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut file = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--file" | "-f" => file = Some(next_path(&mut args, "--file")?),
            "--help" | "-h" => return Err(usage(&program)),
            other => return Err(format!("unknown argument: {other}\n\n{}", usage(&program))),
        }
    }

    Ok(CommandArgs::DumpText(DumpTextArgs {
        file: file.ok_or_else(|| "--file is required".to_string())?,
    }))
}

fn parse_page_json_args<I>(program: OsString, args: I) -> Result<CommandArgs, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut file = None;
    let mut page = PageIndex(0);
    let mut output = None;
    let mut bitmap_dir = None;
    let mut bitmap_width = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--file" | "-f" => file = Some(next_path(&mut args, "--file")?),
            "--page" | "-p" => {
                let value = next_string(&mut args, "--page")?;
                let parsed = value
                    .parse::<u32>()
                    .map_err(|_| "--page must be a positive page number".to_string())?;
                if parsed == 0 {
                    return Err("--page is 1-based and must be greater than 0".to_string());
                }
                page = PageIndex(parsed - 1);
            }
            "--output" | "-o" => output = Some(next_path(&mut args, "--output")?),
            "--bitmap-dir" => bitmap_dir = Some(next_path(&mut args, "--bitmap-dir")?),
            "--bitmap-width" => {
                let value = next_string(&mut args, "--bitmap-width")?;
                let parsed = value
                    .parse::<u32>()
                    .map_err(|_| "--bitmap-width must be a positive integer".to_string())?;
                if parsed == 0 {
                    return Err("--bitmap-width must be positive".to_string());
                }
                bitmap_width = Some(parsed);
            }
            "--help" | "-h" => return Err(usage(&program)),
            other => return Err(format!("unknown argument: {other}\n\n{}", usage(&program))),
        }
    }

    Ok(CommandArgs::PageJson(PageJsonArgs {
        file: file.ok_or_else(|| "--file is required".to_string())?,
        page,
        output,
        bitmap_dir,
        bitmap_width,
    }))
}

fn parse_json_page_args<I>(program: OsString, args: I) -> Result<CommandArgs, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut file = None;
    let mut output = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--file" | "-f" => file = Some(next_path(&mut args, "--file")?),
            "--output" | "-o" => output = Some(next_path(&mut args, "--output")?),
            "--help" | "-h" => return Err(usage(&program)),
            other => return Err(format!("unknown argument: {other}\n\n{}", usage(&program))),
        }
    }

    Ok(CommandArgs::JsonPage(JsonPageArgs {
        file: file.ok_or_else(|| "--file is required".to_string())?,
        output: output.ok_or_else(|| "--output is required".to_string())?,
    }))
}

fn replace_pdf_text(args: &ReplaceArgs) -> Result<ReplaceReport, CoreError> {
    let engine = LopdfEngine;
    let mut session = DocumentSession::open(&engine, &args.file, OpenOptions::default())?;
    let mut replacements = 0usize;
    let limit = args.count.unwrap_or(usize::MAX);

    if limit > 0 {
        'pages: for page_number in 0..session.page_count() {
            let page = PageIndex(page_number);
            let objects = session.text_objects(page)?;
            let remaining = limit.saturating_sub(replacements);
            if remaining == 0 {
                break 'pages;
            }

            let page_replacements = replace_page_text(
                &mut session,
                &objects,
                &args.find,
                &args.replacement,
                remaining,
                args.allow_overflow,
                args.bounds,
                true,
            )?;
            replacements += page_replacements;
            if replacements >= limit {
                break 'pages;
            }
        }
    }

    let output = output_path(args);
    session.save_as(&output, SaveOptions { overwrite: true })?;
    Ok(ReplaceReport {
        replacements,
        output,
    })
}

fn replace_page_text<D: pdfeditor_core::EngineDocument>(
    session: &mut DocumentSession<D>,
    objects: &[pdfeditor_core::TextObject],
    find: &str,
    replacement: &str,
    limit: usize,
    allow_overflow: bool,
    bounds: Option<Rect>,
    auto_bounds: bool,
) -> Result<usize, CoreError> {
    let mut combined = String::new();
    let mut ranges = Vec::with_capacity(objects.len());
    for object in objects {
        let start = combined.chars().count();
        combined.push_str(&object.content);
        let end = combined.chars().count();
        ranges.push((start, end));
    }

    let mut matches = Vec::new();
    for (byte_start, matched) in combined.match_indices(find) {
        if matches.len() >= limit {
            break;
        }
        let start = byte_to_char_index(&combined, byte_start);
        let end = start + matched.chars().count();
        matches.push((start, end));
    }

    if matches.is_empty() {
        return Ok(0);
    }

    let mut updated = objects
        .iter()
        .map(|object| object.content.clone())
        .collect::<Vec<_>>();

    for (start, end) in matches.iter().rev().copied() {
        apply_cross_object_replacement(&mut updated, &ranges, start, end, replacement);
    }

    for (object, updated_content) in objects.iter().zip(updated) {
        if object.content != updated_content {
            if let Some(bounds) = bounds.or_else(|| {
                auto_bounds
                    .then(|| auto_text_bounds(object.bounds, object.font_size, &updated_content))
            }) {
                if bounds != object.bounds {
                    session.update_text_bounds(object.id, bounds)?;
                }
            }
            if allow_overflow {
                session.update_text_unbounded(object.id, updated_content, None)?;
            } else {
                session.update_text_preserving_layout(object.id, updated_content, None)?;
            }
        }
    }

    Ok(matches.len())
}

fn auto_text_bounds(current: Rect, font_size: f32, content: &str) -> Rect {
    let estimated = estimate_text_size(font_size, content);
    Rect::new(
        current.origin.x,
        current.origin.y,
        current.size.width.max(estimated.0),
        current.size.height.max(estimated.1),
    )
}

fn estimate_text_size(font_size: f32, content: &str) -> (f32, f32) {
    let font_size = font_size.max(1.0);
    let average_glyph_width = font_size * 0.6;
    let line_height = font_size * 1.2;
    let mut line_count = 0usize;
    let mut max_line_chars = 0usize;

    for line in content.split('\n') {
        line_count += 1;
        max_line_chars = max_line_chars.max(line.chars().count());
    }

    (
        max_line_chars as f32 * average_glyph_width,
        line_count.max(1) as f32 * line_height,
    )
}

fn apply_cross_object_replacement(
    updated: &mut [String],
    ranges: &[(usize, usize)],
    start: usize,
    end: usize,
    replacement: &str,
) {
    let touched = ranges
        .iter()
        .enumerate()
        .filter_map(|(index, (object_start, object_end))| {
            if *object_start < end && *object_end > start {
                Some(index)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if touched.is_empty() {
        return;
    }

    if touched.len() == 1 {
        let index = touched[0];
        let local_start = start.saturating_sub(ranges[index].0);
        let local_end = end.saturating_sub(ranges[index].0);
        updated[index] = replace_char_range(&updated[index], local_start, local_end, replacement);
        return;
    }

    let replacement_chars = replacement
        .chars()
        .map(|c| c.to_string())
        .collect::<Vec<_>>();
    let mut replacement_index = 0usize;

    for (position, object_index) in touched.iter().copied().enumerate() {
        let object_start = ranges[object_index].0;
        let object_end = ranges[object_index].1;
        let local_start = start.saturating_sub(object_start);
        let local_end = end.min(object_end).saturating_sub(object_start);
        let local_replacement = if replacement_index < replacement_chars.len() {
            let value = replacement_chars[replacement_index].as_str();
            replacement_index += 1;
            value
        } else {
            ""
        };

        updated[object_index] = replace_char_range(
            &updated[object_index],
            local_start,
            local_end,
            local_replacement,
        );

        if position == touched.len() - 1 && replacement_index < replacement_chars.len() {
            updated[object_index].push_str(&replacement_chars[replacement_index..].concat());
        }
    }
}

fn replace_char_range(input: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut output = String::new();
    for (index, ch) in input.chars().enumerate() {
        if index == start {
            output.push_str(replacement);
        }
        if index < start || index >= end {
            output.push(ch);
        }
    }
    if start >= input.chars().count() {
        output.push_str(replacement);
    }
    output
}

fn parse_bounds(value: &str) -> Result<Rect, String> {
    let parts = value
        .split(',')
        .map(str::trim)
        .map(|part| {
            part.parse::<f32>()
                .map_err(|_| "--bounds must be in x,y,width,height format".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    if parts.len() != 4 {
        return Err("--bounds must be in x,y,width,height format".to_string());
    }
    if parts[2] <= 0.0 || parts[3] <= 0.0 {
        return Err("--bounds width and height must be positive".to_string());
    }

    Ok(Rect::new(parts[0], parts[1], parts[2], parts[3]))
}

fn byte_to_char_index(input: &str, byte_index: usize) -> usize {
    input[..byte_index].chars().count()
}

fn dump_pdf_text(args: &DumpTextArgs) -> Result<(), CoreError> {
    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &args.file, OpenOptions::default())?;
    for page_number in 0..session.page_count() {
        let page = PageIndex(page_number);
        for object in session.text_objects(page)? {
            println!(
                "page={} id={} bounds=({:.1},{:.1},{:.1},{:.1}) text={:?}",
                page.0,
                (object.id.0).0,
                object.bounds.origin.x,
                object.bounds.origin.y,
                object.bounds.size.width,
                object.bounds.size.height,
                object.content
            );
        }
    }
    Ok(())
}

fn page_to_json(args: &PageJsonArgs) -> Result<(), CoreError> {
    let engine = LopdfEngine;
    let session = DocumentSession::open(&engine, &args.file, OpenOptions::default())?;
    let mut structure = session.page_structure(args.page)?;
    if let Some(bitmap_dir) = &args.bitmap_dir {
        write_page_background_bitmap(args, bitmap_dir, &structure)?;
        write_page_image_objects(args, bitmap_dir, &mut structure)?;
    }
    let json = serde_json::to_string_pretty(&structure)
        .map_err(|error| CoreError::Engine(format!("failed to serialize page JSON: {error}")))?;
    if let Some(output) = &args.output {
        std::fs::write(output, json)?;
    } else {
        println!("{json}");
    }
    Ok(())
}

fn write_page_background_bitmap(
    args: &PageJsonArgs,
    bitmap_dir: &Path,
    _structure: &PageStructure,
) -> Result<(), CoreError> {
    std::fs::create_dir_all(bitmap_dir)?;
    if args.bitmap_width.is_some() {
        eprintln!(
            "--bitmap-width is ignored because background PNG dimensions must match page.size"
        );
    }
    let output = bitmap_dir.join(format!("{}.png", args.page.0 + 1));
    write_pdf_background_png(
        &args.file,
        args.page,
        output,
        BackgroundRenderOptions::default(),
    )?;
    Ok(())
}

fn write_page_image_objects(
    args: &PageJsonArgs,
    bitmap_dir: &Path,
    structure: &mut PageStructure,
) -> Result<(), CoreError> {
    let exported = write_pdf_page_images(&args.file, args.page, bitmap_dir)?;
    for image in &mut structure.images {
        if let Some(export) = exported.iter().find(|export| export.id == image.id) {
            image.source_file = Some(export.file_name.clone());
        }
    }
    Ok(())
}

fn json_to_page(args: &JsonPageArgs) -> Result<JsonPageReport, CoreError> {
    let json = std::fs::read_to_string(&args.file)?;
    let mut structure: PageStructure = serde_json::from_str(&json)
        .map_err(|error| CoreError::InvalidOperation(format!("invalid page JSON: {error}")))?;
    structure.page.index = PageIndex(0);
    for bookmark in &mut structure.bookmarks {
        bookmark.page = Some(PageIndex(0));
    }
    write_page_structure_pdf(&structure, &args.output)?;
    Ok(JsonPageReport {
        output: args.output.clone(),
    })
}

fn output_path(args: &ReplaceArgs) -> PathBuf {
    if args.in_place {
        return args.file.clone();
    }
    if let Some(output) = &args.output {
        return output.clone();
    }

    let parent = args.file.parent().unwrap_or_else(|| Path::new(""));
    let stem = args
        .file
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    parent.join(format!("{stem}.edited.pdf"))
}

fn next_string<I>(args: &mut I, flag: &str) -> Result<String, String>
where
    I: Iterator<Item = OsString>,
{
    let value = args
        .next()
        .ok_or_else(|| format!("{flag} requires a value"))?;
    value
        .into_string()
        .map_err(|_| format!("{flag} value must be valid UTF-8"))
}

fn next_path<I>(args: &mut I, flag: &str) -> Result<PathBuf, String>
where
    I: Iterator<Item = OsString>,
{
    let value = args
        .next()
        .ok_or_else(|| format!("{flag} requires a value"))?;
    Ok(PathBuf::from(value))
}

fn usage(program: &OsString) -> String {
    format!(
        "Usage:\n  {} replace --file <input.pdf> --find <text> --replace <text> [--count <n>] [--bounds x,y,width,height] [--allow-overflow] [--output <output.pdf> | --in-place]\n  {} dump-text --file <input.pdf>\n  {} page-json --file <input.pdf> [--page <1-based-page>] [--output <page.json>] [--bitmap-dir <dir>] [--bitmap-width <px>]\n  {} json-page --file <page.json> --output <single-page.pdf>",
        Path::new(program).display(),
        Path::new(program).display(),
        Path::new(program).display(),
        Path::new(program).display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_replace_args() {
        let args = parse_args([
            OsString::from("pdfeditor"),
            OsString::from("replace"),
            OsString::from("--file"),
            OsString::from("a.pdf"),
            OsString::from("--find"),
            OsString::from("old"),
            OsString::from("--replace"),
            OsString::from("new"),
            OsString::from("--count"),
            OsString::from("2"),
            OsString::from("--bounds"),
            OsString::from("1,2,300,40"),
            OsString::from("--allow-overflow"),
        ])
        .unwrap();

        let CommandArgs::Replace(args) = args else {
            panic!("expected replace args");
        };

        assert_eq!(args.file, PathBuf::from("a.pdf"));
        assert_eq!(args.find, "old");
        assert_eq!(args.replacement, "new");
        assert_eq!(args.count, Some(2));
        assert!(args.allow_overflow);
        assert_eq!(args.bounds, Some(Rect::new(1.0, 2.0, 300.0, 40.0)));
    }

    #[test]
    fn parses_page_json_args() {
        let args = parse_args([
            OsString::from("pdfeditor"),
            OsString::from("page-json"),
            OsString::from("--file"),
            OsString::from("a.pdf"),
            OsString::from("--page"),
            OsString::from("2"),
            OsString::from("--output"),
            OsString::from("page.json"),
            OsString::from("--bitmap-dir"),
            OsString::from("bitmaps"),
            OsString::from("--bitmap-width"),
            OsString::from("600"),
        ])
        .unwrap();

        let CommandArgs::PageJson(args) = args else {
            panic!("expected page-json args");
        };

        assert_eq!(args.file, PathBuf::from("a.pdf"));
        assert_eq!(args.page, PageIndex(1));
        assert_eq!(args.output, Some(PathBuf::from("page.json")));
        assert_eq!(args.bitmap_dir, Some(PathBuf::from("bitmaps")));
        assert_eq!(args.bitmap_width, Some(600));
    }

    #[test]
    fn parses_json_page_args() {
        let args = parse_args([
            OsString::from("pdfeditor"),
            OsString::from("json-page"),
            OsString::from("--file"),
            OsString::from("page.json"),
            OsString::from("--output"),
            OsString::from("page.pdf"),
        ])
        .unwrap();

        let CommandArgs::JsonPage(args) = args else {
            panic!("expected json-page args");
        };

        assert_eq!(args.file, PathBuf::from("page.json"));
        assert_eq!(args.output, PathBuf::from("page.pdf"));
    }

    #[test]
    fn replaces_cross_object_text() {
        let objects = [
            pdfeditor_core::TextObject {
                id: pdfeditor_core::TextObjectId(pdfeditor_core::PdfObjectId(1)),
                page: PageIndex(0),
                bounds: pdfeditor_core::Rect::new(0.0, 0.0, 20.0, 20.0),
                content: "2".to_string(),
                font_name: None,
                font_size: 10.0,
                color: pdfeditor_core::Color::BLACK,
                runs: Vec::new(),
            },
            pdfeditor_core::TextObject {
                id: pdfeditor_core::TextObjectId(pdfeditor_core::PdfObjectId(2)),
                page: PageIndex(0),
                bounds: pdfeditor_core::Rect::new(20.0, 0.0, 20.0, 20.0),
                content: "4".to_string(),
                font_name: None,
                font_size: 10.0,
                color: pdfeditor_core::Color::BLACK,
                runs: Vec::new(),
            },
            pdfeditor_core::TextObject {
                id: pdfeditor_core::TextObjectId(pdfeditor_core::PdfObjectId(3)),
                page: PageIndex(0),
                bounds: pdfeditor_core::Rect::new(40.0, 0.0, 20.0, 20.0),
                content: ".8".to_string(),
                font_name: None,
                font_size: 10.0,
                color: pdfeditor_core::Color::BLACK,
                runs: Vec::new(),
            },
        ];
        let mut updated = objects
            .iter()
            .map(|object| object.content.clone())
            .collect::<Vec<_>>();
        let ranges = [(0, 1), (1, 2), (2, 4)];

        apply_cross_object_replacement(&mut updated, &ranges, 0, 4, "25");

        assert_eq!(updated, ["2", "5", ""]);
    }
}

use crate::{
    CoreError, CoreResult, EditCommand, EditQueue, EngineDocument, ImageObject, ObjectSnapshot,
    PageBitmapCache, PageIndex, PageInfo, PdfEngine, Point, Rect, RenderedPage, ResourceBudget,
    TextObject, TextObjectId, TextRun, TextStyle,
};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub resource_budget: ResourceBudget,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            resource_budget: ResourceBudget::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SaveOptions {
    pub overwrite: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self { overwrite: false }
    }
}

#[derive(Debug)]
pub struct DocumentSession<D: EngineDocument> {
    path: PathBuf,
    document: D,
    edits: EditQueue,
    page_cache: PageBitmapCache,
    budget: ResourceBudget,
}

impl<D: EngineDocument> DocumentSession<D> {
    pub fn open<E>(engine: &E, path: impl AsRef<Path>, options: OpenOptions) -> CoreResult<Self>
    where
        E: PdfEngine<Document = D>,
    {
        let path = path.as_ref().to_path_buf();
        let document = engine.open(&path)?;
        Ok(Self {
            path,
            document,
            edits: EditQueue::new(options.resource_budget.undo_steps),
            page_cache: PageBitmapCache::new(options.resource_budget.page_cache_bytes),
            budget: options.resource_budget,
        })
    }

    pub fn source_path(&self) -> &Path {
        &self.path
    }

    pub fn page_count(&self) -> u32 {
        self.document.page_count()
    }

    pub fn page_info(&self, page: PageIndex) -> CoreResult<PageInfo> {
        self.document.page_info(page)
    }

    pub fn text_objects(&self, page: PageIndex) -> CoreResult<Vec<TextObject>> {
        self.document.text_objects(page)
    }

    pub fn hit_test_text(&self, page: PageIndex, point: Point) -> CoreResult<Option<TextObject>> {
        let mut objects = self.document.text_objects(page)?;
        objects.reverse();
        Ok(objects
            .into_iter()
            .find(|object| object.bounds.contains(point)))
    }

    pub fn image_objects(&self, page: PageIndex) -> CoreResult<Vec<ImageObject>> {
        self.document.image_objects(page)
    }

    pub fn render_page(&mut self, page: PageIndex, scale: f32) -> CoreResult<&RenderedPage> {
        if self.page_cache.get(page).is_none() {
            let rendered = self
                .document
                .render_page(page, scale, self.budget.max_render_pixels)?;
            self.page_cache.insert(rendered);
        }
        self.page_cache
            .get(page)
            .ok_or_else(|| CoreError::Engine("rendered page was not cached".to_string()))
    }

    pub fn add_text(
        &mut self,
        page: PageIndex,
        bounds: Rect,
        content: String,
        style: TextStyle,
    ) -> CoreResult<TextObject> {
        let object = self
            .document
            .add_text_object(page, bounds, content.clone(), style.clone())?;
        self.edits.push(EditCommand::AddText {
            page,
            bounds,
            content,
            style: style.clone(),
            runs: vec![TextRun::new(
                object.content.clone(),
                style.font_name,
                style.font_size,
                style.color,
            )],
        });
        self.page_cache.remove(page);
        Ok(object)
    }

    pub fn update_text(
        &mut self,
        id: TextObjectId,
        content: String,
        style: Option<TextStyle>,
    ) -> CoreResult<TextObject> {
        let before = self.find_text_object(id)?;
        let effective_style = style.clone().unwrap_or_else(|| TextStyle {
            font_name: before.font_name.clone(),
            font_size: before.font_size,
            color: before.color,
        });

        ensure_text_fits_bounds(&content, &effective_style, before.bounds)?;

        let before_snapshot = ObjectSnapshot::Text {
            id: before.id,
            page: before.page,
            bounds: before.bounds,
            content: before.content.clone(),
            style: TextStyle {
                font_name: before.font_name.clone(),
                font_size: before.font_size,
                color: before.color,
            },
            runs: before.runs.clone(),
        };

        let updated = self
            .document
            .update_text_object(id, content.clone(), style.clone())?;
        self.edits.push(EditCommand::UpdateText {
            id,
            before: before_snapshot,
            content,
            style,
        });
        self.page_cache.remove(updated.page);
        Ok(updated)
    }

    pub fn update_text_unbounded(
        &mut self,
        id: TextObjectId,
        content: String,
        style: Option<TextStyle>,
    ) -> CoreResult<TextObject> {
        let before = self.find_text_object(id)?;
        let before_snapshot = ObjectSnapshot::Text {
            id: before.id,
            page: before.page,
            bounds: before.bounds,
            content: before.content.clone(),
            style: TextStyle {
                font_name: before.font_name.clone(),
                font_size: before.font_size,
                color: before.color,
            },
            runs: before.runs.clone(),
        };

        let updated = self
            .document
            .update_text_object(id, content.clone(), style.clone())?;
        self.edits.push(EditCommand::UpdateText {
            id,
            before: before_snapshot,
            content,
            style,
        });
        self.page_cache.remove(updated.page);
        Ok(updated)
    }

    pub fn update_text_preserving_layout(
        &mut self,
        id: TextObjectId,
        content: String,
        style: Option<TextStyle>,
    ) -> CoreResult<TextObject> {
        let before = self.find_text_object(id)?;
        let effective_style = style.clone().unwrap_or_else(|| TextStyle {
            font_name: before.font_name.clone(),
            font_size: before.font_size,
            color: before.color,
        });

        ensure_text_preserves_layout(&before.content, &content, &effective_style, before.bounds)?;

        let before_snapshot = ObjectSnapshot::Text {
            id: before.id,
            page: before.page,
            bounds: before.bounds,
            content: before.content.clone(),
            style: TextStyle {
                font_name: before.font_name.clone(),
                font_size: before.font_size,
                color: before.color,
            },
            runs: before.runs.clone(),
        };

        let updated = self
            .document
            .update_text_object(id, content.clone(), style.clone())?;
        self.edits.push(EditCommand::UpdateText {
            id,
            before: before_snapshot,
            content,
            style,
        });
        self.page_cache.remove(updated.page);
        Ok(updated)
    }

    pub fn update_text_runs(
        &mut self,
        id: TextObjectId,
        runs: Vec<TextRun>,
    ) -> CoreResult<TextObject> {
        if runs.is_empty() {
            return Err(CoreError::InvalidOperation(
                "text runs cannot be empty".to_string(),
            ));
        }

        let before = self.find_text_object(id)?;
        ensure_text_runs_fit_bounds(&runs, before.bounds)?;

        let before_snapshot = ObjectSnapshot::Text {
            id: before.id,
            page: before.page,
            bounds: before.bounds,
            content: before.content.clone(),
            style: TextStyle {
                font_name: before.font_name.clone(),
                font_size: before.font_size,
                color: before.color,
            },
            runs: before.runs.clone(),
        };

        let updated = self.document.update_text_object_runs(id, runs.clone())?;
        self.edits.push(EditCommand::UpdateTextRuns {
            id,
            before: before_snapshot,
            runs,
        });
        self.page_cache.remove(updated.page);
        Ok(updated)
    }

    pub fn update_text_bounds(&mut self, id: TextObjectId, bounds: Rect) -> CoreResult<TextObject> {
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return Err(CoreError::InvalidOperation(
                "text bounds width and height must be positive".to_string(),
            ));
        }

        let updated = self.document.update_text_object_bounds(id, bounds)?;
        self.page_cache.remove(updated.page);
        Ok(updated)
    }

    pub fn insert_image(&mut self, page: PageIndex, bounds: Rect, format: String, bytes: Vec<u8>) {
        self.edits.push(EditCommand::InsertImage {
            page,
            bounds,
            format,
            bytes,
        });
        self.page_cache.remove(page);
    }

    pub fn move_visible_page_object(&mut self, page: PageIndex, _delta: Point) {
        self.page_cache.remove(page);
    }

    pub fn edits(&self) -> &EditQueue {
        &self.edits
    }

    pub fn edits_mut(&mut self) -> &mut EditQueue {
        &mut self.edits
    }

    pub fn is_dirty(&self) -> bool {
        self.edits.is_dirty()
    }

    pub fn cache_stats(&self) -> crate::CacheStats {
        self.page_cache.stats()
    }

    pub fn save_as(&mut self, target: impl AsRef<Path>, options: SaveOptions) -> CoreResult<()> {
        let target = target.as_ref();
        if target.exists() && !options.overwrite {
            return Err(CoreError::InvalidOperation(format!(
                "{} already exists",
                target.display()
            )));
        }

        let temp_path = temp_save_path(target);
        if temp_path.exists() {
            fs::remove_file(&temp_path)?;
        }

        self.document.save_to(&temp_path)?;
        if target.exists() && options.overwrite {
            fs::remove_file(target)?;
        }
        fs::rename(&temp_path, target)?;
        self.edits.mark_saved();
        Ok(())
    }

    fn find_text_object(&self, id: TextObjectId) -> CoreResult<TextObject> {
        for page_index in 0..self.document.page_count() {
            let page = PageIndex(page_index);
            if let Some(object) = self
                .document
                .text_objects(page)?
                .into_iter()
                .find(|object| object.id == id)
            {
                return Ok(object);
            }
        }

        Err(CoreError::NotFound(format!("text object {}", (id.0).0)))
    }
}

fn temp_save_path(target: &Path) -> PathBuf {
    let mut extension = target
        .extension()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    extension.push(".tmp");
    target.with_extension(extension)
}

fn ensure_text_fits_bounds(content: &str, style: &TextStyle, bounds: Rect) -> CoreResult<()> {
    let run = TextRun::new(
        content.to_string(),
        style.font_name.clone(),
        style.font_size,
        style.color,
    );
    ensure_text_runs_fit_bounds(&[run], bounds)
}

fn ensure_text_preserves_layout(
    before: &str,
    after: &str,
    style: &TextStyle,
    bounds: Rect,
) -> CoreResult<()> {
    let before_metrics = estimate_text_metrics(before, style.font_size);
    let after_metrics = estimate_text_metrics(after, style.font_size);
    let allowed_width = bounds.size.width.max(before_metrics.width);
    let allowed_height = bounds.size.height.max(before_metrics.height);

    if after_metrics.width <= allowed_width && after_metrics.height <= allowed_height {
        return Ok(());
    }

    Err(CoreError::InvalidOperation(format!(
        "text exceeds object bounds: estimated {:.1}x{:.1}, allowed {:.1}x{:.1}",
        after_metrics.width, after_metrics.height, allowed_width, allowed_height
    )))
}

fn ensure_text_runs_fit_bounds(runs: &[TextRun], bounds: Rect) -> CoreResult<()> {
    let metrics = estimate_text_runs_metrics(runs);
    if metrics.width <= bounds.size.width && metrics.height <= bounds.size.height {
        return Ok(());
    }

    Err(CoreError::InvalidOperation(format!(
        "text exceeds object bounds: estimated {:.1}x{:.1}, allowed {:.1}x{:.1}",
        metrics.width, metrics.height, bounds.size.width, bounds.size.height
    )))
}

fn estimate_text_runs_metrics(runs: &[TextRun]) -> crate::Size {
    let mut max_width = 0.0f32;
    let mut current_width = 0.0f32;
    let mut current_line_height = 0.0f32;
    let mut total_height = 0.0f32;
    let mut saw_content = false;

    for run in runs {
        let font_size = run.font_size.max(1.0);
        let line_height = font_size * 1.2;
        let average_glyph_width = font_size * 0.6;

        for segment in run.content.split_inclusive('\n') {
            saw_content = true;
            let visible = segment.trim_end_matches('\n');
            current_width += visible.chars().count() as f32 * average_glyph_width;
            current_line_height = current_line_height.max(line_height);

            if segment.ends_with('\n') {
                max_width = max_width.max(current_width);
                total_height += current_line_height.max(line_height);
                current_width = 0.0;
                current_line_height = 0.0;
            }
        }
    }

    if saw_content {
        max_width = max_width.max(current_width);
        total_height += current_line_height.max(1.0);
    }

    crate::Size::new(max_width, total_height)
}

#[allow(dead_code)]
fn estimate_text_metrics(content: &str, font_size: f32) -> crate::Size {
    let run = TextRun::new(content.to_string(), None, font_size, crate::Color::BLACK);
    estimate_text_runs_metrics(&[run])
}

use crate::{Color, ImageObjectId, PageIndex, Point, Rect, TextObjectId, TextRun};

#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    pub font_name: Option<String>,
    pub font_size: f32,
    pub color: Color,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectSnapshot {
    Text {
        id: TextObjectId,
        page: PageIndex,
        bounds: Rect,
        content: String,
        style: TextStyle,
        runs: Vec<TextRun>,
    },
    Image {
        id: ImageObjectId,
        page: PageIndex,
        bounds: Rect,
        format: String,
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditCommand {
    AddText {
        page: PageIndex,
        bounds: Rect,
        content: String,
        style: TextStyle,
        runs: Vec<TextRun>,
    },
    UpdateText {
        id: TextObjectId,
        before: ObjectSnapshot,
        content: String,
        style: Option<TextStyle>,
    },
    UpdateTextRuns {
        id: TextObjectId,
        before: ObjectSnapshot,
        runs: Vec<TextRun>,
    },
    MoveText {
        id: TextObjectId,
        delta: Point,
        before: ObjectSnapshot,
    },
    DeleteText {
        id: TextObjectId,
        before: ObjectSnapshot,
    },
    InsertImage {
        page: PageIndex,
        bounds: Rect,
        format: String,
        bytes: Vec<u8>,
    },
    ReplaceImage {
        id: ImageObjectId,
        before: ObjectSnapshot,
        format: String,
        bytes: Vec<u8>,
    },
    MoveImage {
        id: ImageObjectId,
        delta: Point,
        before: ObjectSnapshot,
    },
    DeleteImage {
        id: ImageObjectId,
        before: ObjectSnapshot,
    },
}

#[derive(Debug, Clone)]
pub struct EditQueue {
    pending: Vec<EditCommand>,
    undone: Vec<EditCommand>,
    max_undo_steps: usize,
}

impl EditQueue {
    pub fn new(max_undo_steps: usize) -> Self {
        Self {
            pending: Vec::new(),
            undone: Vec::new(),
            max_undo_steps,
        }
    }

    pub fn push(&mut self, command: EditCommand) {
        self.pending.push(command);
        self.undone.clear();
        self.trim();
    }

    pub fn undo(&mut self) -> Option<&EditCommand> {
        let command = self.pending.pop()?;
        self.undone.push(command);
        self.undone.last()
    }

    pub fn redo(&mut self) -> Option<&EditCommand> {
        let command = self.undone.pop()?;
        self.pending.push(command);
        self.pending.last()
    }

    pub fn pending(&self) -> &[EditCommand] {
        &self.pending
    }

    pub fn undone(&self) -> &[EditCommand] {
        &self.undone
    }

    pub fn is_dirty(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn mark_saved(&mut self) {
        self.pending.clear();
        self.undone.clear();
    }

    fn trim(&mut self) {
        if self.max_undo_steps == 0 {
            self.pending.clear();
            return;
        }

        let overflow = self.pending.len().saturating_sub(self.max_undo_steps);
        if overflow > 0 {
            self.pending.drain(0..overflow);
        }
    }
}

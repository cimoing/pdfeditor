#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBudget {
    pub page_cache_bytes: usize,
    pub thumbnail_cache_bytes: usize,
    pub undo_steps: usize,
    pub max_render_pixels: u64,
}

impl ResourceBudget {
    pub const LOW_RESOURCE: Self = Self {
        page_cache_bytes: 96 * 1024 * 1024,
        thumbnail_cache_bytes: 16 * 1024 * 1024,
        undo_steps: 30,
        max_render_pixels: 16_000_000,
    };

    pub const DEFAULT: Self = Self {
        page_cache_bytes: 192 * 1024 * 1024,
        thumbnail_cache_bytes: 32 * 1024 * 1024,
        undo_steps: 50,
        max_render_pixels: 32_000_000,
    };
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self::DEFAULT
    }
}

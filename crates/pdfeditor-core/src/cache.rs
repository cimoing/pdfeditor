use crate::{PageIndex, RenderedPage};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub used_bytes: usize,
    pub max_bytes: usize,
}

#[derive(Debug)]
pub struct PageBitmapCache {
    max_bytes: usize,
    used_bytes: usize,
    pages: HashMap<PageIndex, RenderedPage>,
    order: VecDeque<PageIndex>,
}

impl PageBitmapCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            used_bytes: 0,
            pages: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn insert(&mut self, page: RenderedPage) {
        let index = page.page;
        self.remove(index);

        let size = page.estimated_bytes();
        if size > self.max_bytes {
            return;
        }

        self.used_bytes += size;
        self.order.push_back(index);
        self.pages.insert(index, page);
        self.evict_until_within_budget();
    }

    pub fn get(&mut self, index: PageIndex) -> Option<&RenderedPage> {
        if self.pages.contains_key(&index) {
            self.touch(index);
        }
        self.pages.get(&index)
    }

    pub fn remove(&mut self, index: PageIndex) -> Option<RenderedPage> {
        self.order.retain(|item| *item != index);
        let removed = self.pages.remove(&index)?;
        self.used_bytes = self.used_bytes.saturating_sub(removed.estimated_bytes());
        Some(removed)
    }

    pub fn clear(&mut self) {
        self.pages.clear();
        self.order.clear();
        self.used_bytes = 0;
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.pages.len(),
            used_bytes: self.used_bytes,
            max_bytes: self.max_bytes,
        }
    }

    fn touch(&mut self, index: PageIndex) {
        self.order.retain(|item| *item != index);
        self.order.push_back(index);
    }

    fn evict_until_within_budget(&mut self) {
        while self.used_bytes > self.max_bytes {
            let Some(index) = self.order.pop_front() else {
                break;
            };
            if let Some(page) = self.pages.remove(&index) {
                self.used_bytes = self.used_bytes.saturating_sub(page.estimated_bytes());
            }
        }
    }
}

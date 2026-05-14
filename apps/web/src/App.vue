<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref } from "vue";
import type {
  LoadedFontAsset,
  PageStructure,
  StructuredTextObject,
  TextEditSessionInfo,
  TextLayoutPreview
} from "./pdfEditor";
import {
  closePdfDocument,
  commitTextEdit,
  describePdfFontUsage,
  loadPdfPage,
  openPdfDocument,
  previewTextLayout,
  startTextEdit
} from "./pdfEditor";
import { SkiaPageRenderer } from "./skiaRenderer";
import { composePageTransform, pdfRectToViewportRect, type PageViewport } from "./viewport";

const pdfBytes = ref<Uint8Array | null>(null);
const pdfHandle = ref<number | null>(null);
const pdfFileName = ref("document.pdf");
const pageNumber = ref(1);
const status = ref("选择 PDF 后加载页面");
const page = ref<PageStructure | null>(null);
const backgroundUrl = ref<string | null>(null);
const skiaCanvas = ref<HTMLCanvasElement | null>(null);
const zoom = ref(1);
const selectedTextId = ref<number | null>(null);
const editSession = ref<TextEditSessionInfo | null>(null);
const draftText = ref("");
const layoutPreview = ref<TextLayoutPreview | null>(null);
const fontFamilies = ref<Record<string, string>>({});
const isPreparingEdit = ref(false);
const isSavingEdit = ref(false);

const skiaRenderer = new SkiaPageRenderer();
let currentFontAssets: LoadedFontAsset[] = [];
let previewTimer: number | null = null;
let previewRequestSequence = 0;
let selectionSequence = 0;

const currentViewport = computed<PageViewport | null>(() => {
  if (!page.value) return null;
  return composePageTransform({
    pageIndex: page.value.page.index,
    pageWidth: page.value.page.size.width,
    pageHeight: page.value.page.size.height,
    zoom: zoom.value,
    rotation: page.value.page.rotation ?? 0,
    devicePixelRatio: window.devicePixelRatio || 1
  });
});

const zoomPercent = computed(() => `${Math.round(zoom.value * 100)}%`);
const textCount = computed(() => page.value?.text.length ?? 0);
const imageCount = computed(() => page.value?.images.length ?? 0);
const pageTextObjects = computed(() => page.value?.text ?? []);
const selectedTextObject = computed<StructuredTextObject | null>(
  () => page.value?.text.find((item) => item.id === selectedTextId.value) ?? null
);
const activeGroupObjectIds = computed(() => {
  const ids = layoutPreview.value?.group_object_ids ?? editSession.value?.group_object_ids ?? [];
  return ids.length ? ids : selectedTextId.value != null ? [selectedTextId.value] : [];
});
const selectedFontUsage = computed(() =>
  describePdfFontUsage(editSession.value?.font_id ?? selectedTextObject.value?.font_name ?? null, fontFamilies.value)
);
const renderTextObjects = computed(() => {
  const pageText = page.value?.text ?? [];
  if (!layoutPreview.value || !selectedTextObject.value || !editSession.value) {
    return pageText;
  }
  const hiddenIds = new Set(layoutPreview.value.group_object_ids);
  const previewObject: StructuredTextObject = {
    ...selectedTextObject.value,
    bounds: layoutPreview.value.bbox,
    content: layoutPreview.value.text,
    font_name: editSession.value.font_id,
    font_size: editSession.value.font_size,
    transform: editSession.value.matrix,
    glyphs: layoutPreview.value.glyphs
  };
  return pageText.filter((text) => !hiddenIds.has(text.id)).concat(previewObject);
});
const selectedViewportRect = computed(() => {
  const viewport = currentViewport.value;
  const targetBounds = layoutPreview.value?.bbox ?? selectedTextObject.value?.bounds;
  if (!viewport || !targetBounds) return null;
  return pdfRectToViewportRect(viewport, targetBounds);
});
const previewGlyphRects = computed(() => {
  const viewport = currentViewport.value;
  const glyphs = layoutPreview.value?.glyphs ?? [];
  if (!viewport) return [];
  return glyphs.map((glyph, index) => ({
    id: `${index}-${glyph.ch}-${glyph.x}-${glyph.y}`,
    rect: pdfRectToViewportRect(viewport, glyph.bbox)
  }));
});
const canSaveEdit = computed(() => {
  if (!selectedTextObject.value || !editSession.value || isSavingEdit.value || isPreparingEdit.value) {
    return false;
  }
  if (layoutPreview.value?.overflow) {
    return false;
  }
  return draftText.value !== editSession.value.original_text;
});
const previewStatus = computed(() => {
  if (!selectedTextObject.value) return "点击页面高亮框或左侧列表，开始编辑文本对象。";
  if (isPreparingEdit.value) return "正在准备文本编辑会话...";
  if (!layoutPreview.value) return "修改文本后会实时生成布局预览。";
  if (layoutPreview.value.overflow) return "当前文本超出原始文本边界，暂不能保存。";
  if (draftText.value === editSession.value?.original_text) return "当前内容与原始文本一致。";
  return "预览通过，可以保存到当前 PDF。";
});

onBeforeUnmount(() => {
  closePdfDocument(pdfHandle.value);
  skiaRenderer.dispose();
  revokeUrls();
  clearPreviewTimer();
});

async function onFileChange(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;

  clearEditingState();
  closePdfDocument(pdfHandle.value);
  pdfHandle.value = null;
  page.value = null;
  pdfFileName.value = file.name;
  pdfBytes.value = new Uint8Array(await file.arrayBuffer());
  status.value = "正在打开 PDF...";
  pdfHandle.value = await openPdfDocument(pdfBytes.value);
  await loadPage();
}

async function loadPage(options?: { preserveSelectionId?: number | null }) {
  if (!pdfBytes.value) return;

  status.value = "正在解析 PDF 页面...";
  revokeUrls();
  const loaded = await loadPdfPage(pdfBytes.value, pageNumber.value, pdfHandle.value);
  page.value = loaded.structure;
  backgroundUrl.value = loaded.backgroundUrl;
  currentFontAssets = loaded.fontAssets;
  fontFamilies.value = loaded.fontFamilies;

  const preserveSelectionId = options?.preserveSelectionId ?? null;
  if (preserveSelectionId == null || !loaded.structure.text.some((item) => item.id === preserveSelectionId)) {
    clearEditingState();
  } else {
    selectedTextId.value = preserveSelectionId;
    editSession.value = null;
    layoutPreview.value = null;
  }

  await nextTick();
  await renderPdfCanvas();
  status.value = `已加载第 ${pageNumber.value} 页：${loaded.structure.text.length} 个文本对象，${loaded.structure.images.length} 个图片对象`;
}

function onLoadPageClick() {
  void loadPage();
}

async function renderPdfCanvas() {
  if (!backgroundUrl.value || !skiaCanvas.value || !currentViewport.value || !page.value) return;
  try {
    await skiaRenderer.render({
      canvas: skiaCanvas.value,
      viewport: currentViewport.value,
      backgroundUrl: backgroundUrl.value,
      texts: renderTextObjects.value,
      fonts: currentFontAssets
    });
  } catch (error) {
    console.warn("Failed to render page with CanvasKit", error);
    status.value = "CanvasKit 渲染失败，请查看控制台错误";
  }
}

async function beginTextEdit(objectId: number) {
  if (!pdfBytes.value) return;
  clearPreviewTimer();
  selectedTextId.value = objectId;
  isPreparingEdit.value = true;
  layoutPreview.value = null;
  void nextTick(renderPdfCanvas);
  status.value = `正在准备文本对象 ${objectId} 的编辑会话...`;
  const currentSelection = ++selectionSequence;

  try {
    const session = await startTextEdit(pdfBytes.value, objectId, pdfHandle.value);
    if (currentSelection !== selectionSequence) return;
    editSession.value = session;
    draftText.value = session.original_text;
    await refreshPreview(objectId, session.original_text, currentSelection);
    if (currentSelection !== selectionSequence) return;
    status.value = `已选中文本对象 ${objectId}，可直接修改并保存`;
  } catch (error) {
    console.error(error);
    if (currentSelection !== selectionSequence) return;
    status.value = error instanceof Error ? error.message : "启动文本编辑失败";
    selectedTextId.value = null;
    editSession.value = null;
    layoutPreview.value = null;
  } finally {
    if (currentSelection === selectionSequence) {
      isPreparingEdit.value = false;
    }
  }
}

function onDraftInput() {
  if (!selectedTextId.value) return;
  clearPreviewTimer();
  previewTimer = window.setTimeout(() => {
    void refreshPreview(selectedTextId.value!, draftText.value, selectionSequence);
  }, 120);
}

async function refreshPreview(objectId: number, text: string, selectionToken = selectionSequence) {
  if (!pdfBytes.value) return;
  const requestId = ++previewRequestSequence;
  try {
    const preview = await previewTextLayout(pdfBytes.value, objectId, text, pdfHandle.value);
    if (requestId !== previewRequestSequence || selectionToken !== selectionSequence || selectedTextId.value !== objectId) {
      return;
    }
    layoutPreview.value = preview;
    void nextTick(renderPdfCanvas);
  } catch (error) {
    console.error(error);
    if (requestId !== previewRequestSequence || selectionToken !== selectionSequence) {
      return;
    }
    status.value = error instanceof Error ? error.message : "生成文本布局预览失败";
  }
}

async function saveTextEdit() {
  if (!pdfBytes.value || !selectedTextId.value || !canSaveEdit.value) return;
  clearPreviewTimer();
  isSavingEdit.value = true;
  status.value = `正在保存文本对象 ${selectedTextId.value}...`;
  const objectId = selectedTextId.value;

  try {
    const updatedBytes = await commitTextEdit(pdfBytes.value, objectId, draftText.value, pdfHandle.value);
    pdfBytes.value = new Uint8Array(updatedBytes);
    await loadPage({ preserveSelectionId: objectId });
    await beginTextEdit(objectId);
    status.value = `文本对象 ${objectId} 已保存`;
  } catch (error) {
    console.error(error);
    status.value = error instanceof Error ? error.message : "保存文本编辑失败";
  } finally {
    isSavingEdit.value = false;
  }
}

function clearEditingState() {
  clearPreviewTimer();
  selectionSequence += 1;
  previewRequestSequence += 1;
  selectedTextId.value = null;
  editSession.value = null;
  draftText.value = "";
  layoutPreview.value = null;
  isPreparingEdit.value = false;
  isSavingEdit.value = false;
  void nextTick(renderPdfCanvas);
}

function clearPreviewTimer() {
  if (previewTimer != null) {
    window.clearTimeout(previewTimer);
    previewTimer = null;
  }
}

function zoomIn() {
  zoom.value = clampZoom(zoom.value + 0.1);
  void nextTick(renderPdfCanvas);
}

function zoomOut() {
  zoom.value = clampZoom(zoom.value - 0.1);
  void nextTick(renderPdfCanvas);
}

function resetZoom() {
  zoom.value = 1;
  void nextTick(renderPdfCanvas);
}

function clampZoom(value: number) {
  return Math.min(3, Math.max(0.25, Math.round(value * 10) / 10));
}

function pageViewportStyle() {
  const viewport = currentViewport.value;
  if (!viewport) return {};
  return {
    width: `${viewport.width}px`,
    height: `${viewport.height}px`
  };
}

function pageCanvasStyle() {
  const viewport = currentViewport.value;
  if (!viewport) return {};
  return {
    width: `${viewport.width}px`,
    height: `${viewport.height}px`
  };
}

function textObjectStyle(text: StructuredTextObject) {
  const viewport = currentViewport.value;
  if (!viewport) return {};
  const rect = pdfRectToViewportRect(viewport, text.bounds);
  return {
    left: `${rect.left}px`,
    top: `${rect.top}px`,
    width: `${Math.max(rect.width, 12)}px`,
    height: `${Math.max(rect.height, 12)}px`
  };
}

function exportCurrentPdf() {
  if (!pdfBytes.value) return;
  const blob = new Blob([toArrayBuffer(pdfBytes.value)], { type: "application/pdf" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  const baseName = pdfFileName.value.replace(/\.pdf$/i, "") || "document";
  anchor.href = url;
  anchor.download = `${baseName}-edited.pdf`;
  anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}

function textObjectLabel(text: StructuredTextObject) {
  const normalized = text.content.replace(/\s+/g, " ").trim();
  return normalized || `文本对象 ${text.id}`;
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

function revokeUrls() {
  if (backgroundUrl.value) {
    URL.revokeObjectURL(backgroundUrl.value);
    backgroundUrl.value = null;
  }
  page.value?.images.forEach((image) => {
    if (image.objectUrl) URL.revokeObjectURL(image.objectUrl);
  });
}
</script>

<template>
  <main class="app-shell">
    <aside class="sidebar">
      <header>
        <h1>PDF Editor Web Demo</h1>
      </header>

      <label class="field">
        <span>PDF 文件</span>
        <input type="file" accept="application/pdf" @change="onFileChange" />
      </label>

      <label class="field page-field">
        <span>页码</span>
        <input v-model.number="pageNumber" type="number" min="1" />
        <button :disabled="!pdfBytes" @click="onLoadPageClick">加载</button>
      </label>

      <section class="zoom-controls" aria-label="缩放">
        <button title="缩小" :disabled="zoom <= 0.25" @click="zoomOut">−</button>
        <button title="重置缩放" @click="resetZoom">{{ zoomPercent }}</button>
        <button title="放大" :disabled="zoom >= 3" @click="zoomIn">+</button>
      </section>

      <div class="action-row">
        <button :disabled="!pdfBytes" @click="exportCurrentPdf">导出当前 PDF</button>
        <button :disabled="!selectedTextId" @click="clearEditingState">取消选择</button>
      </div>

      <p class="status">{{ status }}</p>

      <section v-if="page" class="render-summary">
        <h2>渲染摘要</h2>
        <div>文本对象：{{ textCount }}</div>
        <div>图片对象：{{ imageCount }}</div>
        <div>旋转角度：{{ page.page.rotation ?? 0 }}°</div>
      </section>

      <section v-if="selectedTextObject" class="editor-panel">
        <h2>文本编辑</h2>
        <div class="object-id">对象 ID：{{ selectedTextObject.id }}</div>
        <div v-if="activeGroupObjectIds.length > 1" class="object-id">联动对象：{{ activeGroupObjectIds.length }} 个</div>
        <div class="font-meta">
          <div>PDF 字体：{{ editSession?.font_id ?? selectedTextObject.font_name ?? "未提供" }}</div>
          <div>显示字体：{{ selectedFontUsage.displayFamily }}</div>
          <div>字号：{{ (editSession?.font_size ?? selectedTextObject.font_size).toFixed(2) }}</div>
          <div :class="selectedFontUsage.fellBack ? 'font-fallback' : 'font-embedded'">
            {{ selectedFontUsage.fellBack ? selectedFontUsage.fallbackReason : "已命中嵌入字体" }}
          </div>
        </div>
        <label class="field">
          <span>文本内容</span>
          <textarea
            v-model="draftText"
            :disabled="isPreparingEdit || isSavingEdit"
            spellcheck="false"
            @input="onDraftInput"
          />
        </label>
        <p class="helper-text" :class="{ danger: Boolean(layoutPreview?.overflow) }">
          {{ previewStatus }}
        </p>
        <button class="save-button" :disabled="!canSaveEdit" @click="saveTextEdit">
          {{ isSavingEdit ? "正在保存..." : "保存到 PDF" }}
        </button>
      </section>

      <section v-if="pageTextObjects.length" class="text-list">
        <h2>文本对象列表</h2>
        <button
          v-for="text in pageTextObjects"
          :key="text.id"
          :class="{ selected: activeGroupObjectIds.includes(text.id) }"
          :title="text.content"
          @click="beginTextEdit(text.id)"
        >
          {{ textObjectLabel(text) }}
        </button>
      </section>
    </aside>

    <section class="canvas-pane">
      <div v-if="page && backgroundUrl" class="page-viewport" :style="pageViewportStyle()">
        <div class="page-canvas" :style="pageCanvasStyle()">
          <canvas ref="skiaCanvas" class="background" aria-label="PDF canvas render"></canvas>

          <button
            v-for="text in pageTextObjects"
            :key="`overlay-${text.id}`"
            class="text-object"
            :class="{ selected: activeGroupObjectIds.includes(text.id) }"
            :style="textObjectStyle(text)"
            :title="text.content || `文本对象 ${text.id}`"
            @click="beginTextEdit(text.id)"
          />

          <svg
            v-if="currentViewport && selectedViewportRect"
            class="layout-preview"
            :class="{ overflow: Boolean(layoutPreview?.overflow) }"
            :viewBox="`0 0 ${currentViewport.width} ${currentViewport.height}`"
          >
            <rect
              class="layout-preview-box"
              :x="selectedViewportRect.left"
              :y="selectedViewportRect.top"
              :width="selectedViewportRect.width"
              :height="selectedViewportRect.height"
            />
            <rect
              v-for="glyph in previewGlyphRects"
              :key="glyph.id"
              class="layout-preview-glyph"
              :x="glyph.rect.left"
              :y="glyph.rect.top"
              :width="glyph.rect.width"
              :height="glyph.rect.height"
            />
          </svg>
        </div>
      </div>
      <div v-else class="empty-state">加载 PDF 后显示 CanvasKit 渲染结果，并可直接选择文本编辑</div>
    </section>
  </main>
</template>

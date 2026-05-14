<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref } from "vue";
import type { StructuredTextObject } from "./pdfEditor";
import {
  commitTextEdit,
  describePdfFontUsage,
  previewTextLayout,
  startTextEdit,
  hitTestPdf
} from "./pdfEditor";
import { SkiaPageRenderer } from "./skiaRenderer";
import { pdfRectToViewportRect, viewportToPdf } from "./viewport";
import { usePdfDocument } from "./composables/usePdfDocument";
import { usePdfEditor } from "./composables/usePdfEditor";

const {
  pdfBytes,
  pdfHandle,
  pdfFileName,
  pageNumber,
  status,
  page,
  backgroundUrl,
  zoom,
  fontFamilies,
  fontAssets,
  currentViewport,
  cleanup: cleanupPdf,
  openFile,
  loadCurrentPage
} = usePdfDocument();

const {
  selectedTextId,
  editSession,
  draftText,
  layoutPreview,
  isPreparingEdit,
  isSavingEdit,
  clearPreviewTimer,
  clearEditingState,
  getSelectionToken,
  incrementSelection,
  getPreviewToken,
  incrementPreviewToken,
  setPreviewTimer
} = usePdfEditor();

const skiaCanvas = ref<HTMLCanvasElement | null>(null);
const skiaRenderer = new SkiaPageRenderer();

// Viewport bindings
const zoomPercent = computed(() => `${Math.round(zoom.value * 100)}%`);
const textCount = computed(() => page.value?.text.length ?? 0);
const imageCount = computed(() => page.value?.images.length ?? 0);
const pageTextObjects = computed(() => page.value?.text ?? []);

// Edit bindings
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
  cleanupPdf();
  skiaRenderer.dispose();
  clearPreviewTimer();
});

async function onFileChange(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;

  clearEditingState();
  await openFile(file);
  await loadPage();
}

async function loadPage(options?: { preserveSelectionId?: number | null }) {
  const loaded = await loadCurrentPage();
  if (!loaded) return;

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
      fonts: fontAssets.value
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
  const currentSelection = incrementSelection();

  try {
    const session = await startTextEdit(pdfBytes.value, objectId, pdfHandle.value);
    if (currentSelection !== getSelectionToken()) return;
    editSession.value = session;
    draftText.value = session.original_text;
    await refreshPreview(objectId, session.original_text, currentSelection);
    if (currentSelection !== getSelectionToken()) return;
    status.value = `已选中文本对象 ${objectId}，可直接修改并保存`;
  } catch (error) {
    console.error(error);
    if (currentSelection !== getSelectionToken()) return;
    status.value = error instanceof Error ? error.message : "启动文本编辑失败";
    selectedTextId.value = null;
    editSession.value = null;
    layoutPreview.value = null;
  } finally {
    if (currentSelection === getSelectionToken()) {
      isPreparingEdit.value = false;
    }
  }
}

function onDraftInput() {
  if (!selectedTextId.value) return;
  setPreviewTimer(() => {
    void refreshPreview(selectedTextId.value!, draftText.value, getSelectionToken());
  }, 120);
}

async function refreshPreview(objectId: number, text: string, selectionToken = getSelectionToken()) {
  if (!pdfBytes.value) return;
  const requestId = incrementPreviewToken();
  try {
    const preview = await previewTextLayout(pdfBytes.value, objectId, text, pdfHandle.value);
    if (requestId !== getPreviewToken() || selectionToken !== getSelectionToken() || selectedTextId.value !== objectId) {
      return;
    }
    layoutPreview.value = preview;
    void nextTick(renderPdfCanvas);
  } catch (error) {
    console.error(error);
    if (requestId !== getPreviewToken() || selectionToken !== getSelectionToken()) {
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

function clearSelection() {
  clearEditingState();
  void nextTick(renderPdfCanvas);
}

// Zoom controls
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

// Canvas Interaction (HitTest)
async function onCanvasPointerDown(event: PointerEvent) {
  if (!pdfBytes.value || !currentViewport.value || !page.value) return;
  
  const target = event.currentTarget as HTMLElement;
  const rect = target.getBoundingClientRect();
  const offsetX = event.clientX - rect.left;
  const offsetY = event.clientY - rect.top;
  
  const pdfPoint = viewportToPdf(currentViewport.value, offsetX, offsetY);
  
  try {
    const hitResult = await hitTestPdf(pdfBytes.value, page.value.page.index, pdfPoint.x, pdfPoint.y, pdfHandle.value);
    if (hitResult && hitResult.object_type === "text") {
      await beginTextEdit(hitResult.object_id);
    } else {
      clearSelection();
    }
  } catch (error) {
    console.error("Hit test failed", error);
  }
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
    height: `${viewport.height}px`,
    cursor: "text"
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
        <button :disabled="!selectedTextId" @click="clearSelection">取消选择</button>
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
        <div class="page-canvas" :style="pageCanvasStyle()" @pointerdown="onCanvasPointerDown">
          <canvas ref="skiaCanvas" class="background" aria-label="PDF canvas render"></canvas>

          <svg
            v-if="currentViewport && selectedViewportRect"
            class="layout-preview"
            :class="{ overflow: Boolean(layoutPreview?.overflow) }"
            :viewBox="`0 0 ${currentViewport.width} ${currentViewport.height}`"
            style="pointer-events: none;"
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
      <div v-else class="empty-state">加载 PDF 后显示 CanvasKit 渲染结果，并可直接点击画布文本进行编辑</div>
    </section>
  </main>
</template>

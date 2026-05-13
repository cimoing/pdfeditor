 <script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref } from "vue";
import type { LoadedFontAsset, PageStructure, Rect, StructuredTextObject, TextLayoutPreview } from "./pdfEditor";
import {
  applyTextEdits,
  asBlobPart,
  closePdfDocument,
  commitTextEdit,
  describePdfFontUsage,
  hitTestPdf,
  loadPdfPage,
  openPdfDocument,
  previewTextLayout,
  resolvePdfFontFamily,
  startTextEdit
} from "./pdfEditor";
import { SkiaPageRenderer } from "./skiaRenderer";
import {
  composePageTransform,
  pdfRectToViewportRect,
  viewportToPdf,
  type PageViewport,
  type ViewportRect
} from "./viewport";

interface EditableText {
  id: number;
  original: string;
  content: string;
  object: StructuredTextObject;
  composing: boolean;
  preview: TextLayoutPreview | null;
}

const pdfBytes = ref<Uint8Array | null>(null);
const pdfHandle = ref<number | null>(null);
const fileName = ref("");
const pageNumber = ref(1);
const status = ref("选择 PDF 后加载页面");
const page = ref<PageStructure | null>(null);
const backgroundUrl = ref<string | null>(null);
const fontFamilies = ref<Record<string, string>>({});
const skiaCanvas = ref<HTMLCanvasElement | null>(null);
const editableTexts = ref<EditableText[]>([]);
const measuredTextRects = ref<Record<number, ViewportRect>>({});
const selectedTextId = ref<number | null>(null);
const saving = ref(false);
const committingTextId = ref<number | null>(null);
const zoom = ref(1);
const skiaRenderer = new SkiaPageRenderer();
let renderSequence = 0;
let currentFontAssets: LoadedFontAsset[] = [];
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

const selectedText = computed(() =>
  editableTexts.value.find((text) => text.id === selectedTextId.value) ?? editableTexts.value[0] ?? null
);
const selectedTextFontUsage = computed(() =>
  selectedText.value ? describePdfFontUsage(selectedText.value.object.font_name, fontFamilies.value) : null
);
const inlineEditingText = computed(() =>
  selectedTextId.value == null ? null : editableTexts.value.find((text) => text.id === selectedTextId.value) ?? null
);
const zoomPercent = computed(() => `${Math.round(zoom.value * 100)}%`);

onBeforeUnmount(() => {
  closePdfDocument(pdfHandle.value);
  skiaRenderer.dispose();
  revokeUrls();
});

async function onFileChange(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  closePdfDocument(pdfHandle.value);
  pdfHandle.value = null;
  fileName.value = file.name;
  pdfBytes.value = new Uint8Array(await file.arrayBuffer());
  status.value = "正在打开 PDF...";
  pdfHandle.value = await openPdfDocument(pdfBytes.value);
  await loadPage();
}

async function loadPage() {
  if (!pdfBytes.value) return;
  status.value = "正在解析 PDF 页面...";
  revokeUrls();
  const loaded = await loadPdfPage(pdfBytes.value, pageNumber.value, pdfHandle.value);
  page.value = loaded.structure;
  backgroundUrl.value = loaded.backgroundUrl;
  fontFamilies.value = loaded.fontFamilies;
  currentFontAssets = loaded.fontAssets;
  editableTexts.value = loaded.structure.text.map((object) => ({
    id: object.id,
    original: object.content,
    content: object.content,
    object,
    composing: false,
    preview: null
  }));
  await nextTick();
  await renderPdfCanvas();
  selectedTextId.value = null;
  status.value = `已加载第 ${pageNumber.value} 页：${editableTexts.value.length} 个文本对象，${loaded.structure.images.length} 个图片对象`;
}

function revokeUrls() {
  if (backgroundUrl.value) URL.revokeObjectURL(backgroundUrl.value);
  page.value?.images.forEach((image) => {
    if (image.objectUrl) URL.revokeObjectURL(image.objectUrl);
  });
}

function selectText(id: number) {
  void beginTextEdit(id);
}

async function beginTextEdit(id: number) {
  selectedTextId.value = id;
  const text = editableTexts.value.find((item) => item.id === id);
  if (!text || !pdfBytes.value) return;
  try {
    const session = await startTextEdit(pdfBytes.value, id, pdfHandle.value);
    text.preview = {
      object_id: id,
      text: text.content,
      glyphs: session.glyphs,
      bbox: session.bbox,
      overflow: false
    };
  } catch (error) {
    console.warn(`Failed to start text edit for object ${id}`, error);
  }
  await nextTick();
  await renderPdfCanvas();
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

function textViewportRect(text: EditableText): ViewportRect {
  const viewport = currentViewport.value;
  if (!viewport) return { left: 0, top: 0, width: 0, height: 0 };
  if (text.preview) {
    return glyphViewportRect(text) ?? pdfRectToViewportRect(viewport, textPreviewBounds(text));
  }
  const measured = measuredTextRects.value[text.id];
  if (measured) return measured;
  return glyphViewportRect(text) ?? pdfRectToViewportRect(viewport, textPreviewBounds(text));
}

function measuredTextViewportRect(text: EditableText): ViewportRect {
  return textViewportRect(text);
}

function textStyle(text: EditableText) {
  const rect = textViewportRect(text);
  const fontSize = Math.max(text.object.font_size * zoom.value, 1);
  return {
    left: `${rect.left}px`,
    top: `${rect.top}px`,
    width: `${Math.max(rect.width, 2)}px`,
    height: `${Math.max(rect.height, 2)}px`,
    fontFamily: resolvePdfFontFamily(text.object.font_name, fontFamilies.value),
    fontSize: `${fontSize}px`,
    lineHeight: `${fontSize * 1.2}px`
  };
}

function textHitStyle(text: EditableText) {
  const rect = measuredTextViewportRect(text);
  return {
    left: `${rect.left}px`,
    top: `${rect.top}px`,
    width: `${Math.max(rect.width, 2)}px`,
    height: `${Math.max(rect.height, 2)}px`
  };
}

function textPreviewBounds(text: EditableText): Rect {
  return text.preview?.bbox ?? text.object.bounds;
}

function glyphViewportRect(text: EditableText): ViewportRect | null {
  const viewport = currentViewport.value;
  if (!viewport) return null;
  const glyphs = text.preview?.glyphs ?? text.object.glyphs;
  if (!glyphs?.length) return null;

  const rects = glyphs.map((glyph) => pdfRectToViewportRect(viewport, glyph.bbox));
  const left = Math.min(...rects.map((rect) => rect.left));
  const top = Math.min(...rects.map((rect) => rect.top));
  const right = Math.max(...rects.map((rect) => rect.left + rect.width));
  const bottom = Math.max(...rects.map((rect) => rect.top + rect.height));
  return {
    left,
    top,
    width: Math.max(right - left, 1),
    height: Math.max(bottom - top, 1)
  };
}

function previewBoxRect(text: EditableText): ViewportRect {
  const viewport = currentViewport.value;
  if (!viewport) return { left: 0, top: 0, width: 0, height: 0 };
  return pdfRectToViewportRect(viewport, textPreviewBounds(text));
}

function previewGlyphRects(text: EditableText): ViewportRect[] {
  const viewport = currentViewport.value;
  if (!viewport || !text.preview) return [];
  return text.preview.glyphs.map((glyph) => pdfRectToViewportRect(viewport, glyph.bbox));
}

async function renderPdfCanvas() {
  if (!backgroundUrl.value || !skiaCanvas.value || !currentViewport.value) return;
  const sequence = ++renderSequence;
  const hiddenTextIds = new Set(
    editableTexts.value.filter((text) => inlineEditingText.value?.id === text.id).map((text) => text.id)
  );
  const texts = editableTexts.value.map((text) => text.object).sort((left, right) => left.z_index - right.z_index);

  try {
    const measured = await skiaRenderer.render({
      canvas: skiaCanvas.value,
      viewport: currentViewport.value,
      backgroundUrl: backgroundUrl.value,
      texts,
      fonts: currentFontAssets,
      hiddenTextIds
    });
    if (sequence === renderSequence) {
      measuredTextRects.value = Object.fromEntries(measured);
      status.value = status.value.replace("CanvasKit 渲染失败", "CanvasKit 渲染完成");
    }
  } catch (error) {
    if (sequence === renderSequence) {
      console.warn("Failed to render page with CanvasKit", error);
      status.value = "CanvasKit 渲染失败，请查看控制台错误";
    }
  }
}

function shouldHideCanvasTextForEditing(text: EditableText) {
  const editing = inlineEditingText.value;
  if (!editing) return false;
  if (text.id === editing.id) return true;
  return rectsOverlap(expandRect(textPreviewBounds(editing), 2), text.object.bounds);
}

function rectsOverlap(left: Rect, right: Rect) {
  return (
    left.origin.x < right.origin.x + right.size.width &&
    left.origin.x + left.size.width > right.origin.x &&
    left.origin.y < right.origin.y + right.size.height &&
    left.origin.y + left.size.height > right.origin.y
  );
}

function expandRect(rect: Rect, amount: number): Rect {
  return {
    origin: {
      x: rect.origin.x - amount,
      y: rect.origin.y - amount
    },
    size: {
      width: rect.size.width + amount * 2,
      height: rect.size.height + amount * 2
    }
  };
}

async function selectTextFromPointer(event: PointerEvent) {
  const viewport = currentViewport.value;
  const pageElement = event.currentTarget as HTMLElement;
  if (!viewport || !pageElement || !pdfBytes.value) return;
  const bounds = pageElement.getBoundingClientRect();
  const pdfPoint = viewportToPdf(viewport, event.clientX - bounds.left, event.clientY - bounds.top);
  const hit = await hitTestPdf(pdfBytes.value, pageNumber.value, pdfPoint.x, pdfPoint.y, pdfHandle.value);
  if (hit?.object_type === "text") {
    await beginTextEdit(hit.object_id);
  } else {
    selectedTextId.value = null;
    await nextTick();
    await renderPdfCanvas();
  }
}

function startComposition(text: EditableText) {
  text.composing = true;
}

function endComposition(text: EditableText) {
  text.composing = false;
  void previewInlineText(text);
}

async function previewInlineText(text: EditableText) {
  if (text.composing || !pdfBytes.value) return;
  try {
    text.preview = await previewTextLayout(pdfBytes.value, text.id, text.content, pdfHandle.value);
    text.object.content = text.content;
    void nextTick(renderPdfCanvas);
    if (text.preview.overflow) {
      status.value = `文本对象 ${text.id} 超出原始边界，保存前请确认版面`;
    }
  } catch (error) {
    console.warn(`Failed to preview text layout for object ${text.id}`, error);
  }
}

async function commitInlineText(text: EditableText) {
  if (text.composing || !pdfBytes.value || committingTextId.value === text.id) return;
  if (text.content === text.original) {
    selectedTextId.value = null;
    text.preview = null;
    await nextTick();
    await renderPdfCanvas();
    return;
  }

  committingTextId.value = text.id;
  status.value = `正在提交文本对象 ${text.id}...`;
  try {
    if (!text.preview) {
      text.preview = await previewTextLayout(pdfBytes.value, text.id, text.content, pdfHandle.value);
    }
    const updated = await commitTextEdit(pdfBytes.value, text.id, text.content, pdfHandle.value);
    pdfBytes.value = updated;
    text.original = text.content;
    text.object.content = text.content;
    if (text.preview) {
      text.object.bounds = text.preview.bbox;
    }
    selectedTextId.value = null;
    text.preview = null;
    status.value = `已提交文本对象 ${text.id}`;
    await loadPage();
  } catch (error) {
    console.warn(`Failed to commit text edit for object ${text.id}`, error);
    status.value = `文本对象 ${text.id} 提交失败`;
  } finally {
    committingTextId.value = null;
  }
}

async function onInlineKeydown(event: KeyboardEvent, text: EditableText) {
  if (event.key === "Escape") {
    event.preventDefault();
    text.content = text.original;
    text.object.content = text.original;
    text.preview = null;
    selectedTextId.value = null;
    void nextTick(renderPdfCanvas);
  } else if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
    event.preventDefault();
    await commitInlineText(text);
  }
}

function onInlineBlur(text: EditableText) {
  void commitInlineText(text);
}

async function savePdf() {
  if (!pdfBytes.value) return;
  saving.value = true;
  status.value = "正在应用文本修改...";
  const edits = editableTexts.value
    .filter((text) => text.content !== text.original)
    .map((text) => ({ id: text.id, content: text.content }));

  const updated = edits.length > 0 ? await applyTextEdits(pdfBytes.value, edits, pdfHandle.value) : pdfBytes.value;
  pdfBytes.value = updated;
  const blob = new Blob([asBlobPart(updated)], { type: "application/pdf" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName.value.replace(/\.pdf$/i, "") + ".edited.pdf";
  link.click();
  URL.revokeObjectURL(url);
  saving.value = false;
  status.value = edits.length > 0 ? `已保存 ${edits.length} 个文本修改` : "没有文本修改，已导出原文件";
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
        <button :disabled="!pdfBytes" @click="loadPage">加载</button>
      </label>

      <section class="zoom-controls" aria-label="缩放">
        <button title="缩小" :disabled="zoom <= 0.25" @click="zoomOut">−</button>
        <button title="重置缩放" @click="resetZoom">{{ zoomPercent }}</button>
        <button title="放大" :disabled="zoom >= 3" @click="zoomIn">+</button>
      </section>

      <p class="status">{{ status }}</p>

      <section v-if="selectedText" class="editor-panel">
        <h2>编辑文本对象</h2>
        <div class="object-id">ID {{ selectedText.id }}</div>
        <div v-if="selectedTextFontUsage" class="font-meta">
          <div>请求字体：{{ selectedTextFontUsage.requestedFont ?? "(未提供)" }}</div>
          <div>实际显示：{{ selectedTextFontUsage.displayFamily }}</div>
          <div v-if="selectedTextFontUsage.fellBack" class="font-fallback">
            已回退：{{ selectedTextFontUsage.fallbackReason }}
          </div>
          <div v-else class="font-embedded">使用嵌入字体</div>
        </div>
        <textarea
          v-model="selectedText.content"
          spellcheck="false"
          @input="previewInlineText(selectedText)"
        ></textarea>
        <button class="save-button" :disabled="saving || !pdfBytes" @click="savePdf">
          保存 PDF
        </button>
      </section>

      <section class="text-list" v-if="editableTexts.length">
        <h2>文本对象</h2>
        <button
          v-for="text in editableTexts"
          :key="text.id"
          :class="{ selected: text.id === selectedTextId }"
          @click="selectText(text.id)"
        >
          {{ text.content || "(空文本)" }}
        </button>
      </section>
    </aside>

    <section class="canvas-pane">
      <div
        v-if="page && backgroundUrl"
        class="page-viewport"
        :style="pageViewportStyle()"
      >
        <div
          class="page-canvas"
          :style="pageCanvasStyle()"
          @pointerdown="selectTextFromPointer"
        >
          <canvas ref="skiaCanvas" class="background" aria-hidden="true"></canvas>
          <button
            v-for="text in editableTexts"
            :key="text.id"
            :data-text-id="text.id"
            class="text-object"
            :class="{ selected: text.id === selectedTextId }"
            :style="textHitStyle(text)"
            :aria-label="text.content || '空文本对象'"
            tabindex="-1"
            @click="selectText(text.id)"
          ></button>
          <svg
            v-if="inlineEditingText?.preview"
            class="layout-preview"
            :class="{ overflow: inlineEditingText.preview.overflow }"
            aria-hidden="true"
          >
            <rect
              class="layout-preview-box"
              :x="previewBoxRect(inlineEditingText).left"
              :y="previewBoxRect(inlineEditingText).top"
              :width="Math.max(previewBoxRect(inlineEditingText).width, 1)"
              :height="Math.max(previewBoxRect(inlineEditingText).height, 1)"
            ></rect>
            <rect
              v-for="(glyph, index) in previewGlyphRects(inlineEditingText)"
              :key="index"
              class="layout-preview-glyph"
              :x="glyph.left"
              :y="glyph.top"
              :width="Math.max(glyph.width, 1)"
              :height="Math.max(glyph.height, 1)"
            ></rect>
          </svg>
          <textarea
            v-if="inlineEditingText"
            v-model="inlineEditingText.content"
            class="inline-text-editor"
            :style="textStyle(inlineEditingText)"
            spellcheck="false"
            @compositionstart="startComposition(inlineEditingText)"
            @compositionend="endComposition(inlineEditingText)"
            @input="previewInlineText(inlineEditingText)"
            @keydown="onInlineKeydown($event, inlineEditingText)"
            @blur="onInlineBlur(inlineEditingText)"
            @pointerdown.stop
          ></textarea>
        </div>
      </div>
      <div v-else class="empty-state">加载 PDF 后显示可编辑页面</div>
    </section>
  </main>
</template>

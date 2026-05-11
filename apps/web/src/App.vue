<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import type { PageStructure, StructuredTextObject } from "./pdfEditor";
import {
  applyTextEdits,
  asBlobPart,
  closePdfDocument,
  loadPdfPage,
  openPdfDocument,
  resolvePdfFontFamily
} from "./pdfEditor";

interface EditableText {
  id: number;
  original: string;
  content: string;
  object: StructuredTextObject;
  visualX: number;
  visualY: number;
}

const pdfBytes = ref<Uint8Array | null>(null);
const pdfHandle = ref<number | null>(null);
const fileName = ref("");
const pageNumber = ref(1);
const status = ref("选择 PDF 后加载页面");
const page = ref<PageStructure | null>(null);
const backgroundUrl = ref<string | null>(null);
const fontFamilies = ref<Record<string, string>>({});
const editableTexts = ref<EditableText[]>([]);
const selectedTextId = ref<number | null>(null);
const saving = ref(false);
const zoom = ref(1);

const selectedText = computed(() =>
  editableTexts.value.find((text) => text.id === selectedTextId.value) ?? editableTexts.value[0] ?? null
);
const zoomPercent = computed(() => `${Math.round(zoom.value * 100)}%`);

onBeforeUnmount(() => {
  closePdfDocument(pdfHandle.value);
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
  editableTexts.value = loaded.structure.text.map((object) => ({
    id: object.id,
    original: object.content,
    content: object.content,
    object,
    visualX: object.transform[4],
    visualY: object.transform[5]
  }));
  selectedTextId.value = editableTexts.value[0]?.id ?? null;
  status.value = `已加载第 ${pageNumber.value} 页：${editableTexts.value.length} 个文本对象，${loaded.structure.images.length} 个图片对象`;
}

function revokeUrls() {
  if (backgroundUrl.value) URL.revokeObjectURL(backgroundUrl.value);
  page.value?.images.forEach((image) => {
    if (image.objectUrl) URL.revokeObjectURL(image.objectUrl);
  });
}

function selectText(id: number) {
  selectedTextId.value = id;
}

function zoomIn() {
  zoom.value = clampZoom(zoom.value + 0.1);
}

function zoomOut() {
  zoom.value = clampZoom(zoom.value - 0.1);
}

function resetZoom() {
  zoom.value = 1;
}

function clampZoom(value: number) {
  return Math.min(3, Math.max(0.25, Math.round(value * 10) / 10));
}

function pageViewportStyle(pageInfo: PageStructure["page"]) {
  return {
    width: `${pageInfo.size.width * zoom.value}px`,
    height: `${pageInfo.size.height * zoom.value}px`
  };
}

function pageCanvasStyle(pageInfo: PageStructure["page"]) {
  return {
    width: `${pageInfo.size.width}px`,
    height: `${pageInfo.size.height}px`,
    transform: `scale(${zoom.value})`
  };
}

function textStyle(text: EditableText) {
  const pageHeight = page.value?.page.size.height ?? 0;
  const fontSize = effectiveFontSize(text.object);
  const height = Math.max(fontSize * 1.25, 8);
  const width = Math.max(text.object.bounds.size.width, 1);
  const scaleX = textHorizontalScale(text.object.content, fontSize, width);
  return {
    left: `${text.visualX}px`,
    top: `${pageHeight - text.visualY}px`,
    width: `${width}px`,
    minHeight: `${height}px`,
    fontSize: `${fontSize}px`,
    lineHeight: "1.15",
    "--text-scale-x": `${scaleX}`,
    transform: `rotate(${-text.object.angle_degrees}deg) translateY(-${fontSize}px)`,
    transformOrigin: "left bottom",
    fontFamily: resolvePdfFontFamily(text.object.font_name, fontFamilies.value),
    color: cssColor(text.object.color)
  };
}

function effectiveFontSize(text: StructuredTextObject) {
  return effectiveObjectFontSize(text);
}

function effectiveObjectFontSize(text: StructuredTextObject) {
  const [a, b, c, d] = text.transform;
  const xScale = Math.hypot(a, b);
  const yScale = Math.hypot(c, d);
  const transformSize = Math.max(xScale, yScale);
  return Math.max(transformSize || text.font_size, 1);
}

function textHorizontalScale(content: string, fontSize: number, targetWidth: number) {
  const estimatedWidth = estimateBrowserTextWidth(content, fontSize);
  if (estimatedWidth <= 0 || targetWidth <= 0) return 1;
  return Math.min(1.2, Math.max(0.35, targetWidth / estimatedWidth));
}

function estimateBrowserTextWidth(content: string, fontSize: number) {
  let units = 0;
  for (const character of content || " ") {
    if (character === " " || character === "\u00a0") {
      units += 0.28;
    } else if (/[\u0000-\u007f]/.test(character)) {
      units += /[ilI.,|]/.test(character) ? 0.28 : 0.62;
    } else {
      units += 1;
    }
  }
  return Math.max(units * fontSize, 1);
}

function cssColor(color: StructuredTextObject["color"]) {
  const alpha = Math.max(0.15, color.a / 255);
  return `rgba(${color.r}, ${color.g}, ${color.b}, ${alpha})`;
}

function imageStyle(image: NonNullable<PageStructure["images"][number]>) {
  const bounds = image.bounds;
  const pageHeight = page.value?.page.size.height ?? 0;
  return {
    left: `${bounds.origin.x}px`,
    top: `${pageHeight - bounds.origin.y - bounds.size.height}px`,
    width: `${bounds.size.width}px`,
    height: `${bounds.size.height}px`,
    transform: `rotate(${-image.angle_degrees}deg)`,
    transformOrigin: "left top"
  };
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
        <textarea v-model="selectedText.content" spellcheck="false"></textarea>
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
        :style="pageViewportStyle(page.page)"
      >
        <div
          class="page-canvas"
          :style="pageCanvasStyle(page.page)"
        >
          <img class="background" :src="backgroundUrl" alt="" />
          <img
            v-for="image in page.images"
            :key="image.id"
            class="image-object"
            :src="image.objectUrl"
            :style="imageStyle(image)"
            alt=""
          />
          <button
            v-for="text in editableTexts"
            :key="text.id"
            :data-text-id="text.id"
            class="text-object"
            :class="{ selected: text.id === selectedTextId }"
            :style="textStyle(text)"
            @click="selectText(text.id)"
          >
            <span>{{ text.content }}</span>
          </button>
        </div>
      </div>
      <div v-else class="empty-state">加载 PDF 后显示可编辑页面</div>
    </section>
  </main>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref } from "vue";
import * as pdfjsLib from "pdfjs-dist";
import pdfWorkerUrl from "pdfjs-dist/build/pdf.worker.mjs?url";
import type { PageStructure, StructuredTextObject } from "./pdfEditor";
import {
  applyTextEdits,
  asBlobPart,
  closePdfDocument,
  loadPdfPage,
  openPdfDocument
} from "./pdfEditor";

pdfjsLib.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;

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
const pageCanvas = ref<HTMLCanvasElement | null>(null);
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
  editableTexts.value = loaded.structure.text.map((object) => ({
    id: object.id,
    original: object.content,
    content: object.content,
    object,
    visualX: object.transform[4],
    visualY: object.transform[5]
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
  const bounds = text.object.bounds;
  return {
    left: `${bounds.origin.x}px`,
    top: `${pageHeight - bounds.origin.y - bounds.size.height}px`,
    width: `${Math.max(bounds.size.width, 2)}px`,
    height: `${Math.max(bounds.size.height, 2)}px`,
    transform: `rotate(${-text.object.angle_degrees}deg)`,
    transformOrigin: "left top"
  };
}

async function renderPdfCanvas() {
  if (!pdfBytes.value || !pageCanvas.value || !page.value) return;
  const loadingTask = pdfjsLib.getDocument({ data: pdfBytes.value.slice() });
  const pdfDocument = await loadingTask.promise;
  try {
    const pdfPage = await pdfDocument.getPage(pageNumber.value);
    const viewport = pdfPage.getViewport({ scale: 1 });
    const ratio = window.devicePixelRatio || 1;
    const canvas = pageCanvas.value;
    const context = canvas.getContext("2d");
    if (!context) return;

    canvas.width = Math.ceil(viewport.width * ratio);
    canvas.height = Math.ceil(viewport.height * ratio);
    canvas.style.width = `${viewport.width}px`;
    canvas.style.height = `${viewport.height}px`;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    await pdfPage.render({ canvas, canvasContext: context, viewport }).promise;
  } finally {
    await pdfDocument.destroy();
  }
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
          <canvas ref="pageCanvas" class="background" aria-hidden="true"></canvas>
          <button
            v-for="text in editableTexts"
            :key="text.id"
            :data-text-id="text.id"
            class="text-object"
            :class="{ selected: text.id === selectedTextId }"
            :style="textStyle(text)"
            :aria-label="text.content || '空文本对象'"
            @click="selectText(text.id)"
          ></button>
        </div>
      </div>
      <div v-else class="empty-state">加载 PDF 后显示可编辑页面</div>
    </section>
  </main>
</template>

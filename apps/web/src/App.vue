<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref } from "vue";
import type { LoadedFontAsset, PageStructure } from "./pdfEditor";
import { closePdfDocument, loadPdfPage, openPdfDocument } from "./pdfEditor";
import { SkiaPageRenderer } from "./skiaRenderer";
import { composePageTransform, type PageViewport } from "./viewport";

const pdfBytes = ref<Uint8Array | null>(null);
const pdfHandle = ref<number | null>(null);
const pageNumber = ref(1);
const status = ref("选择 PDF 后加载页面");
const page = ref<PageStructure | null>(null);
const backgroundUrl = ref<string | null>(null);
const skiaCanvas = ref<HTMLCanvasElement | null>(null);
const zoom = ref(1);

const skiaRenderer = new SkiaPageRenderer();
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

const zoomPercent = computed(() => `${Math.round(zoom.value * 100)}%`);
const textCount = computed(() => page.value?.text.length ?? 0);
const imageCount = computed(() => page.value?.images.length ?? 0);

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
  page.value = null;
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
  currentFontAssets = loaded.fontAssets;

  await nextTick();
  await renderPdfCanvas();
  status.value = `已加载第 ${pageNumber.value} 页：${loaded.structure.text.length} 个文本对象，${loaded.structure.images.length} 个图片对象`;
}

async function renderPdfCanvas() {
  if (!backgroundUrl.value || !skiaCanvas.value || !currentViewport.value || !page.value) return;
  try {
    await skiaRenderer.render({
      canvas: skiaCanvas.value,
      viewport: currentViewport.value,
      backgroundUrl: backgroundUrl.value,
      texts: page.value.text,
      fonts: currentFontAssets
    });
  } catch (error) {
    console.warn("Failed to render page with CanvasKit", error);
    status.value = "CanvasKit 渲染失败，请查看控制台错误";
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
        <button :disabled="!pdfBytes" @click="loadPage">加载</button>
      </label>

      <section class="zoom-controls" aria-label="缩放">
        <button title="缩小" :disabled="zoom <= 0.25" @click="zoomOut">−</button>
        <button title="重置缩放" @click="resetZoom">{{ zoomPercent }}</button>
        <button title="放大" :disabled="zoom >= 3" @click="zoomIn">+</button>
      </section>

      <p class="status">{{ status }}</p>

      <section v-if="page" class="render-summary">
        <h2>渲染摘要</h2>
        <div>文本对象：{{ textCount }}</div>
        <div>图片对象：{{ imageCount }}</div>
        <div>旋转角度：{{ page.page.rotation ?? 0 }}°</div>
      </section>
    </aside>

    <section class="canvas-pane">
      <div v-if="page && backgroundUrl" class="page-viewport" :style="pageViewportStyle()">
        <div class="page-canvas" :style="pageCanvasStyle()">
          <canvas ref="skiaCanvas" class="background" aria-label="PDF canvas render"></canvas>
        </div>
      </div>
      <div v-else class="empty-state">加载 PDF 后显示 CanvasKit 渲染结果</div>
    </section>
  </main>
</template>

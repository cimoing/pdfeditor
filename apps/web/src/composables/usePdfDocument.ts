import { ref, shallowRef, computed } from "vue";
import { openPdfDocument, loadPdfPage, closePdfDocument } from "../pdfEditor";
import type { PageStructure, LoadedFontAsset } from "../pdfEditor";
import { composePageTransform, type PageViewport } from "../viewport";

export function usePdfDocument() {
  const pdfBytes = shallowRef<Uint8Array | null>(null);
  const pdfHandle = ref<number | null>(null);
  const pdfFileName = ref("document.pdf");
  const pageNumber = ref(1);
  const status = ref("选择 PDF 后加载页面");
  const page = ref<PageStructure | null>(null);
  const backgroundUrl = ref<string | null>(null);
  const zoom = ref(1);
  const fontFamilies = ref<Record<string, string>>({});
  const fontAssets = shallowRef<LoadedFontAsset[]>([]);

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

  function revokeUrls() {
    if (backgroundUrl.value) {
      URL.revokeObjectURL(backgroundUrl.value);
      backgroundUrl.value = null;
    }
    page.value?.images.forEach((image) => {
      if (image.objectUrl) URL.revokeObjectURL(image.objectUrl);
    });
  }

  function cleanup() {
    closePdfDocument(pdfHandle.value);
    revokeUrls();
    pdfHandle.value = null;
    page.value = null;
    pdfBytes.value = null;
  }

  async function openFile(file: File) {
    cleanup();
    pdfFileName.value = file.name;
    const buffer = await file.arrayBuffer();
    pdfBytes.value = new Uint8Array(buffer);
    status.value = "正在打开 PDF...";
    pdfHandle.value = await openPdfDocument(pdfBytes.value);
  }

  async function loadCurrentPage() {
    if (!pdfBytes.value) return null;
    status.value = "正在解析 PDF 页面...";
    revokeUrls();
    const loaded = await loadPdfPage(pdfBytes.value, pageNumber.value, pdfHandle.value);
    page.value = loaded.structure;
    backgroundUrl.value = loaded.backgroundUrl;
    fontAssets.value = loaded.fontAssets;
    fontFamilies.value = loaded.fontFamilies;
    status.value = `已加载第 ${pageNumber.value} 页：${loaded.structure.text.length} 个文本对象，${loaded.structure.images.length} 个图片对象`;
    return loaded;
  }

  return {
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
    revokeUrls,
    cleanup,
    openFile,
    loadCurrentPage
  };
}

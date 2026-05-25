<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watchEffect } from "vue";
import type { LayoutGlyph, StructuredImageObject, StructuredTextObject } from "./pdfEditor";
import {
  commitTextEdit,
  describePdfFontUsage,
  hitTestPdf,
  previewTextLayout,
  resolvePdfFontFamily,
  startTextEdit
} from "./pdfEditor";
import { pdfRectToViewportRect, viewportToPdf, type Matrix2D } from "./viewport";
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

const fontAssetMap = computed(() => new Map(fontAssets.value.map((font) => [font.resource_name, font])));
const inlineEditor = ref<HTMLTextAreaElement | null>(null);
const zoomPercent = computed(() => `${Math.round(zoom.value * 100)}%`);

// ── Edit-box resize state ──────────────────────────────────────────────────
/** Width override in viewport pixels set by dragging the right edge. */
const editBoxWidthOverride = ref<number | null>(null);
/** Left offset override in viewport pixels set by dragging the left edge. */
const editBoxLeftOverride = ref<number | null>(null);

interface EdgeDragState {
  edge: "left" | "right";
  startX: number;
  initialLeft: number;
  initialWidth: number;
}
let edgeDragState: EdgeDragState | null = null;
const textCount = computed(() => page.value?.text.length ?? 0);
const imageCount = computed(() => page.value?.images.length ?? 0);
const pageTextObjects = computed(() => page.value?.text ?? []);
const renderImageObjects = computed(() => page.value?.images.filter((image) => image.objectUrl) ?? []);

const selectedTextObject = computed<StructuredTextObject | null>(
  () => page.value?.text.find((item) => item.id === selectedTextId.value) ?? null
);
const activeGroupObjectIds = computed(() => {
  return selectedTextId.value != null ? [selectedTextId.value] : [];
});
const selectedFontUsage = computed(() =>
  describePdfFontUsage(editSession.value?.font_id ?? selectedTextObject.value?.font_name ?? null, fontFamilies.value)
);

// ── Fallback-font width normalisation ─────────────────────────────────────
/**
 * Measures the average glyph advance width (in CSS px) for an array of single
 * characters rendered in `cssFont` (e.g. `"14px KaiTi, 'Kaiti SC', serif"`).
 * Uses the Canvas 2D text-measurement API — no DOM insertion, no reflow.
 */
function measureAverageCharWidth(cssFont: string, chars: string[]): number {
  if (!chars.length) return 0;
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) return 0;
  ctx.font = cssFont;
  const total = chars.reduce((sum, ch) => sum + ctx.measureText(ch).width, 0);
  return total / chars.length;
}

/**
 * Horizontal scale factor applied to the inline textarea so that fallback-font
 * character widths visually match the PDF font's glyph advances.
 * Stays at 1.0 when the embedded font is successfully loaded.
 */
const editorFontScaleX = ref(1.0);

watchEffect(() => {
  const session = editSession.value;
  const text = selectedTextObject.value;
  const viewport = currentViewport.value;
  const fontUsage = selectedFontUsage.value;

  // No correction needed when the actual embedded font is in use.
  if (!session?.glyphs.length || !text || !viewport || !fontUsage.fellBack) {
    editorFontScaleX.value = 1.0;
    return;
  }

  // Average advance per character from the PDF font metrics.
  // editSession.glyphs already incorporates TJ compression (layout_preview_glyphs
  // in the backend applies the compression factor before returning the session).
  const validGlyphs = session.glyphs.filter((g) => g.advance > 0);
  if (!validGlyphs.length) {
    editorFontScaleX.value = 1.0;
    return;
  }
  const avgPdfAdvance = validGlyphs.reduce((s, g) => s + g.advance, 0) / validGlyphs.length;

  // Convert PDF advance (in normalised glyph units) to viewport pixels.
  const effFs = effectiveFontSize(text);
  const targetPx = avgPdfAdvance * effFs * viewport.zoom;

  // Measure the fallback font at the exact CSS size we will use in the textarea.
  const cssFontSizePx = Math.max(10, effFs * viewport.zoom);
  const cssFont = `${cssFontSizePx}px ${fontUsage.cssFontFamily}`;
  const measuredPx = measureAverageCharWidth(cssFont, validGlyphs.map((g) => g.ch));

  if (measuredPx < 0.5) {
    editorFontScaleX.value = 1.0;
    return;
  }

  // Clamp to a reasonable range: very large corrections usually indicate a
  // measurement anomaly (invisible characters, unmapped glyphs, etc.).
  editorFontScaleX.value = Math.max(0.5, Math.min(2.0, targetPx / measuredPx));
});

const renderTextObjects = computed(() => {
  const pageText = page.value?.text ?? [];
  if (!layoutPreview.value || !selectedTextObject.value || !editSession.value) {
    return pageText;
  }
  const previewObject: StructuredTextObject = {
    ...selectedTextObject.value,
    bounds: layoutPreview.value.bbox,
    content: layoutPreview.value.text,
    font_name: editSession.value.font_id,
    font_size: editSession.value.font_size,
    transform: editSession.value.matrix,
    glyphs: layoutPreview.value.glyphs
  };
  return pageText.map((text) => (text.id === selectedTextObject.value!.id ? previewObject : text));
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

/** Layout styles (position, size) for the editor wrapper div. */
const inlineEditorWrapStyle = computed(() => {
  const text = selectedTextObject.value;
  const viewport = currentViewport.value;
  if (!text || !viewport || !editSession.value) return {};
  const rect = pdfRectToViewportRect(viewport, editSession.value.bbox);
  const fontSize = Math.max(10, effectiveFontSize(text) * viewport.zoom);
  const naturalWidth = Math.max(rect.width, fontSize * 4);
  return {
    left: `${editBoxLeftOverride.value ?? rect.left}px`,
    top: `${rect.top}px`,
    width: `${editBoxWidthOverride.value ?? naturalWidth}px`,
    minHeight: `${Math.max(rect.height, fontSize * 1.4)}px`
  };
});

/** Typography + scale styles for the textarea itself. */
const inlineEditorStyle = computed(() => {
  const text = selectedTextObject.value;
  const viewport = currentViewport.value;
  if (!text || !viewport || !editSession.value) return {};
  const fontSize = Math.max(10, effectiveFontSize(text) * viewport.zoom);
  const scale = editorFontScaleX.value;
  const needsScale = Math.abs(scale - 1.0) > 0.005;
  return {
    fontFamily: svgFontFamily(editSession.value.font_id ?? text.font_name),
    fontSize: `${fontSize}px`,
    fontWeight: fontWeightFor(editSession.value.font_id ?? text.font_name),
    color: colorToCss(text.color),
    lineHeight: "1.2",
    // When the fallback font's character widths differ from the PDF font, compress
    // or stretch the textarea content horizontally so character widths match.
    // The compensating `width` ensures the scaled textarea still fills the wrapper
    // (scaleX anchors at left, so visual right edge stays at wrapper_width).
    ...(needsScale
      ? {
          transform: `scaleX(${scale.toFixed(5)})`,
          transformOrigin: "left top",
          width: `${(100 / scale).toFixed(5)}%`
        }
      : {})
  };
});

const canSaveEdit = computed(() => {
  if (!selectedTextObject.value || !editSession.value || isSavingEdit.value || isPreparingEdit.value) {
    return false;
  }
  return draftText.value !== editSession.value.original_text;
});

/** Overflow only matters when the user actually changed the text from the original. */
const hasEffectiveOverflow = computed(() =>
  Boolean(layoutPreview.value?.overflow) && draftText.value !== editSession.value?.original_text
);

const previewStatus = computed(() => {
  if (!selectedTextObject.value) return "点击页面高亮框或左侧列表，开始编辑文本对象。";
  if (isPreparingEdit.value) return "正在准备文本编辑会话...";
  if (!layoutPreview.value) return "修改文本后会实时生成布局预览。";
  if (draftText.value === editSession.value?.original_text) return "当前内容与原始文本一致。";
  if (layoutPreview.value.overflow) return "当前文本超出原始文本边界，可以保存。";
  return "预览通过，可以保存到当前 PDF。";
});

onBeforeUnmount(() => {
  cleanupPdf();
  clearPreviewTimer();
  stopEdgeDrag();
});

function resetEditBoxOverrides() {
  editBoxWidthOverride.value = null;
  editBoxLeftOverride.value = null;
}

function stopEdgeDrag() {
  if (!edgeDragState) return;
  edgeDragState = null;
  window.removeEventListener("pointermove", onEdgeDragMove);
  window.removeEventListener("pointerup", onEdgeDragEnd);
}

function onEdgeDragStart(event: PointerEvent, edge: "left" | "right") {
  const text = selectedTextObject.value;
  const viewport = currentViewport.value;
  if (!text || !viewport || !editSession.value) return;
  const rect = pdfRectToViewportRect(viewport, editSession.value.bbox);
  const fontSize = Math.max(10, effectiveFontSize(text) * viewport.zoom);
  const naturalWidth = Math.max(rect.width, fontSize * 4);
  const initialLeft = editBoxLeftOverride.value ?? rect.left;
  const initialWidth = editBoxWidthOverride.value ?? naturalWidth;
  edgeDragState = { edge, startX: event.clientX, initialLeft, initialWidth };
  window.addEventListener("pointermove", onEdgeDragMove);
  window.addEventListener("pointerup", onEdgeDragEnd);
  event.preventDefault();
}

function onEdgeDragMove(event: PointerEvent) {
  if (!edgeDragState) return;
  const dx = event.clientX - edgeDragState.startX;
  if (edgeDragState.edge === "right") {
    editBoxWidthOverride.value = Math.max(40, edgeDragState.initialWidth + dx);
  } else {
    const newWidth = Math.max(40, edgeDragState.initialWidth - dx);
    editBoxWidthOverride.value = newWidth;
    editBoxLeftOverride.value = edgeDragState.initialLeft + (edgeDragState.initialWidth - newWidth);
  }
}

function onEdgeDragEnd() {
  stopEdgeDrag();
}

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
  return loaded;
}

function onLoadPageClick() {
  void loadPage();
}

async function beginTextEdit(objectId: number) {
  if (!pdfBytes.value) return;
  resetEditBoxOverrides();
  clearPreviewTimer();
  selectedTextId.value = objectId;
  isPreparingEdit.value = true;
  layoutPreview.value = null;
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
    await nextTick();
    inlineEditor.value?.focus();
    inlineEditor.value?.select();
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

function onInlineEditorKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    clearSelection();
    return;
  }
  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
    event.preventDefault();
    void saveTextEdit({ closeAfterSave: true });
  }
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
  } catch (error) {
    console.error(error);
    if (requestId !== getPreviewToken() || selectionToken !== getSelectionToken()) {
      return;
    }
    status.value = error instanceof Error ? error.message : "生成文本布局预览失败";
  }
}

async function saveTextEdit(options: { closeAfterSave?: boolean } = {}) {
  if (!pdfBytes.value || !selectedTextId.value || !canSaveEdit.value) return;
  clearPreviewTimer();
  isSavingEdit.value = true;
  status.value = `正在保存文本对象 ${selectedTextId.value}...`;
  const objectId = selectedTextId.value;
  const savedText = draftText.value;
  const savedBounds = layoutPreview.value?.bbox ?? editSession.value?.bbox ?? selectedTextObject.value?.bounds ?? null;
  // Overflow saves skip re-opening the edit session to avoid an extra WASM round-trip.
  const closeAfterSave = options.closeAfterSave ?? Boolean(layoutPreview.value?.overflow);

  try {
    const updatedBytes = await commitTextEdit(pdfBytes.value, objectId, savedText, pdfHandle.value);
    pdfBytes.value = new Uint8Array(updatedBytes);
    if (closeAfterSave) {
      await loadPage();
      clearEditingState();
    } else {
      const loaded = await loadPage();
      const nextObjectId = loaded ? findSavedTextObjectId(loaded.structure.text, objectId, savedText, savedBounds) : null;
      if (nextObjectId == null) {
        clearEditingState();
      } else {
        await beginTextEdit(nextObjectId);
      }
    }
    status.value = `文本对象 ${objectId} 已保存`;
  } catch (error) {
    console.error(error);
    status.value = error instanceof Error ? error.message : "保存文本编辑失败";
  } finally {
    isSavingEdit.value = false;
  }
}

function findSavedTextObjectId(
  textObjects: StructuredTextObject[],
  previousId: number,
  savedText: string,
  savedBounds: StructuredTextObject["bounds"] | null
) {
  if (textObjects.some((item) => item.id === previousId)) {
    return previousId;
  }

  const normalizedSavedText = normalizeTextForMatch(savedText);
  const textMatches = textObjects.filter((item) => normalizeTextForMatch(item.content) === normalizedSavedText);
  const candidates = textMatches.length ? textMatches : textObjects;
  if (!candidates.length) return null;
  if (!savedBounds) return candidates[0].id;

  return candidates
    .map((item) => ({ id: item.id, distance: rectCenterDistanceSquared(item.bounds, savedBounds) }))
    .sort((left, right) => left.distance - right.distance)[0].id;
}

function normalizeTextForMatch(value: string) {
  return value.replace(/\s+/g, " ").trim();
}

function rectCenterDistanceSquared(left: StructuredTextObject["bounds"], right: StructuredTextObject["bounds"]) {
  const leftX = left.origin.x + left.size.width / 2;
  const leftY = left.origin.y + left.size.height / 2;
  const rightX = right.origin.x + right.size.width / 2;
  const rightY = right.origin.y + right.size.height / 2;
  return (leftX - rightX) ** 2 + (leftY - rightY) ** 2;
}

async function saveTextEditOnBlur() {
  if (isPreparingEdit.value || isSavingEdit.value) return;
  if (!selectedTextObject.value || !editSession.value) return;
  if (draftText.value === editSession.value.original_text) {
    clearSelection();
    return;
  }
  await saveTextEdit({ closeAfterSave: true });
}

function clearSelection() {
  resetEditBoxOverrides();
  clearEditingState();
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

async function onCanvasPointerDown(event: PointerEvent) {
  if (!pdfBytes.value || !currentViewport.value || !page.value) return;

  const target = event.currentTarget as HTMLElement;
  const rect = target.getBoundingClientRect();
  const offsetX = event.clientX - rect.left;
  const offsetY = event.clientY - rect.top;
  const pdfPoint = viewportToPdf(currentViewport.value, offsetX, offsetY);

  try {
    const hitResult = await hitTestPdf(pdfBytes.value, pageNumber.value, pdfPoint.x, pdfPoint.y, pdfHandle.value);
    if (hitResult && hitResult.object_type === "text") {
      await beginTextEdit(hitResult.object_id);
    } else if (editSession.value) {
      await saveTextEditOnBlur();
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

function backgroundStyle() {
  const viewport = currentViewport.value;
  const pageInfo = page.value?.page;
  if (!viewport || !pageInfo) return {};
  const baseWidth = pageInfo.size.width * viewport.zoom;
  const baseHeight = pageInfo.size.height * viewport.zoom;
  let transform = "none";
  switch (viewport.rotation) {
    case 90:
      transform = `translate(${viewport.width}px, 0px) rotate(90deg)`;
      break;
    case 180:
      transform = `translate(${viewport.width}px, ${viewport.height}px) rotate(180deg)`;
      break;
    case 270:
      transform = `translate(0px, ${viewport.height}px) rotate(-90deg)`;
      break;
    default:
      break;
  }
  return {
    width: `${baseWidth}px`,
    height: `${baseHeight}px`,
    transform,
    transformOrigin: "left top"
  };
}

function svgImageTransform(image: StructuredImageObject) {
  const viewport = currentViewport.value;
  if (!viewport) return "";
  const matrix = multiplyMatrices(
    multiplyMatrices(viewport.transform, image.transform),
    [1, 0, 0, -1, 0, 1]
  );
  return `matrix(${matrix.map((value) => roundSvg(value)).join(" ")})`;
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

function svgTextTransform(text: StructuredTextObject) {
  const viewport = currentViewport.value;
  if (!viewport) return "";
  const matrix = multiplyMatrices(multiplyMatrices(viewport.transform, text.transform), [1, 0, 0, -1, 0, 0]);
  return `matrix(${matrix.map((value) => roundSvg(value)).join(" ")})`;
}

function svgTextLength(text: StructuredTextObject) {
  if (text.content.includes("\n")) return undefined;
  if (!text.glyphs?.length) {
    // For fonts with punctuation width substitution (标点宽度替换) and no per-glyph
    // positions, derive textLength from the object's actual bounds width so that
    // lengthAdjust="spacingAndGlyphs" distributes the text (including narrow
    // punctuation) across the correct PDF-space width.
    if (text.punct_width_squeeze) {
      const fs = effectiveFontSize(text);
      if (fs > 0.01) return roundSvg(text.bounds.size.width / fs);
    }
    return undefined;
  }
  if (!shouldTrustSvgTextLength(text)) return undefined;
  return roundSvg(text.glyphs.reduce((sum, glyph) => sum + Math.max(glyph.advance, 0), 0));
}

function shouldTrustSvgTextLength(text: StructuredTextObject) {
  const glyphs = text.glyphs ?? [];
  const chars = Array.from(text.content);
  if (glyphs.length !== chars.length) return false;
  if (glyphs.length <= 1) return true;
  for (let index = 0; index < chars.length; index += 1) {
    if (glyphs[index].ch !== chars[index]) return false;
  }
  const unitAdvanceCount = glyphs.filter((glyph) => Math.abs(glyph.advance - 1) < 0.001).length;
  return unitAdvanceCount / glyphs.length < 0.8;
}

function svgTextLines(text: StructuredTextObject) {
  return text.content.split("\n");
}

function svgTextRuns(text: StructuredTextObject) {
  const runs = (text.runs ?? []).filter((run) => run.content);
  if (!runs.length || text.content.includes("\n")) {
    return null;
  }
  return runs;
}

function svgFontFamily(fontName: string | null) {
  return resolvePdfFontFamily(fontName, fontFamilies.value);
}

function svgFill(text: StructuredTextObject) {
  return shouldFillText(text.rendering_mode) ? colorToCss(text.color) : "none";
}

function svgStroke(text: StructuredTextObject) {
  return shouldStrokeText(text.rendering_mode) ? colorToCss(text.stroke_color) : "none";
}

function svgStrokeWidth(text: StructuredTextObject) {
  if (!shouldStrokeText(text.rendering_mode)) return undefined;
  return roundSvg(text.stroke_width / Math.max(effectiveFontSize(text), 1));
}

function svgPaintOrder(text: StructuredTextObject) {
  if (shouldFillText(text.rendering_mode) && shouldStrokeText(text.rendering_mode)) {
    return "stroke fill";
  }
  return undefined;
}

/**
 * Builds a CSS `font-feature-settings` value string for the given text object,
 * e.g. `"kern" 1, "palt" 1`.  Returns undefined when no features are detected.
 */
function svgFontFeatureSettings(text: StructuredTextObject): string | undefined {
  const features = text.font_features;
  if (!features?.length) return undefined;
  return features.map((f) => `"${f}" 1`).join(", ");
}

function fontWeightFor(fontName: string | null) {
  if (!fontName) return undefined;
  const font = fontAssetMap.value.get(fontName);
  return font?.font_weight ? String(font.font_weight) : undefined;
}

function colorToCss(color: { r: number; g: number; b: number; a: number }) {
  return `rgba(${color.r}, ${color.g}, ${color.b}, ${Math.max(0, Math.min(color.a / 255, 1))})`;
}

function effectiveFontSize(text: StructuredTextObject) {
  const yAxisScale = Math.hypot(text.transform[2], text.transform[3]);
  if (yAxisScale > 0.01) return yAxisScale;
  const xAxisScale = Math.hypot(text.transform[0], text.transform[1]);
  if (xAxisScale > 0.01) return xAxisScale;
  return text.font_size > 0 ? text.font_size : 1;
}

function shouldFillText(renderingMode: number) {
  return renderingMode === 0 || renderingMode === 2 || renderingMode === 4 || renderingMode === 6;
}

function shouldStrokeText(renderingMode: number) {
  return renderingMode === 1 || renderingMode === 2 || renderingMode === 5 || renderingMode === 6;
}

function multiplyMatrices(left: Matrix2D, right: Matrix2D): Matrix2D {
  return [
    left[0] * right[0] + left[2] * right[1],
    left[1] * right[0] + left[3] * right[1],
    left[0] * right[2] + left[2] * right[3],
    left[1] * right[2] + left[3] * right[3],
    left[0] * right[4] + left[2] * right[5] + left[4],
    left[1] * right[4] + left[3] * right[5] + left[5]
  ];
}

function roundSvg(value: number) {
  return Number(value.toFixed(4));
}

/**
 * Returns the glyph array when it can be used for per-glyph absolute SVG rendering:
 * - glyphs must be present and non-empty
 * - glyph count must match the character count of the content
 * - each glyph.ch must match the corresponding character
 * Returns null when the fallback textLength rendering should be used instead.
 *
 * Each returned glyph is rendered with its own textLength constraint so the ink
 * width exactly matches the Tm-derived advance, reproducing PDF-viewer behaviour
 * even when a fallback font is used for display.
 */
function glyphsForSvg(text: StructuredTextObject): LayoutGlyph[] | null {
  const glyphs = text.glyphs;
  if (!glyphs?.length) return null;
  const chars = Array.from(text.content);
  if (glyphs.length !== chars.length) return null;
  for (let i = 0; i < glyphs.length; i++) {
    if (glyphs[i].ch !== chars[i]) return null;
  }
  return glyphs;
}

/**
 * Builds an SVG transform string that positions a single glyph at its absolute
 * PDF-space coordinates (glyph.x, glyph.y).  The orientation (scale + rotation)
 * is taken from the parent text object's transform[0..3], but the translation is
 * replaced with the glyph's own position.  This guarantees each glyph lands at the
 * exact position described by the original PDF Tm operator, regardless of how the
 * containing StructuredTextObject was assembled (merged scatter group or otherwise).
 */
function svgGlyphTransform(glyph: LayoutGlyph, text: StructuredTextObject): string {
  const viewport = currentViewport.value;
  if (!viewport) return "";
  const glyphMatrix: Matrix2D = [
    text.transform[0],
    text.transform[1],
    text.transform[2],
    text.transform[3],
    glyph.x,
    glyph.y
  ];
  const matrix = multiplyMatrices(
    multiplyMatrices(viewport.transform, glyphMatrix),
    [1, 0, 0, -1, 0, 0]
  );
  return `matrix(${matrix.map((v) => roundSvg(v)).join(" ")})`;
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
        <div>SVG 文本对象：{{ textCount }}</div>
        <div>背景图片对象：{{ imageCount }}</div>
        <div>旋转角度：{{ page.page.rotation ?? 0 }}°</div>
      </section>

      <section v-if="selectedTextObject" class="editor-panel">
        <h2>文本编辑</h2>
        <div class="object-id">对象 ID：{{ selectedTextObject.id }}</div>
        <div v-if="selectedTextObject.punct_width_squeeze" class="object-id" title="该字体为全角标点定义了压缩字宽（标点宽度替换特性）">
          ⚠ 标点宽度替换
        </div>
        <div v-if="selectedTextObject.font_features?.length" class="object-id" :title="`OpenType 特性：${selectedTextObject.font_features?.join(', ')}`">
          OpenType 特性：{{ selectedTextObject.font_features?.join(", ") }}
        </div>
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
        <p class="helper-text" :class="{ danger: hasEffectiveOverflow }">
          {{ previewStatus }}
        </p>
        <button class="save-button" :disabled="!canSaveEdit" @click="saveTextEdit()">
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
      <div v-if="page && backgroundUrl && currentViewport" class="page-viewport" :style="pageViewportStyle()">
        <div class="page-canvas" :style="pageCanvasStyle()" @pointerdown="onCanvasPointerDown">
          <img
            class="background"
            :style="backgroundStyle()"
            :src="backgroundUrl"
            alt="PDF background render"
          />

          <svg
            class="page-svg"
            :viewBox="`0 0 ${currentViewport.width} ${currentViewport.height}`"
            aria-label="PDF svg text render"
          >
            <image
              v-for="image in renderImageObjects"
              :key="`image-${image.id}`"
              :href="image.objectUrl"
              width="1"
              height="1"
              preserveAspectRatio="none"
              :transform="svgImageTransform(image)"
            />
            <template v-for="text in renderTextObjects" :key="`text-${text.id}`">
              <!--
                Per-glyph absolute rendering (scatter-format merged groups and any object
                where glyph positions are available).  Each character is placed at its exact
                PDF-space (x, y) coordinate so it aligns with the background bitmap regardless
                of inter-character spacing variations in the original PDF.
                Clicking any glyph triggers editing for the whole logical group via text.id,
                which maps to the primary member of the group — the backend distributes the
                new text across all group members via repartition_group_text.
              -->
              <g
                v-if="glyphsForSvg(text)"
                :data-object-id="text.id"
              >
                <text
                  v-for="(glyph, glyphIndex) in (glyphsForSvg(text) ?? [])"
                  :key="`glyph-${text.id}-${glyphIndex}`"
                  :transform="svgGlyphTransform(glyph, text)"
                  :font-family="svgFontFamily(glyph.font_name ?? text.font_name)"
                  :font-weight="fontWeightFor(glyph.font_name ?? text.font_name)"
                  :fill="svgFill(text)"
                  :stroke="svgStroke(text)"
                  :stroke-width="svgStrokeWidth(text)"
                  :paint-order="svgPaintOrder(text)"
                  :style="svgFontFeatureSettings(text) ? { fontFeatureSettings: svgFontFeatureSettings(text) } : undefined"
                  font-size="1"
                  dominant-baseline="alphabetic"
                  @pointerdown.stop
                  @click.stop="beginTextEdit(text.id)"
                >{{ glyph.ch }}</text>
              </g>
              <!--
                Legacy textLength rendering for text objects without glyph position data.
                Uses the group transform + lengthAdjust to distribute characters across the
                object's width.
              -->
              <text
                v-else
                :transform="svgTextTransform(text)"
                :font-family="svgFontFamily(text.font_name)"
                :font-weight="fontWeightFor(text.font_name)"
                :fill="svgFill(text)"
                :stroke="svgStroke(text)"
                :stroke-width="svgStrokeWidth(text)"
                :paint-order="svgPaintOrder(text)"
                :style="svgFontFeatureSettings(text) ? { fontFeatureSettings: svgFontFeatureSettings(text) } : undefined"
                font-size="1"
                xml:space="preserve"
                dominant-baseline="alphabetic"
                :textLength="svgTextLength(text)"
                :lengthAdjust="svgTextLength(text) != null ? 'spacingAndGlyphs' : undefined"
                @pointerdown.stop
                @click.stop="beginTextEdit(text.id)"
              >
                <template v-if="svgTextRuns(text)">
                  <tspan
                    v-for="(run, runIndex) in svgTextRuns(text) ?? []"
                    :key="`run-${text.id}-${runIndex}`"
                    :font-family="svgFontFamily(run.font_name)"
                    :font-weight="fontWeightFor(run.font_name)"
                    :fill="colorToCss(run.color)"
                  >
                    {{ run.content }}
                  </tspan>
                </template>
                <template v-else-if="svgTextLines(text).length > 1">
                  <tspan
                    v-for="(line, lineIndex) in svgTextLines(text)"
                    :key="`line-${text.id}-${lineIndex}`"
                    x="0"
                    :y="lineIndex === 0 ? 0 : lineIndex * 1.2"
                  >
                    {{ line }}
                  </tspan>
                </template>
                <template v-else>{{ text.content }}</template>
              </text>
            </template>
          </svg>

          <svg
            v-if="selectedViewportRect"
            class="layout-preview"
            :class="{ overflow: hasEffectiveOverflow }"
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

          <div
            v-if="editSession && selectedTextObject"
            class="inline-editor-wrap"
            :style="inlineEditorWrapStyle"
          >
            <div
              class="editor-edge editor-edge-left"
              title="拖拽调整编辑框宽度"
              @pointerdown.prevent.stop="onEdgeDragStart($event, 'left')"
            />
            <textarea
              ref="inlineEditor"
              v-model="draftText"
              class="inline-text-editor"
              :class="{ overflow: hasEffectiveOverflow }"
              :style="inlineEditorStyle"
              :disabled="isPreparingEdit || isSavingEdit"
              spellcheck="false"
              @input="onDraftInput"
              @keydown="onInlineEditorKeydown"
              @blur="saveTextEditOnBlur"
              @pointerdown.stop
              @click.stop
            />
            <div
              class="editor-edge editor-edge-right"
              title="拖拽调整编辑框宽度"
              @pointerdown.prevent.stop="onEdgeDragStart($event, 'right')"
            />
          </div>
        </div>
      </div>
      <div v-else class="empty-state">加载 PDF 后显示 PNG 背景与 SVG 文本层，并可直接点击文本进行编辑</div>
    </section>
  </main>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch, watchEffect } from "vue";
import type { LayoutGlyph, LocalSystemFontOption, RichTextRun, StructuredImageObject, StructuredTextObject, TextTypography } from "./pdfEditor";
import {
  describePdfFontUsage,
  getPdfBytesByHandle,
  getPageStructureByHandle,
  previewTextLayout,
  resolvePdfFontFamily,
  setCjkFontByHandle,
  setLocalFontByHandle,
  startTextEdit,
  updateTextByHandle,
  updateTextRunsByHandle
} from "./pdfEditor";
import { pdfRectToViewportRect, pdfToViewport, viewportToPdf, type Matrix2D } from "./viewport";
import { usePdfDocument } from "./composables/usePdfDocument";
import { usePdfEditor } from "./composables/usePdfEditor";
import RunsEditor from "./components/RunsEditor.vue";

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
  draftRuns,
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

interface LocalFontData {
  family: string;
  fullName: string;
  postscriptName: string;
  style: string;
  blob(): Promise<Blob>;
}

type QueryLocalFonts = (options?: { postscriptNames?: string[] }) => Promise<LocalFontData[]>;

const systemFontOptions = ref<LocalSystemFontOption[]>([]);
const systemFontData = new Map<string, LocalFontData>();
const systemFontBytes = new Map<string, Uint8Array>();

// ── Built-in / system fonts available in the browser ──────────────────────
// These are loaded via @fontsource packages or known browser fonts.
// resource_name uses a "__builtin__:" prefix to distinguish them from PDF-embedded fonts.
// On save they map to null (backend uses the inherited/fallback PDF font).
const BUILTIN_FONTS = [
  { resource_name: "__builtin__:Noto Sans SC", family_name: "Noto Sans SC", css_family: '"Noto Sans SC","Noto Sans CJK SC",sans-serif' },
  { resource_name: "__builtin__:serif",        family_name: "衬线体 (Serif)",  css_family: 'Georgia,"Times New Roman",serif' },
  { resource_name: "__builtin__:sans-serif",   family_name: "无衬线 (Sans)",   css_family: 'Arial,"Helvetica Neue",sans-serif' },
  { resource_name: "__builtin__:monospace",    family_name: "等宽 (Mono)",     css_family: '"Courier New",Courier,monospace' },
];

/** CSS font-family string for a given resource_name (PDF-embedded or built-in). */
function fontFamilyForResource(resourceName: string | null): string {
  if (!resourceName) return svgFontFamily(null);
  const builtin = BUILTIN_FONTS.find((f) => f.resource_name === resourceName);
  if (builtin) return builtin.css_family;
  const system = systemFontOptions.value.find((f) => f.resource_name === resourceName);
  if (system) return system.css_family;
  return svgFontFamily(resourceName);
}

/** All font options shown in pickers: built-in first, then PDF-embedded. */
const allFontOptions = computed(() => [
  ...BUILTIN_FONTS,
  ...systemFontOptions.value,
  ...fontAssets.value
]);

/** ref to the contenteditable inline rich-text editor div */
const inlineEditor = ref<HTMLDivElement | null>(null);
/** True while we are programmatically updating the contenteditable DOM to avoid a feedback loop */
let skipNextDomSync = false;
/** ID of the run span currently under the cursor (used to set toolbar values) */
const activeRunId = ref<string | null>(null);
const draftTypography = ref<TextTypography>(defaultTextTypography());
/**
 * True while a toolbar control (font select / size input) is receiving interaction.
 * Prevents onInlineEditorBlur from treating the transient focus-loss as a real blur.
 */
const isToolbarInteracting = ref(false);
const zoomPercent = computed(() => `${Math.round(zoom.value * 100)}%`);
let editorPanelInteractionTimer: number | null = null;

function defaultTextTypography(): TextTypography {
  return {
    replace_spaces_with_displacements: false,
    digit_font_name: null,
    compress_punctuation: false,
    detected_tj_displacements: false,
    detected_space_displacements: false,
    detected_punctuation: false,
    detected_digit_font_name: null
  };
}

function normalizeDraftTypography(value?: Partial<TextTypography> | null): TextTypography {
  return {
    ...defaultTextTypography(),
    ...value,
    digit_font_name: value?.digit_font_name ?? null,
    detected_digit_font_name: value?.detected_digit_font_name ?? null
  };
}

// ── Edit-box resize state ──────────────────────────────────────────────────
/** Width override in viewport pixels set by dragging the right edge. */
const editBoxWidthOverride = ref<number | null>(null);
/** Local x-axis offset override in viewport pixels set by dragging the left edge. */
const editBoxLeftOverride = ref<number | null>(null);
/** Local x-axis offset in viewport pixels set by dragging the top/bottom edge. */
const editBoxMoveOffset = ref<number | null>(null);
/** True while edit-box handles may transiently steal focus from the editor. */
const isEdgeDragInteracting = ref(false);

interface EdgeDragState {
  edge: "left" | "right";
  startX: number;
  startY: number;
  axisX: { x: number; y: number };
  initialLeft: number;
  initialWidth: number;
}
let edgeDragState: EdgeDragState | null = null;

interface MoveDragState {
  startX: number;
  startY: number;
  axisX: { x: number; y: number };
  initialOffset: number;
}
let moveDragState: MoveDragState | null = null;
const renderImageObjects = computed(() => page.value?.images.filter((image) => image.objectUrl) ?? []);

const selectedTextObject = computed<StructuredTextObject | null>(
  () => page.value?.text.find((item) => item.id === selectedTextId.value) ?? null
);
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

  const validGlyphs = session.glyphs.filter((g) => g.advance > 0);
  if (!validGlyphs.length) {
    editorFontScaleX.value = 1.0;
    return;
  }
  const avgPdfAdvance = validGlyphs.reduce((s, g) => s + g.advance, 0) / validGlyphs.length;

  const effFs = effectiveFontSize(text);
  const targetPx = avgPdfAdvance * effFs * viewport.zoom;

  const cssFontSizePx = Math.max(10, effFs * viewport.zoom);
  const cssFont = `${cssFontSizePx}px ${fontUsage.cssFontFamily}`;
  const measuredPx = measureAverageCharWidth(cssFont, validGlyphs.map((g) => g.ch));

  if (measuredPx < 0.5) {
    editorFontScaleX.value = 1.0;
    return;
  }

  editorFontScaleX.value = Math.max(0.5, Math.min(2.0, targetPx / measuredPx));
});

// Keep draftText (used for preview) in sync with draftRuns content.
watch(draftRuns, (runs) => {
  draftText.value = runs.map((r) => r.content).join("");
  // Sync runs → contenteditable DOM (only when the change comes from outside, e.g. beginTextEdit).
  if (skipNextDomSync) { skipNextDomSync = false; return; }
  writeRunsToEditor(runs);
}, { deep: true });

/** Write the current draftRuns into the contenteditable div as styled spans. */
function writeRunsToEditor(runs: RichTextRun[]) {
  const el = inlineEditor.value;
  if (!el) return;
  const selection = window.getSelection();
  // Save caret position relative to the focused run (best-effort).
  const focusedRunId = getActiveRunId();
  const caretOffset = selection?.focusOffset ?? 0;

  el.innerHTML = "";
  for (const run of runs) {
    const span = document.createElement("span");
    span.setAttribute("data-run-id", run.id);
    applyRunStyleToSpan(span, run);
    span.textContent = run.content;
    el.appendChild(span);
  }

  // Restore caret into the same run if possible.
  if (focusedRunId && document.activeElement === el) {
    const target = el.querySelector<HTMLElement>(`[data-run-id="${focusedRunId}"]`);
    if (target?.firstChild) {
      try {
        const range = document.createRange();
        range.setStart(target.firstChild, Math.min(caretOffset, target.textContent?.length ?? 0));
        range.collapse(true);
        selection?.removeAllRanges();
        selection?.addRange(range);
      } catch {
        // ignore cursor restore errors
      }
    }
  }
}

/** Apply CSS styling to a span element from a RichTextRun. */
function applyRunStyleToSpan(span: HTMLElement, run: RichTextRun) {
  const viewport = currentViewport.value;
  const text = selectedTextObject.value;
  const session = editSession.value;
  if (!viewport || !text) return;

  const fontName = run.font_name ?? session?.font_id ?? text.font_name;
  const fontSize = run.font_size ?? effectiveFontSize(text);
  const color = run.color ?? text.color;

  span.style.fontFamily = fontFamilyForResource(fontName);
  span.style.fontSize = `${Math.max(8, fontSize * viewport.zoom)}px`;
  span.style.color = colorToCss(color);
  span.style.fontWeight = fontWeightFor(fontName) ?? "";
}

/** Read runs out of the contenteditable DOM. */
function readRunsFromEditor(): RichTextRun[] {
  const el = inlineEditor.value;
  if (!el) return draftRuns.value;
  const spans = Array.from(el.querySelectorAll<HTMLElement>("[data-run-id]"));
  if (!spans.length) {
    // Fallback: plain text with no spans — put it all in the first run.
    const first = draftRuns.value[0];
    if (first) return [{ ...first, content: el.textContent ?? "" }];
    return draftRuns.value;
  }
  return spans.map((span) => {
    const runId = span.getAttribute("data-run-id")!;
    const existing = draftRuns.value.find((r) => r.id === runId);
    return {
      id: runId,
      content: span.textContent ?? "",
      font_name: existing?.font_name ?? null,
      font_size: existing?.font_size ?? null,
      color: existing?.color ?? null
    };
  });
}

/** Get the data-run-id of the span currently containing the caret. */
function getActiveRunId(): string | null {
  const selection = window.getSelection();
  if (!selection?.rangeCount) return null;
  let node: Node | null = selection.getRangeAt(0).startContainer;
  while (node && node !== inlineEditor.value) {
    if (node instanceof Element) {
      const id = node.getAttribute("data-run-id");
      if (id) return id;
    }
    node = node.parentNode;
  }
  return null;
}

// ── Selection-aware style application ────────────────────────────────────────

/** Saved selection range; captured before toolbar interactions steal focus. */
let savedRange: Range | null = null;

/** Save the current editor selection so toolbar changes can use it. */
function saveEditorSelection() {
  const sel = window.getSelection();
  const el = inlineEditor.value;
  if (!el || !sel?.rangeCount) { savedRange = null; return; }
  const r = sel.getRangeAt(0);
  savedRange = el.contains(r.commonAncestorContainer) ? r.cloneRange() : null;
}

/**
 * Find which run and character offset within that run a Range endpoint maps to.
 * `spans` must be the ordered list of [data-run-id] elements from the editor.
 */
function getRangeEndpoint(
  spans: HTMLElement[],
  container: Node,
  offset: number
): { runIndex: number; charOffset: number } | null {
  for (let i = 0; i < spans.length; i++) {
    const span = spans[i];
    if (span === container) {
      // container is the span element itself; offset = child index
      return { runIndex: i, charOffset: offset === 0 ? 0 : (span.textContent?.length ?? 0) };
    }
    if (span.contains(container)) {
      // container is a text node inside this span
      return { runIndex: i, charOffset: offset };
    }
  }
  // container is the editor root; offset = span child index
  if (container === inlineEditor.value) {
    const clampedIdx = Math.min(offset, spans.length - 1);
    return { runIndex: Math.max(0, clampedIdx), charOffset: 0 };
  }
  return null;
}

/** Merge adjacent runs that have identical styling (font_name, font_size, color). */
function mergeAdjacentRuns(runs: RichTextRun[]): RichTextRun[] {
  const result: RichTextRun[] = [];
  for (const run of runs) {
    const last = result[result.length - 1];
    if (
      last &&
      last.font_name === run.font_name &&
      last.font_size === run.font_size &&
      JSON.stringify(last.color) === JSON.stringify(run.color)
    ) {
      last.content += run.content;
    } else {
      result.push({ ...run });
    }
  }
  return result;
}

/**
 * Apply a style patch to the text covered by `range` in `runs`.
 * Runs are split at the selection boundaries; only the selected portion receives the new style.
 * Adjacent runs with identical style are merged.
 */
function applyStyleToRangeInRuns(
  runs: RichTextRun[],
  spans: HTMLElement[],
  range: Range,
  style: { font_name?: string | null; font_size?: number | null }
): RichTextRun[] {
  const sp = getRangeEndpoint(spans, range.startContainer, range.startOffset);
  const ep = getRangeEndpoint(spans, range.endContainer, range.endOffset);
  if (!sp || !ep) return runs;

  // Normalise so start ≤ end
  let [sRun, sChar, eRun, eChar] =
    sp.runIndex < ep.runIndex || (sp.runIndex === ep.runIndex && sp.charOffset <= ep.charOffset)
      ? [sp.runIndex, sp.charOffset, ep.runIndex, ep.charOffset]
      : [ep.runIndex, ep.charOffset, sp.runIndex, sp.charOffset];

  const newId = () => Math.random().toString(36).slice(2);
  const patched = (run: RichTextRun): RichTextRun => ({
    ...run,
    id: newId(),
    font_name: ("font_name" in style ? style.font_name : run.font_name) as string | null,
    font_size: ("font_size" in style ? style.font_size : run.font_size) as number | null
  });

  const newRuns: RichTextRun[] = [];
  for (let i = 0; i < runs.length; i++) {
    const run = runs[i];
    const chars = Array.from(run.content);

    if (i < sRun || i > eRun) {
      newRuns.push(run);
      continue;
    }

    const selStart = i === sRun ? sChar : 0;
    const selEnd   = i === eRun ? eChar : chars.length;

    const before   = chars.slice(0, selStart).join("");
    const selected = chars.slice(selStart, selEnd).join("");
    const after    = chars.slice(selEnd).join("");

    if (before)   newRuns.push({ ...run, id: newId(), content: before });
    if (selected) newRuns.push({ ...patched(run), content: selected });
    if (after)    newRuns.push({ ...run, id: newId(), content: after });
  }

  return mergeAdjacentRuns(newRuns);
}

/**
 * Apply a style change to the current text selection (or to the active run if
 * nothing is selected).  Called by font-picker and size-input toolbar events.
 */
function applyStyleToSelection(style: { font_name?: string | null; font_size?: number | null }) {
  const el = inlineEditor.value;
  if (!el) return;

  const range = savedRange;

  if (!range || range.collapsed) {
    // No text selected → apply to the run under the cursor
    const run = activeRun.value;
    if (!run) return;
    skipNextDomSync = true;
    draftRuns.value = draftRuns.value.map((r) =>
      r.id === run.id
        ? {
            ...r,
            font_name: ("font_name" in style ? style.font_name : r.font_name) as string | null,
            font_size: ("font_size" in style ? style.font_size : r.font_size) as number | null
          }
        : r
    );
    void nextTick(() => {
      const spanEl = el.querySelector<HTMLElement>(`[data-run-id="${run.id}"]`);
      if (spanEl) applyRunStyleToSpan(spanEl, draftRuns.value.find((r) => r.id === run.id)!);
    });
    return;
  }

  // Text is selected → split and re-style the selected portion
  const spans = Array.from(el.querySelectorAll<HTMLElement>("[data-run-id]"));
  const newRuns = applyStyleToRangeInRuns(draftRuns.value, spans, range, style);
  skipNextDomSync = false;
  draftRuns.value = newRuns;
  void nextTick(() => writeRunsToEditor(newRuns));
}

const renderTextObjects = computed(() => {
  const pageText = page.value?.text ?? [];
  if (!layoutPreview.value || !selectedTextObject.value || !editSession.value) {
    return pageText;
  }

  // Compute the effective font name for preview rendering.
  // If the user has changed fonts on any run, reflect that change immediately
  // so characters that were boxes with the original font become visible with the
  // new font.  Falls back to the original session font_id when no override exists.
  const effectiveFontForRun = (runFontName: string | null) =>
    runFontName ?? editSession.value!.font_id;
  const primaryPreviewFont =
    effectiveFontForRun(draftRuns.value[0]?.font_name ?? null);
  const originDelta = editorOriginDeltaPdf();

  // Patch glyph font names so the per-glyph SVG path uses the user-selected font.
  const previewGlyphs = (layoutPreview.value.glyphs ?? []).map((g) => {
    // Find which run this glyph belongs to by matching character content.
    // In practice most edits are single-run so all glyphs get the same override.
    const matchingRun = draftRuns.value.find((r) => r.content.includes(g.ch));
    const overrideFont = effectiveFontForRun(matchingRun?.font_name ?? null);
    return {
      ...(g.font_name !== overrideFont ? { ...g, font_name: overrideFont } : g),
      x: g.x + originDelta.x,
      y: g.y + originDelta.y,
      bbox: translatedByOriginDelta(g.bbox, originDelta)!
    };
  });

  // Build per-run data for the runs-based SVG rendering path.
  const previewRuns = draftRuns.value.map((r) => ({
    content: r.content,
    font_name: effectiveFontForRun(r.font_name ?? null),
    font_size: r.font_size ?? editSession.value!.font_size,
    color: r.color ?? selectedTextObject.value!.color,
  }));

  const previewTransform = [...editSession.value.matrix] as Matrix2D;
  previewTransform[4] += originDelta.x;
  previewTransform[5] += originDelta.y;
  const previewClipBounds = editorClipBoundsPdf();
  const previewObject: StructuredTextObject = {
    ...selectedTextObject.value,
    bounds: translatedByOriginDelta(layoutPreview.value.bbox, originDelta)!,
    content: layoutPreview.value.text,
    font_name: primaryPreviewFont,
    font_size: editSession.value.font_size,
    transform: previewTransform,
    glyphs: previewGlyphs,
    clip_bounds: previewClipBounds ?? undefined,
    runs: previewRuns,
  };
  return pageText.map((text) => (text.id === selectedTextObject.value!.id ? previewObject : text));
});

// ── Per-render data pre-computed once per text object ────────────────────────
// Avoids calling glyphsForSvg / svgFontFeatureSettings / svgTextLength multiple
// times inside the hot template loop.
interface TextRenderItem {
  text: StructuredTextObject;
  glyphs: LayoutGlyph[] | null;
  hasSvgPaths: boolean;
  fontFeatureStyle: Record<string, string> | undefined;
  textLength: number | undefined;
}

const textRenderItems = computed((): TextRenderItem[] =>
  renderTextObjects.value.map((text) => {
    const glyphs = glyphsForSvg(text);
    const featureStr = svgFontFeatureSettings(text);
    return {
      text,
      glyphs,
      hasSvgPaths: glyphs?.some((g) => glyphHasSvgPath(g)) ?? false,
      fontFeatureStyle: featureStr ? { fontFeatureSettings: featureStr } : undefined,
      textLength: svgTextLength(text)
    };
  })
);

// SVG <g> transform that maps PDF page coordinates to viewport pixels.
// All glyph/text/image elements live inside this group so their own transforms
// only encode the PDF-local part and do not change on zoom/pan.
const svgViewportTransform = computed((): string => {
  const m = currentViewport.value?.transform;
  if (!m) return "";
  return `matrix(${m.map(roundSvg).join(" ")})`;
});

const selectedViewportRect = computed(() => {
  const viewport = currentViewport.value;
  const targetBounds = editSession.value ? editorClipBoundsPdf() : selectedTextObject.value?.clip_bounds ?? selectedTextObject.value?.bounds;
  if (!viewport || !targetBounds) return null;
  return pdfRectToViewportRect(viewport, targetBounds);
});

const previewGlyphRects = computed(() => {
  const viewport = currentViewport.value;
  const glyphs = layoutPreview.value?.glyphs ?? [];
  if (!viewport) return [];
  const originDelta = editorOriginDeltaPdf();
  const clipBounds = editorClipBoundsPdf();
  return glyphs.flatMap((glyph, index) => {
    const bbox = translatedByOriginDelta(glyph.bbox, originDelta)!;
    const clipped = clipBounds ? intersectRects(bbox, clipBounds) : bbox;
    if (!clipped) return [];
    return [{
      id: `${index}-${glyph.ch}-${glyph.x}-${glyph.y}`,
      rect: pdfRectToViewportRect(viewport, clipped)
    }];
  });
});

interface InlineEditorGeometry {
  fontSize: number;
  naturalWidth: number;
  origin: { x: number; y: number };
  axisX: { x: number; y: number };
  axisY: { x: number; y: number };
}

function vectorLength(vector: { x: number; y: number }) {
  return Math.hypot(vector.x, vector.y);
}

function normalizeVector(vector: { x: number; y: number }, fallback: { x: number; y: number }) {
  const length = vectorLength(vector);
  if (length < 0.0001) return fallback;
  return { x: vector.x / length, y: vector.y / length };
}

function trailingSpacePunctuationCount(text: string) {
  return Array.from(text).filter((ch) => "，。、：；！？）》」』》…—,.:;!?".includes(ch)).length;
}

function inlineEditorGeometry(text: StructuredTextObject): InlineEditorGeometry | null {
  const viewport = currentViewport.value;
  const session = editSession.value;
  if (!viewport || !session) return null;

  const fontSize = Math.max(10, effectiveFontSize(text) * viewport.zoom);
  const matrix = session.matrix;
  const firstGlyph = text.glyphs?.[0] ?? null;
  const baseline = firstGlyph
    ? pdfToViewport(viewport, firstGlyph.x, firstGlyph.y)
    : pdfToViewport(viewport, matrix[4], matrix[5]);

  const xEnd = pdfToViewport(viewport, matrix[4] + matrix[0], matrix[5] + matrix[1]);
  const downEnd = pdfToViewport(viewport, matrix[4] - matrix[2], matrix[5] - matrix[3]);
  const matrixOrigin = pdfToViewport(viewport, matrix[4], matrix[5]);
  const axisX = normalizeVector({ x: xEnd.x - matrixOrigin.x, y: xEnd.y - matrixOrigin.y }, { x: 1, y: 0 });
  const axisY = normalizeVector({ x: downEnd.x - matrixOrigin.x, y: downEnd.y - matrixOrigin.y }, { x: 0, y: 1 });
  const origin = {
    x: baseline.x - axisY.x * fontSize,
    y: baseline.y - axisY.y * fontSize
  };

  const effFs = effectiveFontSize(text);
  const sessionGlyphs = session.glyphs;
  const totalAdvance = sessionGlyphs.length
    ? sessionGlyphs.reduce((s, g) => s + Math.max(g.advance, 0), 0)
    : 0;
  const compressionPadding = (draftTypography.value.compress_punctuation || draftTypography.value.detected_punctuation)
    ? trailingSpacePunctuationCount(draftText.value || session.original_text) * effFs * viewport.zoom * 0.5
    : 0;
  const visibleBoundsWidth = pdfRectToViewportRect(viewport, text.clip_bounds ?? text.bounds).width;
  const naturalWidth = totalAdvance > 0
    ? Math.max(totalAdvance * effFs * viewport.zoom, fontSize * 4)
    : fontSize * 8;

  return {
    fontSize,
    naturalWidth: Math.max(naturalWidth + compressionPadding, visibleBoundsWidth),
    origin,
    axisX,
    axisY
  };
}

function editorStartOffsetPx() {
  return (editBoxLeftOverride.value ?? 0) + (editBoxMoveOffset.value ?? 0);
}

/** Layout styles (position, size) for the editor wrapper div. */
const inlineEditorWrapStyle = computed(() => {
  const text = selectedTextObject.value;
  if (!text) return {};
  const geometry = inlineEditorGeometry(text);
  if (!geometry) return {};
  const startOffset = editorStartOffsetPx();
  const left = geometry.origin.x + geometry.axisX.x * startOffset;
  const top = geometry.origin.y + geometry.axisX.y * startOffset;

  return {
    left: "0px",
    top: "0px",
    width: `${editBoxWidthOverride.value ?? geometry.naturalWidth}px`,
    height: `${geometry.fontSize * 1.4}px`,
    transform: `matrix(${[
      geometry.axisX.x,
      geometry.axisX.y,
      geometry.axisY.x,
      geometry.axisY.y,
      left,
      top
    ].map((value) => roundSvg(value)).join(", ")})`,
    transformOrigin: "0 0"
  };
});

/** Container style for the contenteditable rich-text editor div. */
const inlineEditorStyle = computed(() => {
  const text = selectedTextObject.value;
  const viewport = currentViewport.value;
  if (!text || !viewport || !editSession.value) return {};
  const fontSize = Math.max(10, effectiveFontSize(text) * viewport.zoom);
  return {
    lineHeight: "1.2",
    height: `${fontSize * 1.4}px`
  };
});

/** Style for the font/size toolbar that floats above the inline editor. */
const toolbarStyle = computed(() => {
  const wrapStyle = inlineEditorWrapStyle.value;
  if (!wrapStyle.transform) return {};
  return wrapStyle;
});

/** The active run object for the toolbar controls. */
const activeRun = computed(() => {
  if (!activeRunId.value) return draftRuns.value[0] ?? null;
  return draftRuns.value.find((r) => r.id === activeRunId.value) ?? draftRuns.value[0] ?? null;
});

const hasRunStyleChanges = computed(() => {
  // Any run with an explicit (non-null) font_name, font_size, or color means a style was applied.
  // Using this simple check avoids false positives from comparing against text.runs which is
  // often empty for basic PDF text objects.
  return draftRuns.value.some((r) => r.font_name !== null || r.font_size !== null || r.color !== null);
});

const hasEditorPositionChange = computed(() => Math.abs(editorStartOffsetPx()) > 0.5);

const hasEditorClipChange = computed(() => {
  const text = selectedTextObject.value;
  if (!text) return false;
  const geometry = inlineEditorGeometry(text);
  if (!geometry) return false;
  const width = editBoxWidthOverride.value ?? geometry.naturalWidth;
  return Math.abs(width - geometry.naturalWidth) > 0.5;
});

const hasTypographyChanges = computed(() => {
  const base = normalizeDraftTypography(editSession.value?.typography ?? selectedTextObject.value?.typography);
  return JSON.stringify(draftTypography.value) !== JSON.stringify(base);
});

const canSaveEdit = computed(() => {
  if (!selectedTextObject.value || !editSession.value || isSavingEdit.value || isPreparingEdit.value) {
    return false;
  }
  // Allow save if text, style, or the edit-box start position changed.
  return draftText.value !== editSession.value.original_text
    || hasRunStyleChanges.value
    || hasEditorPositionChange.value
    || hasEditorClipChange.value
    || hasTypographyChanges.value;
});

const draftUsesBuiltInFont = computed(() =>
  draftRuns.value.some((r) => r.font_name?.startsWith("__builtin__:"))
  || Boolean(draftTypography.value.digit_font_name?.startsWith("__builtin__:"))
);

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
  if (editorPanelInteractionTimer != null) window.clearTimeout(editorPanelInteractionTimer);
  stopEdgeDrag();
  stopMoveDrag();
});

function resetEditBoxOverrides() {
  editBoxWidthOverride.value = null;
  editBoxLeftOverride.value = null;
  editBoxMoveOffset.value = null;
}

function stopEdgeDrag() {
  if (!edgeDragState) return;
  edgeDragState = null;
  window.removeEventListener("pointermove", onEdgeDragMove);
  window.removeEventListener("pointerup", onEdgeDragEnd);
}

function stopMoveDrag() {
  if (!moveDragState) return;
  moveDragState = null;
  window.removeEventListener("pointermove", onMoveDragMove);
  window.removeEventListener("pointerup", onMoveDragEnd);
}

function onEdgeDragStart(event: PointerEvent, edge: "left" | "right") {
  const text = selectedTextObject.value;
  if (!text) return;
  const geometry = inlineEditorGeometry(text);
  if (!geometry) return;
  saveEditorSelection();
  isEdgeDragInteracting.value = true;
  const initialLeft = editBoxLeftOverride.value ?? 0;
  const initialWidth = editBoxWidthOverride.value ?? geometry.naturalWidth;
  edgeDragState = { edge, startX: event.clientX, startY: event.clientY, axisX: geometry.axisX, initialLeft, initialWidth };
  window.addEventListener("pointermove", onEdgeDragMove);
  window.addEventListener("pointerup", onEdgeDragEnd);
  event.preventDefault();
}

function onEdgeDragMove(event: PointerEvent) {
  if (!edgeDragState) return;
  const dx = event.clientX - edgeDragState.startX;
  const dy = event.clientY - edgeDragState.startY;
  const delta = dx * edgeDragState.axisX.x + dy * edgeDragState.axisX.y;
  if (edgeDragState.edge === "right") {
    editBoxWidthOverride.value = Math.max(40, edgeDragState.initialWidth + delta);
  } else {
    const newWidth = Math.max(40, edgeDragState.initialWidth - delta);
    editBoxWidthOverride.value = newWidth;
    editBoxLeftOverride.value = edgeDragState.initialLeft + (edgeDragState.initialWidth - newWidth);
  }
}

function onEdgeDragEnd() {
  stopEdgeDrag();
  // Restore focus to the inline editor after releasing the resize handle.
  void nextTick(() => {
    window.setTimeout(() => {
      inlineEditor.value?.focus({ preventScroll: true });
      isEdgeDragInteracting.value = false;
    }, 0);
  });
}

function onMoveDragStart(event: PointerEvent) {
  const text = selectedTextObject.value;
  if (!text) return;
  const geometry = inlineEditorGeometry(text);
  if (!geometry) return;
  saveEditorSelection();
  isEdgeDragInteracting.value = true;
  moveDragState = {
    startX: event.clientX,
    startY: event.clientY,
    axisX: geometry.axisX,
    initialOffset: editBoxMoveOffset.value ?? 0
  };
  window.addEventListener("pointermove", onMoveDragMove);
  window.addEventListener("pointerup", onMoveDragEnd);
  event.preventDefault();
}

function onMoveDragMove(event: PointerEvent) {
  if (!moveDragState) return;
  const dx = event.clientX - moveDragState.startX;
  const dy = event.clientY - moveDragState.startY;
  const delta = dx * moveDragState.axisX.x + dy * moveDragState.axisX.y;
  editBoxMoveOffset.value = moveDragState.initialOffset + delta;
}

function onMoveDragEnd() {
  stopMoveDrag();
  void nextTick(() => {
    window.setTimeout(() => {
      inlineEditor.value?.focus({ preventScroll: true });
      isEdgeDragInteracting.value = false;
    }, 0);
  });
}

async function onFileChange(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;

  clearEditingState();
  await openFile(file);
  // Pre-embed NotoSans SC — await so the font is ready before the first edit/save.
  if (pdfHandle.value != null) {
    const embedded = await setCjkFontByHandle(pdfHandle.value);
    if (!embedded) {
      status.value = "已打开 PDF，但用于保存内置字体的嵌入字体加载失败";
    }
  }
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

async function loadSystemFonts() {
  const queryLocalFonts = (window as Window & { queryLocalFonts?: QueryLocalFonts }).queryLocalFonts;
  if (!queryLocalFonts) {
    status.value = "当前浏览器不支持读取系统字体；请使用桌面版 Chrome 或 Edge。";
    return;
  }

  try {
    status.value = "正在请求系统字体权限...";
    const fonts = await queryLocalFonts();
    systemFontData.clear();
    systemFontBytes.clear();
    const seen = new Set<string>();
    const options: LocalSystemFontOption[] = [];
    for (const font of fonts) {
      const key = sanitizeLocalFontKey(font.postscriptName || font.fullName || font.family);
      if (!key || seen.has(key)) continue;
      seen.add(key);
      const resourceName = `__localfont__:${key}`;
      systemFontData.set(resourceName, font);
      options.push({
        resource_name: resourceName,
        family_name: `${font.family}${font.style && font.style !== "Regular" ? ` ${font.style}` : ""}`,
        css_family: `"${cssEscapeFontFamily(font.fullName)}", "${cssEscapeFontFamily(font.family)}", sans-serif`,
        full_name: font.fullName,
        postscript_name: font.postscriptName
      });
    }
    options.sort((left, right) => left.family_name.localeCompare(right.family_name));
    systemFontOptions.value = options;
    status.value = `已加载 ${options.length} 个系统字体，可在字体下拉框中选择。`;
  } catch (error) {
    console.error(error);
    status.value = error instanceof Error ? `读取系统字体失败：${error.message}` : "读取系统字体失败";
  }
}

function sanitizeLocalFontKey(value: string) {
  return value.replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 80);
}

function cssEscapeFontFamily(value: string) {
  return value.replace(/["\\]/g, "\\$&");
}

async function ensureSystemFontsForRuns(handle: number, runs: RichTextRun[], typography?: TextTypography | null) {
  const names = Array.from(new Set(
    [
      ...runs.map((run) => run.font_name),
      typography?.digit_font_name ?? null
    ]
      .filter((name): name is string => Boolean(name?.startsWith("__localfont__:")))
  ));
  for (const name of names) {
    let bytes = systemFontBytes.get(name);
    if (!bytes) {
      const font = systemFontData.get(name);
      if (!font) {
        throw new Error("保存失败：所选系统字体未加载，请重新点击“加载系统字体”。");
      }
      const blob = await font.blob();
      const buffer = await blob.arrayBuffer();
      bytes = new Uint8Array(buffer);
      systemFontBytes.set(name, bytes);
    }
    const accepted = await setLocalFontByHandle(handle, name, bytes);
    if (!accepted) {
      const label = systemFontOptions.value.find((font) => font.resource_name === name)?.family_name ?? name;
      throw new Error(`保存失败：系统字体“${label}”不是当前版本可嵌入的 TrueType/TTC 字体。`);
    }
  }
}

async function beginTextEdit(objectId: number) {
  if (pdfHandle.value == null) return;
  resetEditBoxOverrides();
  clearPreviewTimer();
  selectedTextId.value = objectId;
  isPreparingEdit.value = true;
  layoutPreview.value = null;
  status.value = `正在准备文本对象 ${objectId} 的编辑会话...`;
  const currentSelection = incrementSelection();

  try {
    const session = await startTextEdit(pdfHandle.value, objectId);
    if (currentSelection !== getSelectionToken()) return;
    editSession.value = session;

    // Initialize draftRuns from the text object's existing runs (or a single default run).
    const textObj = page.value?.text.find((t) => t.id === objectId);
    draftTypography.value = normalizeDraftTypography(session.typography ?? textObj?.typography);
    const existingRuns = textObj?.runs?.filter((r) => r.content) ?? [];
    if (existingRuns.length > 0) {
      draftRuns.value = existingRuns.map((r) => ({
        id: Math.random().toString(36).slice(2),
        content: r.content,
        font_name: r.font_name,
        font_size: r.font_size !== session.font_size ? r.font_size : null,
        color: null
      }));
    } else {
      draftRuns.value = [{
        id: Math.random().toString(36).slice(2),
        content: session.original_text,
        font_name: null,
        font_size: null,
        color: null
      }];
    }
    draftText.value = draftRuns.value.map((r) => r.content).join("");

    layoutPreview.value = {
      object_id: objectId,
      text: draftText.value,
      group_object_ids: session.group_object_ids,
      glyphs: session.glyphs,
      bbox: session.bbox,
      overflow: false,
      typography: draftTypography.value
    };
    status.value = `已选中文本对象 ${objectId}，可直接修改并保存`;
    await nextTick();
    writeRunsToEditor(draftRuns.value);
    if (currentSelection !== getSelectionToken()) return;
    isPreparingEdit.value = false;
    await nextTick();
    inlineEditor.value?.focus();
  } catch (error) {
    console.error(error);
    if (currentSelection !== getSelectionToken()) return;
    status.value = error instanceof Error ? error.message : "启动文本编辑失败";
    selectedTextId.value = null;
    editSession.value = null;
    layoutPreview.value = null;
  } finally {
    if (currentSelection === getSelectionToken() && isPreparingEdit.value) {
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

function onInlineEditorInput() {
  if (!selectedTextId.value) return;
  skipNextDomSync = true;
  draftRuns.value = readRunsFromEditor();
  // draftText is updated by the draftRuns watcher.
  activeRunId.value = getActiveRunId();
  setPreviewTimer(() => {
    void refreshPreview(selectedTextId.value!, draftText.value, getSelectionToken());
  }, 120);
}

function onToolbarTypographyChange(patch: Partial<TextTypography>) {
  draftTypography.value = { ...draftTypography.value, ...patch };
  onDraftInput();
  isToolbarInteracting.value = false;
  void nextTick(() => inlineEditor.value?.focus({ preventScroll: true }));
}

function onInlineEditorSelectionChange() {
  if (document.activeElement !== inlineEditor.value) return;
  activeRunId.value = getActiveRunId();
  saveEditorSelection();
}

function onInlineEditorKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    clearSelection();
    return;
  }
  if (event.key === "Enter") {
    event.preventDefault();
    void saveTextEdit({ closeAfterSave: true });
  }
}

function onInlineEditorFocus() {
  activeRunId.value = getActiveRunId();
  document.addEventListener("selectionchange", onInlineEditorSelectionChange);
}

function onInlineEditorBlur() {
  // Ignore transient blur caused by edge-drag handles or toolbar controls.
  if (edgeDragState || isEdgeDragInteracting.value || isToolbarInteracting.value) return;
  document.removeEventListener("selectionchange", onInlineEditorSelectionChange);
  void saveTextEditOnBlur();
}

/** Call on mousedown of any toolbar control to block the blur-save path. */
function onToolbarMousedown() {
  // Capture selection BEFORE the toolbar steals focus (mousedown fires before blur).
  saveEditorSelection();
  isToolbarInteracting.value = true;
}

function onEditorPanelInteraction() {
  saveEditorSelection();
  isToolbarInteracting.value = true;
  if (editorPanelInteractionTimer != null) {
    window.clearTimeout(editorPanelInteractionTimer);
  }
  editorPanelInteractionTimer = window.setTimeout(() => {
    isToolbarInteracting.value = false;
    editorPanelInteractionTimer = null;
  }, 500);
}

function finishEditorPanelInteraction() {
  if (editorPanelInteractionTimer != null) {
    window.clearTimeout(editorPanelInteractionTimer);
    editorPanelInteractionTimer = null;
  }
  window.setTimeout(() => {
    isToolbarInteracting.value = false;
    inlineEditor.value?.focus({ preventScroll: true });
  }, 0);
}

/** Call on blur of a toolbar control; restores focus to the editor. */
function onToolbarControlBlur(event: FocusEvent) {
  // If focus moved to another element inside the same toolbar, stay in toolbar mode.
  const toolbar = (event.currentTarget as Element)?.closest?.(".inline-run-toolbar");
  const next = event.relatedTarget as Element | null;
  if (toolbar && next && toolbar.contains(next)) return;
  isToolbarInteracting.value = false;
  void nextTick(() => inlineEditor.value?.focus());
}

function onToolbarFontChangeEvent(event: Event) {
  const value = (event.target as HTMLSelectElement).value;
  applyStyleToSelection({ font_name: value || null });
  isToolbarInteracting.value = false;
  void nextTick(() => inlineEditor.value?.focus());
}

function onToolbarSizeChangeEvent(event: Event) {
  const raw = parseFloat((event.target as HTMLInputElement).value);
  applyStyleToSelection({ font_size: isNaN(raw) || raw <= 0 ? null : raw });
  isToolbarInteracting.value = false;
  void nextTick(() => inlineEditor.value?.focus());
}

/** @deprecated Use applyStyleToSelection instead. Kept for direct run-level access if needed. */
function setActiveRunSize(size: number | null) {
  const run = activeRun.value;
  if (!run) return;
  skipNextDomSync = true;
  draftRuns.value = draftRuns.value.map((r) =>
    r.id === run.id ? { ...r, font_size: size } : r
  );
  nextTick(() => {
    const el = inlineEditor.value?.querySelector<HTMLElement>(`[data-run-id="${run.id}"]`);
    if (el) applyRunStyleToSpan(el, draftRuns.value.find((r) => r.id === run.id)!);
  });
}

async function refreshPreview(objectId: number, text: string, selectionToken = getSelectionToken()) {
  if (pdfHandle.value == null) return;
  const requestId = incrementPreviewToken();
  try {
    const preview = await previewTextLayout(pdfHandle.value, objectId, text);
    if (requestId !== getPreviewToken() || selectionToken !== getSelectionToken() || selectedTextId.value !== objectId) {
      return;
    }
    layoutPreview.value = { ...preview, typography: draftTypography.value };
  } catch (error) {
    console.error(error);
    if (requestId !== getPreviewToken() || selectionToken !== getSelectionToken()) {
      return;
    }
    status.value = error instanceof Error ? error.message : "生成文本布局预览失败";
  }
}

async function saveTextEdit(options: { closeAfterSave?: boolean } = {}) {
  if (!selectedTextId.value || !canSaveEdit.value) return;
  clearPreviewTimer();
  isSavingEdit.value = true;
  status.value = `正在保存文本对象 ${selectedTextId.value}...`;
  const objectId = selectedTextId.value;
  const savedText = draftText.value;
  const originDelta = editorOriginDeltaPdf();
  const savedClipBounds = editorClipBoundsPdf();
  const savedBounds = translatedByOriginDelta(
    layoutPreview.value?.bbox ?? editSession.value?.bbox ?? selectedTextObject.value?.bounds ?? null,
    originDelta
  );
  // Overflow saves skip re-opening the edit session to avoid an extra WASM round-trip.
  const closeAfterSave = options.closeAfterSave ?? Boolean(layoutPreview.value?.overflow);

  try {
    if (pdfHandle.value != null) {
      // Ensure the embedded Noto SC font is loaded before writing so that any
      // CJK characters that cannot be encoded by the original font are stored
      // with the properly-embedded Noto font rather than the bare STSong-Light
      // standard font (which many PDF viewers cannot render).
      const cjkFontEmbedded = await setCjkFontByHandle(pdfHandle.value);
      if (draftUsesBuiltInFont.value && !cjkFontEmbedded) {
        throw new Error("保存失败：用于写入 PDF 的内置字体没有成功嵌入，请检查字体资源是否可加载后重试。");
      }
      await ensureSystemFontsForRuns(pdfHandle.value, draftRuns.value, draftTypography.value);

      // Fast path: update in-memory document, then refresh only the page structure.
      const existingImageUrls = new Map(
        (page.value?.images ?? []).map((img) => [img.id, img.objectUrl])
      );
      const textObj = page.value?.text.find((t) => t.id === objectId);
      if (
        draftRuns.value.length > 1
        || hasRunStyleChanges.value
        || hasEditorPositionChange.value
        || hasEditorClipChange.value
        || hasTypographyChanges.value
        || Boolean(layoutPreview.value?.overflow)
      ) {
        await updateTextRunsByHandle(
          pdfHandle.value, objectId, draftRuns.value,
          textObj?.color ?? { r: 0, g: 0, b: 0, a: 255 },
          editSession.value?.font_id ?? textObj?.font_name ?? null,
          editSession.value?.font_size ?? textObj?.font_size ?? 12,
          originDelta,
          savedClipBounds,
          draftTypography.value
        );
      } else {
        await updateTextByHandle(pdfHandle.value, objectId, savedText);
      }
      const structure = await getPageStructureByHandle(pdfHandle.value, pageNumber.value);
      // Re-attach blob URLs so image SVG overlays remain visible.
      for (const img of structure.images) {
        const url = existingImageUrls.get(img.id);
        if (url) img.objectUrl = url;
      }
      const nextObjectId = findSavedTextObjectId(structure.text, objectId, savedText, savedBounds);
      if (nextObjectId != null) {
        const savedObject = structure.text.find((item) => item.id === nextObjectId);
        if (savedObject) {
          if (savedClipBounds) savedObject.clip_bounds = savedClipBounds;
          savedObject.typography = draftTypography.value;
        }
      }
      page.value = structure;
      if (closeAfterSave) {
        clearEditingState();
      } else {
        if (nextObjectId == null) {
          clearEditingState();
        } else {
          await beginTextEdit(nextObjectId);
        }
      }
    } else {
      throw new Error("保存失败：PDF 文档句柄不存在，请重新打开文件。");
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

function editorOriginDeltaPdf() {
  const text = selectedTextObject.value;
  const viewport = currentViewport.value;
  if (!text || !viewport) return { x: 0, y: 0 };
  const geometry = inlineEditorGeometry(text);
  if (!geometry) return { x: 0, y: 0 };
  const offset = editorStartOffsetPx();
  if (Math.abs(offset) <= 0.5) return { x: 0, y: 0 };
  const from = viewportToPdf(viewport, geometry.origin.x, geometry.origin.y);
  const to = viewportToPdf(
    viewport,
    geometry.origin.x + geometry.axisX.x * offset,
    geometry.origin.y + geometry.axisX.y * offset
  );
  return { x: to.x - from.x, y: to.y - from.y };
}

function editorClipBoundsPdf(): StructuredTextObject["bounds"] | null {
  const text = selectedTextObject.value;
  const viewport = currentViewport.value;
  if (!text || !viewport) return null;
  const geometry = inlineEditorGeometry(text);
  if (!geometry) return null;
  const startOffset = editorStartOffsetPx();
  const width = editBoxWidthOverride.value ?? geometry.naturalWidth;
  const height = geometry.fontSize * 1.4;
  const origin = {
    x: geometry.origin.x + geometry.axisX.x * startOffset,
    y: geometry.origin.y + geometry.axisX.y * startOffset
  };
  const corners = [
    origin,
    {
      x: origin.x + geometry.axisX.x * width,
      y: origin.y + geometry.axisX.y * width
    },
    {
      x: origin.x + geometry.axisX.x * width + geometry.axisY.x * height,
      y: origin.y + geometry.axisX.y * width + geometry.axisY.y * height
    },
    {
      x: origin.x + geometry.axisY.x * height,
      y: origin.y + geometry.axisY.y * height
    }
  ].map((point) => viewportToPdf(viewport, point.x, point.y));
  const minX = Math.min(...corners.map((point) => point.x));
  const maxX = Math.max(...corners.map((point) => point.x));
  const minY = Math.min(...corners.map((point) => point.y));
  const maxY = Math.max(...corners.map((point) => point.y));
  return {
    origin: { x: minX, y: minY },
    size: { width: maxX - minX, height: maxY - minY }
  };
}

function translatedByOriginDelta(
  bounds: StructuredTextObject["bounds"] | null,
  delta: { x: number; y: number }
) {
  if (!bounds) return null;
  return {
    ...bounds,
    origin: {
      x: bounds.origin.x + delta.x,
      y: bounds.origin.y + delta.y
    }
  };
}

async function saveTextEditOnBlur() {
  if (isPreparingEdit.value || isSavingEdit.value) return;
  if (!selectedTextObject.value || !editSession.value) return;
  if (!canSaveEdit.value) {
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

async function onCanvasClick(event: MouseEvent) {
  if (!pdfBytes.value || !currentViewport.value || !page.value) return;
  if (window.getSelection()?.toString()) return;

  const target = event.currentTarget as HTMLElement;
  const rect = target.getBoundingClientRect();
  const offsetX = event.clientX - rect.left;
  const offsetY = event.clientY - rect.top;
  const pdfPoint = viewportToPdf(currentViewport.value, offsetX, offsetY);
  const hitObject = findTextObjectAtPoint(pdfPoint.x, pdfPoint.y);

  try {
    if (editSession.value || isPreparingEdit.value) {
      const nextObjectId = hitObject?.id !== selectedTextId.value ? hitObject?.id : null;
      if (isPreparingEdit.value) {
        clearSelection();
      } else {
        await saveTextEditOnBlur();
      }
      if (nextObjectId != null) {
        await beginTextEdit(nextObjectId);
      }
    } else if (hitObject) {
      await beginTextEdit(hitObject.id);
    } else {
      clearSelection();
    }
  } catch (error) {
    console.error("Hit test failed", error);
  }
}

function findTextObjectAtPoint(pdfX: number, pdfY: number) {
  const viewport = currentViewport.value;
  if (!viewport || !page.value) return null;
  const tolerance = Math.max(2 / viewport.zoom, 1);
  const candidates = renderTextObjects.value;
  for (let index = candidates.length - 1; index >= 0; index -= 1) {
    const text = candidates[index];
    const hitBounds = intersectRects(text.bounds, text.clip_bounds) ?? text.bounds;
    if (rectContainsPoint(hitBounds, pdfX, pdfY, tolerance)) {
      return text;
    }
  }
  return null;
}

function intersectRects(
  first: StructuredTextObject["bounds"],
  second: StructuredTextObject["bounds"] | null | undefined
) {
  if (!second) return first;
  const left = Math.max(first.origin.x, second.origin.x);
  const bottom = Math.max(first.origin.y, second.origin.y);
  const right = Math.min(first.origin.x + first.size.width, second.origin.x + second.size.width);
  const top = Math.min(first.origin.y + first.size.height, second.origin.y + second.size.height);
  if (right < left || top < bottom) return null;
  return {
    origin: { x: left, y: bottom },
    size: { width: right - left, height: top - bottom }
  };
}

function rectContainsPoint(rect: StructuredTextObject["bounds"], x: number, y: number, tolerance = 0) {
  return (
    x >= rect.origin.x - tolerance &&
    x <= rect.origin.x + rect.size.width + tolerance &&
    y >= rect.origin.y - tolerance &&
    y <= rect.origin.y + rect.size.height + tolerance
  );
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
  // viewport transform is applied by the outer <g svgViewportTransform>
  const matrix = multiplyMatrices(image.transform, [1, 0, 0, -1, 0, 1]);
  return `matrix(${matrix.map((value) => roundSvg(value)).join(" ")})`;
}

async function exportCurrentPdf() {
  const baseName = pdfFileName.value.replace(/\.pdf$/i, "") || "document";
  let bytes: Uint8Array;
  if (pdfHandle.value != null) {
    bytes = await getPdfBytesByHandle(pdfHandle.value);
  } else if (pdfBytes.value) {
    bytes = pdfBytes.value;
  } else {
    return;
  }
  const blob = new Blob([toArrayBuffer(bytes)], { type: "application/pdf" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `${baseName}-edited.pdf`;
  anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}

function svgTextTransform(text: StructuredTextObject) {
  // viewport transform is applied by the outer <g svgViewportTransform>
  const matrix = multiplyMatrices(text.transform, [1, 0, 0, -1, 0, 0]);
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
  // viewport transform is applied by the outer <g svgViewportTransform>
  const glyphMatrix: Matrix2D = [
    text.transform[0],
    text.transform[1],
    text.transform[2],
    text.transform[3],
    glyph.x,
    glyph.y
  ];
  const matrix = multiplyMatrices(glyphMatrix, [1, 0, 0, -1, 0, 0]);
  return `matrix(${matrix.map((v) => roundSvg(v)).join(" ")})`;
}

function glyphHasSvgPath(glyph: LayoutGlyph) {
  return Boolean(glyph.svg_fill_path || glyph.svg_stroke_path);
}


function svgGlyphPathTransform(glyph: LayoutGlyph): string {
  // viewport transform is applied by the outer <g svgViewportTransform>;
  // svg_transform maps from font/glyph space to PDF page space.
  return glyph.svg_transform ? `matrix(${glyph.svg_transform.map(roundSvg).join(" ")})` : "";
}

function svgGlyphPathStrokeWidth(glyph: LayoutGlyph) {
  return glyph.svg_stroke_width == null ? undefined : roundSvg(glyph.svg_stroke_width);
}

/**
 * Returns the PDF-space rect to use as the SVG clip boundary for a text object,
 * or null if no clip should be applied.  Priority:
 *  1. Session bbox during active editing — gives the inline editor its boundary.
 *  2. clip_bounds from the PDF content stream — set when the PDF has a
 *     `q re W n … Q` sequence (or when a prior save added one for overflow).
 *  3. null — unedited text with no explicit PDF clip is NOT clipped in the SVG.
 *     Previously the fallback was text.bounds, but that caused glyph-ink to be
 *     clipped when the last character's ink extended past its advance width.
 */
function textClipAttrs(text: StructuredTextObject): { x: number; y: number; width: number; height: number } | null {
  const b =
    text.id === selectedTextId.value && editSession.value
      ? editorClipBoundsPdf()
      : text.clip_bounds ?? null;
  if (!b) return null;
  return { x: b.origin.x, y: b.origin.y, width: b.size.width, height: b.size.height };
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}
</script>

<template>
  <main class="app-shell">
    <aside class="sidebar">
      <header>
        <h1>PDF Editor Web</h1>
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
        <button @click="loadSystemFonts">加载系统字体</button>
      </div>

      <p class="status">{{ status }}</p>

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
        <RunsEditor
          :runs="draftRuns"
          :font-assets="allFontOptions"
          :base-font-name="editSession?.font_id ?? selectedTextObject.font_name ?? null"
          :base-font-size="editSession?.font_size ?? selectedTextObject.font_size"
          :base-color="selectedTextObject.color"
          :disabled="isPreparingEdit || isSavingEdit"
          @update:runs="(newRuns) => { draftRuns = newRuns; onDraftInput(); }"
          @interact="onEditorPanelInteraction"
        />
        <p class="helper-text" :class="{ danger: hasEffectiveOverflow }">
          {{ previewStatus }}
        </p>
        <button class="save-button" :disabled="!canSaveEdit" @click="saveTextEdit()">
          {{ isSavingEdit ? "正在保存..." : "保存到 PDF" }}
        </button>
      </section>

    </aside>

    <section class="canvas-pane">
      <div v-if="page && backgroundUrl && currentViewport" class="page-viewport" :style="pageViewportStyle()">
        <div class="page-canvas" :style="pageCanvasStyle()" @click="onCanvasClick">
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
            <defs>
              <!--
                Clip path rects are in PDF page coordinates.  The outer <g> applies
                the viewport transform, so clipPathUnits="userSpaceOnUse" coordinates
                are resolved in PDF space — matching the elements they clip.
                Only rendered when textClipAttrs returns a non-null rect; text without
                an explicit PDF clip sequence (or active edit bbox) is not clipped.
              -->
              <clipPath
                v-for="item in textRenderItems.filter(i => textClipAttrs(i.text) != null)"
                :key="`clip-${item.text.id}`"
                :id="`clip-text-${item.text.id}`"
              >
                <rect v-bind="textClipAttrs(item.text)!" />
              </clipPath>
            </defs>
            <!--
              Single <g> carrying the viewport→PDF transform.  All child elements
              use only their PDF-local transforms, so changing zoom/pan updates
              only this one attribute instead of thousands of per-glyph matrices.
            -->
            <g :transform="svgViewportTransform">
              <image
                v-for="image in renderImageObjects"
                :key="`image-${image.id}`"
                :href="image.objectUrl"
                width="1"
                height="1"
                preserveAspectRatio="none"
                :transform="svgImageTransform(image)"
              />
              <!-- v-memo="[item.text]": stable items skip VNode creation; only the edited item re-renders. -->
              <g v-for="item in textRenderItems" :key="`text-${item.text.id}`" v-memo="[item.text]">
                <!--
                  Per-glyph absolute rendering: each character placed at its exact
                  PDF-space position for precise alignment with the background.
                  A nearly-invisible copyable-text-run overlay (rendered first)
                  is the target for browser text selection, giving a single
                  contiguous selection box that matches the PDF advance widths.
                  Individual glyph <text> elements follow with pointer-events and
                  user-select disabled so selection falls through to the overlay.
                -->
                <g
                  v-if="item.glyphs"
                  :data-object-id="item.text.id"
                  :clip-path="textClipAttrs(item.text) != null ? `url(#clip-text-${item.text.id})` : undefined"
                >
                  <text
                    class="copyable-text-run"
                    :transform="svgTextTransform(item.text)"
                    :font-family="svgFontFamily(item.text.font_name)"
                    :font-weight="fontWeightFor(item.text.font_name)"
                    font-size="1"
                    dominant-baseline="alphabetic"
                    xml:space="preserve"
                    :textLength="item.textLength"
                    :lengthAdjust="item.textLength != null ? 'spacingAndGlyphs' : undefined"
                  >{{ item.text.content }}</text>
                  <template
                    v-for="(glyph, glyphIndex) in item.glyphs"
                    :key="`glyph-${item.text.id}-${glyphIndex}`"
                  >
                    <g
                      v-if="glyphHasSvgPath(glyph)"
                      class="type3-glyph-path"
                      :transform="svgGlyphPathTransform(glyph)"
                    >
                      <path
                        v-if="glyph.svg_fill_path"
                        :d="glyph.svg_fill_path"
                        :fill="svgFill(item.text)"
                        stroke="none"
                        fill-rule="nonzero"
                      />
                      <path
                        v-if="glyph.svg_stroke_path"
                        :d="glyph.svg_stroke_path"
                        fill="none"
                        :stroke="svgStroke(item.text) === 'none' ? svgFill(item.text) : svgStroke(item.text)"
                        :stroke-width="svgGlyphPathStrokeWidth(glyph)"
                      />
                    </g>
                    <text
                      v-else
                      class="per-glyph-text"
                      :transform="svgGlyphTransform(glyph, item.text)"
                      :font-family="svgFontFamily(glyph.font_name ?? item.text.font_name)"
                      :font-weight="fontWeightFor(glyph.font_name ?? item.text.font_name)"
                      :fill="svgFill(item.text)"
                      :stroke="svgStroke(item.text)"
                      :stroke-width="svgStrokeWidth(item.text)"
                      :paint-order="svgPaintOrder(item.text)"
                      :style="item.fontFeatureStyle"
                      font-size="1"
                      dominant-baseline="alphabetic"
                    >{{ glyph.ch }}</text>
                  </template>
                </g>
                <!--
                  Legacy textLength rendering for text objects without per-glyph
                  position data.  A single <text> distributes characters via
                  lengthAdjust across the object's PDF-measured width.

                  The clip-path MUST live on this transform-less wrapping <g>, not
                  on the inner <text>.  clipPathUnits="userSpaceOnUse" resolves the
                  clip rect in the user space of the element referencing it; the
                  <text> carries its own font matrix transform, which would
                  reinterpret the PDF-space clip coordinates in the text's local
                  (font-size-1) space and clip the entire line away.  The per-glyph
                  path above wraps its clip on a <g> for the same reason.
                -->
                <g
                  v-else
                  :clip-path="textClipAttrs(item.text) != null ? `url(#clip-text-${item.text.id})` : undefined"
                >
                  <text
                    :transform="svgTextTransform(item.text)"
                    :font-family="svgFontFamily(item.text.font_name)"
                    :font-weight="fontWeightFor(item.text.font_name)"
                    :fill="svgFill(item.text)"
                    :stroke="svgStroke(item.text)"
                    :stroke-width="svgStrokeWidth(item.text)"
                    :paint-order="svgPaintOrder(item.text)"
                    :style="item.fontFeatureStyle"
                    font-size="1"
                    xml:space="preserve"
                    dominant-baseline="alphabetic"
                    :textLength="item.textLength"
                    :lengthAdjust="item.textLength != null ? 'spacingAndGlyphs' : undefined"
                  >
                    <template v-if="svgTextRuns(item.text)">
                      <tspan
                        v-for="(run, runIndex) in svgTextRuns(item.text) ?? []"
                        :key="`run-${item.text.id}-${runIndex}`"
                        :font-family="svgFontFamily(run.font_name)"
                        :font-weight="fontWeightFor(run.font_name)"
                        :fill="colorToCss(run.color)"
                      >
                        {{ run.content }}
                      </tspan>
                    </template>
                    <template v-else-if="svgTextLines(item.text).length > 1">
                      <tspan
                        v-for="(line, lineIndex) in svgTextLines(item.text)"
                        :key="`line-${item.text.id}-${lineIndex}`"
                        x="0"
                        :y="lineIndex === 0 ? 0 : lineIndex * 1.2"
                      >
                        {{ line }}
                      </tspan>
                    </template>
                    <template v-else>{{ item.text.content }}</template>
                  </text>
                </g>
              </g>
            </g>
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
            <!-- Font/size toolbar for the active run -->
            <div class="inline-run-toolbar" @pointerdown.stop @click.stop>
              <select
                :value="activeRun?.font_name ?? ''"
                :disabled="isPreparingEdit || isSavingEdit"
                title="当前段字体"
                @mousedown="onToolbarMousedown"
                @change="onToolbarFontChangeEvent($event)"
                @blur="onToolbarControlBlur($event)"
              >
                <option value="">（继承）</option>
                <optgroup label="内置字体">
                  <option
                    v-for="font in BUILTIN_FONTS"
                    :key="font.resource_name"
                    :value="font.resource_name"
                  >{{ font.family_name }}</option>
                </optgroup>
                <optgroup v-if="systemFontOptions.length" label="系统字体">
                  <option
                    v-for="font in systemFontOptions"
                    :key="font.resource_name"
                    :value="font.resource_name"
                  >{{ font.family_name }}</option>
                </optgroup>
                <optgroup v-if="fontAssets.length" label="嵌入字体">
                  <option
                    v-for="font in fontAssets"
                    :key="font.resource_name"
                    :value="font.resource_name"
                  >{{ font.family_name }}</option>
                </optgroup>
              </select>
              <input
                type="number"
                :value="activeRun?.font_size ?? (editSession?.font_size ?? selectedTextObject.font_size)"
                :disabled="isPreparingEdit || isSavingEdit"
                min="1"
                max="500"
                step="0.5"
                title="当前段字号"
                class="toolbar-size-input"
                @mousedown="onToolbarMousedown"
                @change="onToolbarSizeChangeEvent($event)"
                @blur="onToolbarControlBlur($event)"
              />
              <span class="toolbar-run-label" v-if="draftRuns.length > 1">
                段 {{ (draftRuns.findIndex(r => r.id === (activeRunId ?? draftRuns[0]?.id)) + 1) }} / {{ draftRuns.length }}
              </span>
              <span class="toolbar-separator" aria-hidden="true"></span>
              <label class="toolbar-toggle" title="保存时将普通空格写为 TJ 数字位移">
                <input
                  type="checkbox"
                  :checked="draftTypography.replace_spaces_with_displacements"
                  :disabled="isPreparingEdit || isSavingEdit"
                  @mousedown="onToolbarMousedown"
                  @change="onToolbarTypographyChange({ replace_spaces_with_displacements: ($event.target as HTMLInputElement).checked })"
                  @blur="onToolbarControlBlur($event)"
                />
                <span>TJ 空格</span>
              </label>
              <label class="toolbar-toggle" title="保存时对单个或多个标点使用 TJ 位移压缩">
                <input
                  type="checkbox"
                  :checked="draftTypography.compress_punctuation"
                  :disabled="isPreparingEdit || isSavingEdit"
                  @mousedown="onToolbarMousedown"
                  @change="onToolbarTypographyChange({ compress_punctuation: ($event.target as HTMLInputElement).checked })"
                  @blur="onToolbarControlBlur($event)"
                />
                <span>标点压缩</span>
              </label>
              <select
                class="toolbar-digit-font-select"
                :value="draftTypography.digit_font_name ?? ''"
                :disabled="isPreparingEdit || isSavingEdit"
                title="数字字体"
                @mousedown="onToolbarMousedown"
                @change="onToolbarTypographyChange({ digit_font_name: ($event.target as HTMLSelectElement).value || null })"
                @blur="onToolbarControlBlur($event)"
              >
                <option value="">数字继承</option>
                <option
                  v-for="font in allFontOptions"
                  :key="`toolbar-digit-${font.resource_name}`"
                  :value="font.resource_name"
                >{{ font.family_name }}</option>
              </select>
              <span
                class="toolbar-run-label"
                v-if="draftTypography.detected_tj_displacements || draftTypography.detected_space_displacements || draftTypography.detected_punctuation || draftTypography.detected_digit_font_name"
                :title="[
                  draftTypography.detected_tj_displacements ? 'TJ 位移' : '',
                  draftTypography.detected_space_displacements ? '位移空格' : '',
                  draftTypography.detected_punctuation ? '标点压缩' : '',
                  draftTypography.detected_digit_font_name ? '数字字体' : ''
                ].filter(Boolean).join(' / ')"
              >已识别</span>
            </div>

            <div
              class="editor-edge editor-edge-left"
              title="拖拽调整编辑框宽度"
              @mousedown.prevent
              @pointerdown.prevent.stop="onEdgeDragStart($event, 'left')"
              @click.stop
            />
            <div
              class="editor-move-edge editor-move-edge-top"
              title="拖拽移动编辑框位置"
              @mousedown.prevent
              @pointerdown.prevent.stop="onMoveDragStart($event)"
              @click.stop
            />
            <div
              ref="inlineEditor"
              class="inline-text-editor"
              :class="{ overflow: hasEffectiveOverflow, disabled: isPreparingEdit || isSavingEdit }"
              :style="inlineEditorStyle"
              :contenteditable="(!isPreparingEdit && !isSavingEdit) ? 'true' : 'false'"
              spellcheck="false"
              @input.stop="onInlineEditorInput"
              @keydown="onInlineEditorKeydown"
              @focus="onInlineEditorFocus"
              @blur="onInlineEditorBlur"
              @pointerdown.stop
              @click.stop
            />
            <div
              class="editor-move-edge editor-move-edge-bottom"
              title="拖拽移动编辑框位置"
              @mousedown.prevent
              @pointerdown.prevent.stop="onMoveDragStart($event)"
              @click.stop
            />
            <div
              class="editor-edge editor-edge-right"
              title="拖拽调整编辑框宽度"
              @mousedown.prevent
              @pointerdown.prevent.stop="onEdgeDragStart($event, 'right')"
              @click.stop
            />
          </div>
        </div>
      </div>
      <div v-else class="empty-state">加载 PDF 后显示 PNG 背景与 SVG 文本层，并可直接点击文本进行编辑</div>
    </section>
  </main>
</template>

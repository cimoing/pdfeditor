import init, {
  pdf_apply_text_edits,
  pdf_apply_text_edits_by_handle,
  pdf_close_document,
  pdf_commit_text_edit,
  pdf_commit_text_edit_by_handle,
  pdf_get_bytes_by_handle,
  pdf_hit_test,
  pdf_hit_test_by_handle,
  pdf_open_document,
  pdf_page_bundle,
  pdf_page_bundle_by_handle,
  pdf_page_structure_by_handle,
  pdf_preview_text_layout,
  pdf_preview_text_layout_by_handle,
  pdf_set_cjk_font_by_handle,
  pdf_set_local_font_by_handle,
  pdf_start_text_edit,
  pdf_start_text_edit_by_handle,
  pdf_update_text_by_handle,
  pdf_update_text_runs_by_handle
} from "./wasm/pdfeditor_core";
import notoSansScWoffUrl from "@fontsource/noto-sans-sc/files/noto-sans-sc-chinese-simplified-400-normal.woff?url";

export interface Point {
  x: number;
  y: number;
}

export interface Size {
  width: number;
  height: number;
}

export interface Rect {
  origin: Point;
  size: Size;
}

export interface PageInfo {
  index: number;
  size: Size;
  rotation?: number;
}

export interface StructuredTextObject {
  id: number;
  bounds: Rect;
  content: string;
  font_name: string | null;
  font_size: number;
  color: { r: number; g: number; b: number; a: number };
  stroke_color: { r: number; g: number; b: number; a: number };
  stroke_width: number;
  rendering_mode: number;
  char_spacing: number;
  word_spacing: number;
  horizontal_scaling: number;
  transform: [number, number, number, number, number, number];
  angle_degrees: number;
  z_index: number;
  glyphs?: LayoutGlyph[];
  runs?: Array<{
    content: string;
    font_name: string | null;
    font_size: number;
    color: { r: number; g: number; b: number; a: number };
  }>;
  /** True when the font defines reduced advance widths for fullwidth CJK punctuation (标点宽度替换). */
  punct_width_squeeze?: boolean;
  /** OpenType layout features detected in the font (subset of: palt, halt, kern, liga, fwid, hwid). */
  font_features?: string[];
  /** Clipping rectangle from a surrounding `q re W n … Q` sequence in the PDF content stream.
   *  When present, the SVG overlay should clip this text to this rect rather than `bounds`. */
  clip_bounds?: Rect;
}

export interface TextRunInfo {
  content: string;
  font_name: string | null;
  font_size: number;
  color: { r: number; g: number; b: number; a: number };
}

/** A rich text run used in the editor.  `null` fields inherit from the base text object. */
export interface RichTextRun {
  id: string;
  content: string;
  font_name: string | null;
  font_size: number | null;
  color: { r: number; g: number; b: number; a: number } | null;
}

export interface StructuredVisualTextObject {
  id: number;
  bounds: Rect;
  font_name: string | null;
  font_size: number;
  transform: [number, number, number, number, number, number];
  angle_degrees: number;
  z_index: number;
}

export interface StructuredImageObject {
  id: number;
  name: string | null;
  source_file: string | null;
  bounds: Rect;
  transform: [number, number, number, number, number, number];
  angle_degrees: number;
  width_px: number | null;
  height_px: number | null;
  filters: string[];
  z_index: number;
  objectUrl?: string;
}

export interface PageStructure {
  page: PageInfo;
  text: StructuredTextObject[];
  visual_text?: StructuredVisualTextObject[];
  images: StructuredImageObject[];
}

export interface HitTestResult {
  object_id: number;
  object_type: "text" | "image" | string;
  page: number;
  local_position: Point;
  text_run_index: number | null;
  glyph_index: number | null;
  bbox: Rect;
  matrix: [number, number, number, number, number, number];
}

export interface LayoutGlyph {
  ch: string;
  glyph_id: number | null;
  font_name?: string | null;
  x: number;
  y: number;
  advance: number;
  width: number;
  bbox: Rect;
  svg_fill_path?: string | null;
  svg_stroke_path?: string | null;
  svg_stroke_width?: number | null;
  svg_transform?: [number, number, number, number, number, number] | null;
}

export interface TextEditSessionInfo {
  object_id: number;
  page: number;
  original_text: string;
  group_object_ids: number[];
  bbox: Rect;
  matrix: [number, number, number, number, number, number];
  font_id: string | null;
  font_size: number;
  writing_mode: string | null;
  glyphs: LayoutGlyph[];
}

export interface TextLayoutPreview {
  object_id: number;
  text: string;
  group_object_ids: number[];
  glyphs: LayoutGlyph[];
  bbox: Rect;
  overflow: boolean;
}

export interface EmbeddedFontInfo {
  resource_name: string;
  family_name: string;
  font_weight: number;
  is_bold: boolean;
  file_name: string;
  mime_type: string;
  format: string;
}

export interface LoadedFontAsset extends EmbeddedFontInfo {
  data: ArrayBuffer;
}

export interface LocalSystemFontOption {
  resource_name: string;
  family_name: string;
  css_family: string;
  full_name: string;
  postscript_name: string;
}

export interface LoadedPage {
  structure: PageStructure;
  backgroundUrl: string;
  fontFamilies: Record<string, string>;
  fontAssets: LoadedFontAsset[];
}

interface BinaryAssetInfo {
  file_name: string;
  mime_type: string;
  offset: number;
  length: number;
}

interface ImageBundleInfo {
  id: number;
  file_name: string;
  width_px: number;
  height_px: number;
  asset: BinaryAssetInfo;
}

interface FontBundleInfo extends EmbeddedFontInfo {
  asset: BinaryAssetInfo;
}

interface PageBundleInfo {
  structure: PageStructure;
  background_png: BinaryAssetInfo;
  images: ImageBundleInfo[];
  fonts: FontBundleInfo[];
}

let wasmReady: Promise<void> | null = null;
let fontLoadSequence = 0;
let loadedFontFaces: FontFace[] = [];
let loadedFontUrls: string[] = [];

// "Noto Sans SC" is loaded via @fontsource (web font, always available in this app).
// It covers the full Simplified Chinese Unicode range and serves as the primary
// fallback for any embedded PDF font whose subset is missing certain glyphs.
const CJK_CSS_FALLBACK = '"Noto Sans SC", "Noto Sans CJK SC", "Microsoft YaHei", SimSun';
const fallbackFontFamily =
  `${CJK_CSS_FALLBACK}, "Helvetica Neue", Arial, "Segoe UI", "Noto Sans", sans-serif`;

const fontNameFallbacks: Array<[RegExp, string]> = [
  [/simsun|song|stsong|宋体/i, '"Noto Serif CJK SC", SimSun, "Songti SC"'],
  [/simhei|hei|黑体/i, '"Noto Sans CJK SC", SimHei, "Microsoft YaHei"'],
  [/yahei|微软雅黑/i, '"Microsoft YaHei", "Noto Sans CJK SC"'],
  [/kai|楷体/i, 'KaiTi, "Kaiti SC", "Noto Serif CJK SC"'],
  [/fangsong|仿宋/i, 'FangSong, STFangsong, "Noto Serif CJK SC"'],
  [/times|serif/i, '"Times New Roman", Times, serif'],
  [/courier|mono/i, '"Courier New", Courier, monospace'],
  [/helvetica|arial/i, 'Arial, "Helvetica Neue"']
];

export function ensureWasm(): Promise<void> {
  wasmReady ??= init().then(() => undefined);
  return wasmReady;
}

export async function openPdfDocument(pdfBytes: Uint8Array): Promise<number> {
  await ensureWasm();
  return pdf_open_document(pdfBytes);
}

export function closePdfDocument(handle: number | null) {
  if (handle == null) return;
  try {
    pdf_close_document(handle);
  } catch (error) {
    console.warn(`Failed to close PDF document handle ${handle}`, error);
  }
}

export async function loadPdfPage(
  pdfBytes: Uint8Array,
  pageNumber: number,
  handle?: number | null
): Promise<LoadedPage> {
  await ensureWasm();
  releaseLoadedFonts();
  const bundleBytes =
    handle == null ? pdf_page_bundle(pdfBytes, pageNumber) : pdf_page_bundle_by_handle(handle, pageNumber);
  const { metadata, payload } = parsePageBundle(bundleBytes);
  const structure = normalizePageStructure(metadata.structure);
  const fontFamilies = await loadEmbeddedFonts(metadata.fonts, payload);
  const fontAssets = metadata.fonts.map((font) => ({
    resource_name: font.resource_name,
    family_name: font.family_name,
    font_weight: font.font_weight,
    is_bold: font.is_bold,
    file_name: font.file_name,
    mime_type: font.mime_type,
    format: font.format,
    data: assetBlobPart(payload, font.asset)
  }));
  const backgroundUrl = URL.createObjectURL(
    new Blob([assetBlobPart(payload, metadata.background_png)], { type: metadata.background_png.mime_type })
  );

  const imageAssets = new Map(metadata.images.map((image) => [image.id, image.asset]));
  for (const image of structure.images) {
    const asset = imageAssets.get(image.id);
    if (!asset) continue;
    image.objectUrl = URL.createObjectURL(new Blob([assetBlobPart(payload, asset)], { type: asset.mime_type }));
  }

  return { structure, backgroundUrl, fontFamilies, fontAssets };
}

export async function applyTextEdits(
  pdfBytes: Uint8Array | null,
  edits: Array<{ id: number; content: string }>,
  handle?: number | null
): Promise<Uint8Array> {
  await ensureWasm();
  const request = {
    edits: edits.map((edit) => ({
      type: "replace_text",
      id: edit.id,
      content: edit.content
    }))
  };
  if (handle != null) {
    return pdf_apply_text_edits_by_handle(handle, JSON.stringify(request));
  }
  if (!pdfBytes) {
    throw new Error("Missing PDF bytes for text edit request");
  }
  return pdf_apply_text_edits(pdfBytes, JSON.stringify(request));
}

export async function hitTestPdf(
  pdfBytes: Uint8Array | null,
  pageNumber: number,
  pdfX: number,
  pdfY: number,
  handle?: number | null
): Promise<HitTestResult | null> {
  await ensureWasm();
  const json =
    handle == null
      ? pdf_hit_test(requirePdfBytes(pdfBytes), pageNumber, pdfX, pdfY)
      : pdf_hit_test_by_handle(handle, pageNumber, pdfX, pdfY);
  return JSON.parse(json) as HitTestResult | null;
}

export async function startTextEdit(
  pdfBytes: Uint8Array | null,
  objectId: number,
  handle?: number | null
): Promise<TextEditSessionInfo> {
  await ensureWasm();
  const json =
    handle == null
      ? pdf_start_text_edit(requirePdfBytes(pdfBytes), BigInt(objectId))
      : pdf_start_text_edit_by_handle(handle, BigInt(objectId));
  return normalizeTextEditSession(JSON.parse(json) as TextEditSessionInfo);
}

export async function previewTextLayout(
  pdfBytes: Uint8Array | null,
  objectId: number,
  text: string,
  handle?: number | null
): Promise<TextLayoutPreview> {
  await ensureWasm();
  const json =
    handle == null
      ? pdf_preview_text_layout(requirePdfBytes(pdfBytes), BigInt(objectId), text)
      : pdf_preview_text_layout_by_handle(handle, BigInt(objectId), text);
  return normalizeTextLayoutPreview(JSON.parse(json) as TextLayoutPreview);
}

export async function commitTextEdit(
  pdfBytes: Uint8Array | null,
  objectId: number,
  text: string,
  handle?: number | null
): Promise<Uint8Array> {
  await ensureWasm();
  return handle == null
    ? pdf_commit_text_edit(requirePdfBytes(pdfBytes), BigInt(objectId), text)
    : pdf_commit_text_edit_by_handle(handle, BigInt(objectId), text);
}

export async function updateTextByHandle(handle: number, objectId: number, text: string): Promise<void> {
  await ensureWasm();
  pdf_update_text_by_handle(handle, BigInt(objectId), text);
}

/** Cached Noto Sans SC woff bytes — fetched once, reused for every document. */
let _notoWoffCache: Uint8Array | null = null;

/**
 * Pre-load and embed the Noto Sans SC WOFF1 font into the in-memory PDF document.
 *
 * After this call, any CJK characters that cannot be encoded by the page's original
 * fonts will be stored using the embedded NotoSansSC TrueType font (Identity-H)
 * instead of the unembedded STSong-Light standard font, which many modern PDF viewers
 * (especially browser-based) cannot render, producing boxes.
 *
 * Returns true if the backend accepted the font.  A false return means edits that
 * need CJK/built-in fallback would be written with the legacy unembedded PDF
 * fallback font, which can render as boxes in many viewers.
 */
export async function setCjkFontByHandle(handle: number): Promise<boolean> {
  await ensureWasm();
  if (!_notoWoffCache) {
    try {
      const response = await fetch(notoSansScWoffUrl);
      if (!response.ok) return false;
      _notoWoffCache = new Uint8Array(await response.arrayBuffer());
    } catch {
      return false;
    }
  }
  return pdf_set_cjk_font_by_handle(handle, _notoWoffCache);
}

export async function setLocalFontByHandle(
  handle: number,
  resourceName: string,
  fontBytes: Uint8Array
): Promise<boolean> {
  await ensureWasm();
  const key = localFontKey(resourceName);
  if (!key) return false;
  return pdf_set_local_font_by_handle(handle, key, fontBytes);
}

function localFontKey(resourceName: string): string | null {
  return resourceName.startsWith("__localfont__:")
    ? resourceName.slice("__localfont__:".length)
    : null;
}

export async function updateTextRunsByHandle(
  handle: number,
  objectId: number,
  runs: RichTextRun[],
  baseColor: { r: number; g: number; b: number; a: number },
  baseFontName: string | null,
  baseFontSize: number,
  originDelta: { x: number; y: number } = { x: 0, y: 0 },
  clipBounds: Rect | null = null
): Promise<void> {
  await ensureWasm();
  // Built-in browser fonts need PDF-side resource names.  Noto Sans SC keeps using
  // the embedded CJK fallback; Latin generic families map to standard PDF fonts.
  const builtinPdfFontName = (fontName: string | null | undefined) => {
    switch (fontName) {
      case "__builtin__:monospace":
        return "__pdfeditor_builtin_monospace__";
      case "__builtin__:serif":
        return "__pdfeditor_builtin_serif__";
      case "__builtin__:sans-serif":
        return "__pdfeditor_builtin_sans__";
      case "__builtin__:Noto Sans SC":
        return "__cjk_fallback__";
      default:
        return fontName ?? baseFontName;
    }
  };
  const payload = runs.map((run) => ({
    content: run.content,
    font_name: builtinPdfFontName(run.font_name),
    font_size: run.font_size ?? baseFontSize,
    color: (() => {
      const c = run.color ?? baseColor;
      return [c.r, c.g, c.b, c.a];
    })()
  }));
  pdf_update_text_runs_by_handle(
    handle,
    BigInt(objectId),
    JSON.stringify(payload),
    originDelta.x,
    originDelta.y,
    clipBounds ? JSON.stringify(clipBounds) : ""
  );
}

export async function getPageStructureByHandle(handle: number, pageNumber: number): Promise<PageStructure> {
  await ensureWasm();
  const json = pdf_page_structure_by_handle(handle, pageNumber);
  return normalizePageStructure(JSON.parse(json) as PageStructure);
}

export async function getPdfBytesByHandle(handle: number): Promise<Uint8Array> {
  await ensureWasm();
  return pdf_get_bytes_by_handle(handle);
}

export function asBlobPart(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

function requirePdfBytes(pdfBytes: Uint8Array | null): Uint8Array {
  if (!pdfBytes) {
    throw new Error("Missing PDF bytes");
  }
  return pdfBytes;
}

function parsePageBundle(bytes: Uint8Array): { metadata: PageBundleInfo; payload: Uint8Array } {
  if (bytes.byteLength < 4) {
    throw new Error("Invalid page bundle: missing metadata length");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const jsonLength = view.getUint32(0, false);
  const jsonStart = 4;
  const jsonEnd = jsonStart + jsonLength;
  if (jsonEnd > bytes.byteLength) {
    throw new Error("Invalid page bundle: metadata length exceeds bundle size");
  }
  const json = new TextDecoder().decode(bytes.subarray(jsonStart, jsonEnd));
  return {
    metadata: JSON.parse(json) as PageBundleInfo,
    payload: bytes.subarray(jsonEnd)
  };
}

function normalizePageStructure(structure: PageStructure): PageStructure {
  return {
    ...structure,
    text: structure.text.map((text) => ({
      ...text,
      content: normalizeCompatibilityText(text.content),
      glyphs: normalizeLayoutGlyphs(text.glyphs),
      runs: text.runs?.map((run) => ({
        ...run,
        content: normalizeCompatibilityText(run.content)
      })) ?? []
    }))
  };
}

function normalizeTextEditSession(session: TextEditSessionInfo): TextEditSessionInfo {
  return {
    ...session,
    original_text: normalizeCompatibilityText(session.original_text),
    group_object_ids: session.group_object_ids ?? [session.object_id],
    glyphs: normalizeLayoutGlyphs(session.glyphs)
  };
}

function normalizeTextLayoutPreview(preview: TextLayoutPreview): TextLayoutPreview {
  return {
    ...preview,
    text: normalizeCompatibilityText(preview.text),
    group_object_ids: preview.group_object_ids ?? [preview.object_id],
    glyphs: normalizeLayoutGlyphs(preview.glyphs)
  };
}

function normalizeLayoutGlyphs(glyphs: LayoutGlyph[] | undefined): LayoutGlyph[] {
  return glyphs?.map((glyph) => ({
    ...glyph,
    font_name: glyph.font_name ?? null,
    svg_fill_path: glyph.svg_fill_path ?? null,
    svg_stroke_path: glyph.svg_stroke_path ?? null,
    svg_stroke_width: glyph.svg_stroke_width ?? null,
    svg_transform: glyph.svg_transform ?? null,
    ch: normalizeCompatibilityText(glyph.ch)
  })) ?? [];
}

function normalizeCompatibilityText(value: string): string {
  // NFC: canonical decomposition + recomposition only.
  // This handles combining-character sequences (e.g. e + combining-accent → é)
  // without collapsing compatibility forms such as fullwidth punctuation:
  //   NFKC would turn "，" (U+FF0C) → "," (U+002C), "：" → ":", "（" → "(", etc.
  //   Those characters must reach the backend unchanged so they are encoded with
  //   the correct fullwidth glyph rather than the narrow ASCII substitute.
  return value ? value.normalize("NFC") : value;
}

function assetBlobPart(payload: Uint8Array, asset: BinaryAssetInfo): ArrayBuffer {
  return asBlobPart(payload.subarray(asset.offset, asset.offset + asset.length));
}

export function resolvePdfFontFamily(fontName: string | null, embeddedFonts: Record<string, string>): string {
  if (fontName && embeddedFonts[fontName]) {
    return embeddedFonts[fontName];
  }
  return withFallbackFonts(mappedFallbackForFont(fontName));
}

export interface PdfFontUsageInfo {
  requestedFont: string | null;
  displayFamily: string;
  cssFontFamily: string;
  fellBack: boolean;
  fallbackReason: string | null;
}

export function describePdfFontUsage(
  fontName: string | null,
  embeddedFonts: Record<string, string>
): PdfFontUsageInfo {
  const cssFontFamily = resolvePdfFontFamily(fontName, embeddedFonts);
  const displayFamily = firstCssFamilyName(cssFontFamily);
  const hasEmbedded = fontName ? Boolean(embeddedFonts[fontName]?.includes("PdfEmbedded_")) : false;

  if (!fontName) {
    return {
      requestedFont: null,
      displayFamily,
      cssFontFamily,
      fellBack: true,
      fallbackReason: "PDF 对象未提供字体资源名，使用浏览器回退字体链"
    };
  }

  if (!embeddedFonts[fontName]) {
    return {
      requestedFont: fontName,
      displayFamily,
      cssFontFamily,
      fellBack: true,
      fallbackReason: "未找到对应的嵌入字体映射，使用浏览器回退字体链"
    };
  }

  if (!hasEmbedded) {
    return {
      requestedFont: fontName,
      displayFamily,
      cssFontFamily,
      fellBack: true,
      fallbackReason: "嵌入字体未成功加载，使用浏览器回退字体链"
    };
  }

  return {
    requestedFont: fontName,
    displayFamily,
    cssFontFamily,
    fellBack: false,
    fallbackReason: null
  };
}

async function loadEmbeddedFonts(fonts: FontBundleInfo[], payload: Uint8Array): Promise<Record<string, string>> {
  const result: Record<string, string> = {};

  for (const font of fonts) {
    let blobUrl: string | null = null;
    try {
      if (!isBrowserLoadableFont(font.format)) {
        result[font.resource_name] = withFallbackFonts(mappedFallbackForFont(font.family_name));
        continue;
      }
      blobUrl = URL.createObjectURL(new Blob([assetBlobPart(payload, font.asset)], { type: font.mime_type }));
      const family = `PdfEmbedded_${sanitizeCssName(font.resource_name)}_${fontLoadSequence++}`;
      // Use the CSS-compatible format hint. CFF data is exposed as "opentype"
      // because browsers handle CFF outlines inside OpenType containers.
      const source = `url(${blobUrl}) format("${cssFontFormat(font.format)}")`;
      const face = new FontFace(family, source, {
        weight: `${font.font_weight || 400}`
      });
      await face.load();
      document.fonts.add(face);
      loadedFontFaces.push(face);
      loadedFontUrls.push(blobUrl);
      result[font.resource_name] = withFallbackFonts(
        `${quoteCssFamily(family)}, ${mappedFallbackForFont(font.family_name) ?? ""}`
      );
    } catch (error) {
      if (blobUrl) URL.revokeObjectURL(blobUrl);
      console.warn(`Failed to load embedded PDF font ${font.resource_name}`, error);
      result[font.resource_name] = withFallbackFonts(mappedFallbackForFont(font.family_name));
    }
  }

  return result;
}

function releaseLoadedFonts() {
  for (const face of loadedFontFaces) {
    document.fonts.delete(face);
  }
  for (const url of loadedFontUrls) {
    URL.revokeObjectURL(url);
  }
  loadedFontFaces = [];
  loadedFontUrls = [];
}

function mappedFallbackForFont(fontName: string | null): string | null {
  if (!fontName) return null;
  const normalized = fontName.replace(/^[A-Z]{6}\+/, "");
  return fontNameFallbacks.find(([pattern]) => pattern.test(normalized))?.[1] ?? null;
}

function withFallbackFonts(primary: string | null): string {
  const cleanPrimary = primary
    ?.split(",")
    .map((part) => part.trim())
    .filter(Boolean)
    .join(", ");
  return cleanPrimary ? `${cleanPrimary}, ${fallbackFontFamily}` : fallbackFontFamily;
}

function sanitizeCssName(value: string): string {
  return value.replace(/[^a-zA-Z0-9_-]/g, "_") || "font";
}

function quoteCssFamily(value: string): string {
  return JSON.stringify(value);
}

function firstCssFamilyName(value: string): string {
  const [first] = value.split(",");
  return first?.trim().replace(/^['"]|['"]$/g, "") || "sans-serif";
}

/**
 * Returns true for font formats the browser's FontFace API can load.
 * CFF (Type1C / CIDFontType0C) is included because browsers can parse
 * CFF glyph data when it is referenced with the "opentype" CSS hint.
 */
function isBrowserLoadableFont(format: string): boolean {
  return (
    format === "truetype" ||
    format === "opentype" ||
    format === "cff" ||
    format === "woff" ||
    format === "woff2"
  );
}

/**
 * Maps the backend format name to the CSS @font-face format() hint.
 * Pure CFF data uses the same "opentype" hint since browsers treat them
 * equivalently when loading via FontFace.
 */
function cssFontFormat(format: string): string {
  return format === "cff" ? "opentype" : format;
}

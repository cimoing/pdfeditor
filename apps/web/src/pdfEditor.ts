import init, {
  pdf_apply_text_edits,
  pdf_apply_text_edits_by_handle,
  pdf_close_document,
  pdf_open_document,
  pdf_page_bundle,
  pdf_page_bundle_by_handle
} from "./wasm/pdfeditor_core";

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
}

export interface StructuredTextObject {
  id: number;
  bounds: Rect;
  content: string;
  font_name: string | null;
  font_size: number;
  color: { r: number; g: number; b: number; a: number };
  transform: [number, number, number, number, number, number];
  angle_degrees: number;
  z_index: number;
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

export interface EmbeddedFontInfo {
  resource_name: string;
  family_name: string;
  file_name: string;
  mime_type: string;
  format: string;
}

export interface LoadedPage {
  structure: PageStructure;
  backgroundUrl: string;
  fontFamilies: Record<string, string>;
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

const fallbackFontFamily =
  '"Helvetica Neue", Arial, "Segoe UI", "Noto Sans", "Noto Sans CJK SC", "Microsoft YaHei", SimSun, sans-serif';

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
  const structure = metadata.structure;
  const fontFamilies = await loadEmbeddedFonts(metadata.fonts, payload);
  const backgroundUrl = URL.createObjectURL(
    new Blob([assetBlobPart(payload, metadata.background_png)], { type: metadata.background_png.mime_type })
  );

  const imageAssets = new Map(metadata.images.map((image) => [image.id, image.asset]));
  for (const image of structure.images) {
    const asset = imageAssets.get(image.id);
    if (!asset) continue;
    image.objectUrl = URL.createObjectURL(new Blob([assetBlobPart(payload, asset)], { type: asset.mime_type }));
  }

  return { structure, backgroundUrl, fontFamilies };
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

export function asBlobPart(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
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

function assetBlobPart(payload: Uint8Array, asset: BinaryAssetInfo): ArrayBuffer {
  return asBlobPart(payload.subarray(asset.offset, asset.offset + asset.length));
}

export function resolvePdfFontFamily(fontName: string | null, embeddedFonts: Record<string, string>): string {
  if (fontName && embeddedFonts[fontName]) {
    return embeddedFonts[fontName];
  }
  return withFallbackFonts(mappedFallbackForFont(fontName));
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
      const source = `url(${blobUrl}) format("${font.format}")`;
      const face = new FontFace(family, source);
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

function isBrowserLoadableFont(format: string): boolean {
  return format === "truetype" || format === "opentype";
}

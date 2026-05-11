import init, {
  pdf_apply_text_edits,
  pdf_font_asset,
  pdf_image_object_png,
  pdf_page_background_png,
  pdf_page_fonts_to_json,
  pdf_page_to_json
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

export async function loadPdfPage(pdfBytes: Uint8Array, pageNumber: number): Promise<LoadedPage> {
  await ensureWasm();
  releaseLoadedFonts();
  const structure = JSON.parse(pdf_page_to_json(pdfBytes, pageNumber)) as PageStructure;
  const fontFamilies = await loadEmbeddedFonts(pdfBytes, pageNumber);
  const backgroundPng = pdf_page_background_png(pdfBytes, pageNumber);
  const backgroundUrl = URL.createObjectURL(new Blob([asBlobPart(backgroundPng)], { type: "image/png" }));

  for (const image of structure.images) {
    const png = pdf_image_object_png(pdfBytes, pageNumber, BigInt(image.id));
    image.objectUrl = URL.createObjectURL(new Blob([asBlobPart(png)], { type: "image/png" }));
  }

  return { structure, backgroundUrl, fontFamilies };
}

export async function applyTextEdits(
  pdfBytes: Uint8Array,
  edits: Array<{ id: number; content: string }>
): Promise<Uint8Array> {
  await ensureWasm();
  const request = {
    edits: edits.map((edit) => ({
      type: "replace_text",
      id: edit.id,
      content: edit.content
    }))
  };
  return pdf_apply_text_edits(pdfBytes, JSON.stringify(request));
}

export function asBlobPart(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

export function resolvePdfFontFamily(fontName: string | null, embeddedFonts: Record<string, string>): string {
  if (fontName && embeddedFonts[fontName]) {
    return embeddedFonts[fontName];
  }
  return withFallbackFonts(mappedFallbackForFont(fontName));
}

async function loadEmbeddedFonts(pdfBytes: Uint8Array, pageNumber: number): Promise<Record<string, string>> {
  const result: Record<string, string> = {};
  const fonts = JSON.parse(pdf_page_fonts_to_json(pdfBytes, pageNumber)) as EmbeddedFontInfo[];

  for (const font of fonts) {
    let blobUrl: string | null = null;
    try {
      if (!isBrowserLoadableFont(font.format)) {
        result[font.resource_name] = withFallbackFonts(mappedFallbackForFont(font.family_name));
        continue;
      }
      const bytes = pdf_font_asset(pdfBytes, pageNumber, font.resource_name);
      blobUrl = URL.createObjectURL(new Blob([asBlobPart(bytes)], { type: font.mime_type }));
      const family = `PdfEmbedded_${pageNumber}_${sanitizeCssName(font.resource_name)}_${fontLoadSequence++}`;
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

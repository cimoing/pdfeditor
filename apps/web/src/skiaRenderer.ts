import CanvasKitInit, {
  type Canvas,
  type CanvasKit,
  type Font,
  type Image,
  type Paint,
  type Surface,
  type Typeface
} from "canvaskit-wasm";
import canvasKitWasmUrl from "canvaskit-wasm/bin/canvaskit.wasm?url";
import notoSansScChineseSimplifiedUrl from "@fontsource/noto-sans-sc/files/noto-sans-sc-chinese-simplified-400-normal.woff?url";
import notoSansScLatinUrl from "@fontsource/noto-sans-sc/files/noto-sans-sc-latin-400-normal.woff?url";
import type { LayoutGlyph, LoadedFontAsset, StructuredTextObject } from "./pdfEditor";
import { pdfToViewport, type PageViewport, type ViewportRect } from "./viewport";

interface RenderInput {
  canvas: HTMLCanvasElement;
  viewport: PageViewport;
  backgroundUrl: string;
  texts: StructuredTextObject[];
  fonts: LoadedFontAsset[];
  hiddenTextIds?: Set<number>;
}

let canvasKitReady: Promise<CanvasKit> | null = null;

export function ensureCanvasKit(): Promise<CanvasKit> {
  canvasKitReady ??= CanvasKitInit({
    locateFile: () => canvasKitWasmUrl
  });
  return canvasKitReady;
}

export class SkiaPageRenderer {
  private surface: Surface | null = null;
  private surfaceCanvas: HTMLCanvasElement | null = null;
  private surfaceWidth = 0;
  private surfaceHeight = 0;
  private backgroundImage: Image | null = null;
  private backgroundUrl: string | null = null;
  private typefaces = new Map<string, Typeface>();
  private fallbackTypefaces: Typeface[] = [];
  private fallbackLoadStarted = false;
  private defaultTypeface: Typeface | null = null;
  private loggedFontDiagnostics = new Set<string>();

  async render(input: RenderInput): Promise<Map<number, ViewportRect>> {
    const canvasKit = await ensureCanvasKit();
    this.defaultTypeface ??= canvasKit.Typeface.GetDefault();
    this.resizeCanvas(input.canvas, input.viewport);
    const surface = this.ensureSurface(canvasKit, input.canvas);
    const background = await this.loadBackground(canvasKit, input.backgroundUrl);
    this.loadTypefaces(canvasKit, input.fonts);
    await this.loadFallbackTypefaces(canvasKit);

    const skiaCanvas = surface.getCanvas();
    skiaCanvas.clear(canvasKit.TRANSPARENT);
    skiaCanvas.save();
    skiaCanvas.scale(input.viewport.devicePixelRatio, input.viewport.devicePixelRatio);
    this.drawBackground(canvasKit, skiaCanvas, background, input.viewport);
    const measuredTextRects = this.drawStructuredText(
      canvasKit,
      skiaCanvas,
      input.viewport,
      input.texts,
      input.hiddenTextIds ?? new Set()
    );
    skiaCanvas.restore();
    surface.flush();
    return measuredTextRects;
  }

  dispose() {
    this.backgroundImage?.delete();
    this.surface?.delete();
    for (const typeface of this.typefaces.values()) {
      typeface.delete();
    }
    for (const typeface of this.fallbackTypefaces) {
      typeface.delete();
    }
    this.backgroundImage = null;
    this.backgroundUrl = null;
    this.surface = null;
    this.surfaceCanvas = null;
    this.surfaceWidth = 0;
    this.surfaceHeight = 0;
    this.typefaces.clear();
    this.fallbackTypefaces = [];
    this.fallbackLoadStarted = false;
    this.defaultTypeface = null;
  }

  private resizeCanvas(canvas: HTMLCanvasElement, viewport: PageViewport) {
    const width = Math.max(1, Math.ceil(viewport.width * viewport.devicePixelRatio));
    const height = Math.max(1, Math.ceil(viewport.height * viewport.devicePixelRatio));
    if (canvas.width !== width) canvas.width = width;
    if (canvas.height !== height) canvas.height = height;
    canvas.style.width = `${viewport.width}px`;
    canvas.style.height = `${viewport.height}px`;
  }

  private ensureSurface(canvasKit: CanvasKit, canvas: HTMLCanvasElement): Surface {
    if (this.surface && this.surfaceCanvas === canvas && this.surfaceWidth === canvas.width && this.surfaceHeight === canvas.height) {
      return this.surface;
    }
    this.surface?.delete();
    const surface = canvasKit.MakeCanvasSurface(canvas);
    if (!surface) {
      throw new Error("Failed to create CanvasKit surface");
    }
    this.surface = surface;
    this.surfaceCanvas = canvas;
    this.surfaceWidth = canvas.width;
    this.surfaceHeight = canvas.height;
    return surface;
  }

  private async loadBackground(canvasKit: CanvasKit, sourceUrl: string): Promise<Image> {
    if (this.backgroundImage && this.backgroundUrl === sourceUrl) {
      return this.backgroundImage;
    }
    this.backgroundImage?.delete();
    const response = await fetch(sourceUrl);
    const bytes = await response.arrayBuffer();
    const image = canvasKit.MakeImageFromEncoded(bytes);
    if (!image) {
      throw new Error("Failed to decode PDF page bitmap with CanvasKit");
    }
    this.backgroundImage = image;
    this.backgroundUrl = sourceUrl;
    return image;
  }

  private loadTypefaces(canvasKit: CanvasKit, fonts: LoadedFontAsset[]) {
    const nextKeys = new Set(fonts.map((font) => font.resource_name));
    for (const [key, typeface] of this.typefaces) {
      if (!nextKeys.has(key)) {
        typeface.delete();
        this.typefaces.delete(key);
      }
    }

    for (const font of fonts) {
      if (this.typefaces.has(font.resource_name)) continue;
      const typeface = canvasKit.Typeface.MakeTypefaceFromData(font.data.slice(0));
      if (typeface) {
        this.typefaces.set(font.resource_name, typeface);
      }
    }
  }

  private async loadFallbackTypefaces(canvasKit: CanvasKit) {
    if (this.fallbackLoadStarted) return;
    this.fallbackLoadStarted = true;

    const candidates = [notoSansScChineseSimplifiedUrl, notoSansScLatinUrl];

    for (const url of candidates) {
      try {
        const response = await fetch(url);
        if (!response.ok) continue;
        const typeface = canvasKit.Typeface.MakeTypefaceFromData(await response.arrayBuffer());
        if (typeface) {
          this.fallbackTypefaces.push(typeface);
        }
      } catch (error) {
        console.debug(`CanvasKit fallback font unavailable: ${url}`, error);
      }
    }
  }

  private drawBackground(canvasKit: CanvasKit, canvas: Canvas, image: Image, viewport: PageViewport) {
    const paint = new canvasKit.Paint();
    paint.setAntiAlias(true);
    const source = [0, 0, image.width(), image.height()];

    canvas.save();
    switch (viewport.rotation) {
      case 90:
        canvas.translate(viewport.width, 0);
        canvas.rotate(90, 0, 0);
        canvas.drawImageRect(image, source, [0, 0, viewport.pageWidth * viewport.zoom, viewport.pageHeight * viewport.zoom], paint);
        break;
      case 180:
        canvas.translate(viewport.width, viewport.height);
        canvas.rotate(180, 0, 0);
        canvas.drawImageRect(image, source, [0, 0, viewport.pageWidth * viewport.zoom, viewport.pageHeight * viewport.zoom], paint);
        break;
      case 270:
        canvas.translate(0, viewport.height);
        canvas.rotate(-90, 0, 0);
        canvas.drawImageRect(image, source, [0, 0, viewport.pageWidth * viewport.zoom, viewport.pageHeight * viewport.zoom], paint);
        break;
      default:
        canvas.drawImageRect(image, source, [0, 0, viewport.width, viewport.height], paint);
    }
    canvas.restore();
    paint.delete();
  }

  private drawStructuredText(
    canvasKit: CanvasKit,
    canvas: Canvas,
    viewport: PageViewport,
    texts: StructuredTextObject[],
    hiddenTextIds: Set<number>
  ): Map<number, ViewportRect> {
    const paint = new canvasKit.Paint();
    paint.setAntiAlias(true);
    const measuredTextRects = new Map<number, ViewportRect>();

    for (const text of [...texts].sort((left, right) => left.z_index - right.z_index)) {
      if (!text.content) continue;

      const fontSize = Math.max(text.font_size * viewport.zoom, 1);
      const origin = pdfToViewport(viewport, text.transform[4], text.transform[5]);
      const xAxisEnd = pdfToViewport(
        viewport,
        text.transform[4] + text.transform[0],
        text.transform[5] + text.transform[1]
      );
      const scaleX = Math.hypot(xAxisEnd.x - origin.x, xAxisEnd.y - origin.y) / Math.max(fontSize, 1);
      const angle = (Math.atan2(xAxisEnd.y - origin.y, xAxisEnd.x - origin.x) * 180) / Math.PI;
      const lines = text.content.split("\n");
      const lineHeight = fontSize * 1.2;
      const glyphLines = splitGlyphsByLine(text);
      const lineWidths = lines.map((line, index) =>
        this.measureLineWidth(canvasKit, text, fontSize, line, paint, glyphLines?.[index])
      );
      measuredTextRects.set(
        text.id,
        measuredViewportRect(origin, angle, scaleX || 1, Math.max(...lineWidths, 1), fontSize, lineHeight, lines.length)
      );
      if (hiddenTextIds.has(text.id)) continue;

      paint.setColor(canvasKit.Color(text.color.r, text.color.g, text.color.b, text.color.a / 255));
      canvas.save();
      canvas.translate(origin.x, origin.y);
      canvas.rotate(angle, 0, 0);
      canvas.scale(scaleX || 1, 1);
      this.clipToTextBounds(canvasKit, canvas, viewport, text, scaleX || 1, fontSize, lineHeight);

      for (const [index, line] of lines.entries()) {
        this.drawLine(canvasKit, canvas, text, fontSize, line, index * lineHeight, paint, glyphLines?.[index]);
      }
      canvas.restore();
    }

    paint.delete();
    return measuredTextRects;
  }

  private drawLine(
    canvasKit: CanvasKit,
    canvas: Canvas,
    text: StructuredTextObject,
    size: number,
    line: string,
    y: number,
    paint: Paint,
    glyphs?: LayoutGlyph[]
  ) {
    let x = 0;
    const chars = Array.from(line);
    for (const [index, char] of chars.entries()) {
      const glyph = glyphs?.[index];
      const plan = this.resolveGlyphPlan(canvasKit, text, size, char, paint, glyph);
      canvas.drawText(char, x, y, paint, plan.font);
      x += plan.advance;
      plan.font.delete();
    }
  }

  private measureLineWidth(
    canvasKit: CanvasKit,
    text: StructuredTextObject,
    size: number,
    line: string,
    paint: Paint,
    glyphs?: LayoutGlyph[]
  ) {
    let width = 0;
    const chars = Array.from(line);
    for (const [index, char] of chars.entries()) {
      const plan = this.resolveGlyphPlan(canvasKit, text, size, char, paint, glyphs?.[index]);
      width += plan.advance;
      plan.font.delete();
    }
    return width;
  }

  private resolveGlyphPlan(
    canvasKit: CanvasKit,
    text: StructuredTextObject,
    size: number,
    content: string,
    paint: Paint,
    glyph?: LayoutGlyph
  ) {
    const resolved = this.fontForText(canvasKit, text, size, content, glyph);
    const advance =
      resolved.source === "embedded"
        ? glyphAdvance(resolved.font, content, size, paint, glyph?.advance, content)
        : glyphAdvance(resolved.font, content, size, paint);
    if (resolved.source !== "embedded") {
      this.logFontFallback(text, content, resolved.source);
    }
    return { font: resolved.font, advance };
  }

  private fontForText(
    canvasKit: CanvasKit,
    text: StructuredTextObject,
    size: number,
    content: string,
    glyph?: LayoutGlyph
  ) {
    const embedded = text.font_name ? this.typefaces.get(text.font_name) : null;
    if (embedded) {
      const font = new canvasKit.Font(embedded, size);
      if (fontHasGlyphs(font, content)) return { font, source: "embedded" as const };
      font.delete();
    }

    for (const typeface of this.fallbackTypefaces) {
      const font = new canvasKit.Font(typeface, size);
      if (fontHasGlyphs(font, content)) {
        return { font, source: "fallback" as const };
      }
      font.delete();
    }

    return { font: new canvasKit.Font(this.defaultTypeface, size), source: "default" as const };
  }

  private clipToTextBounds(
    canvasKit: CanvasKit,
    canvas: Canvas,
    viewport: PageViewport,
    text: StructuredTextObject,
    scaleX: number,
    fontSize: number,
    lineHeight: number
  ) {
    const clipWidth = Math.max((text.bounds.size.width * viewport.zoom) / Math.max(scaleX, 0.001), 1);
    const clipHeight = Math.max(text.bounds.size.height * viewport.zoom, lineHeight);
    const rect = canvasKit.XYWHRect(-2, -fontSize - 2, clipWidth + 4, clipHeight + 4);
    canvas.clipRect(rect, canvasKit.ClipOp.Intersect, true);
  }

  private logFontFallback(text: StructuredTextObject, content: string, source: "fallback" | "default") {
    const key = `${text.id}:${text.font_name ?? "none"}:${content}:${source}`;
    if (this.loggedFontDiagnostics.has(key)) return;
    this.loggedFontDiagnostics.add(key);
    console.warn(
      `[CanvasKit] ${source} font used for text object ${text.id} (${text.font_name ?? "no-font"}) glyph ${JSON.stringify(content)}`
    );
  }
}

function fontHasGlyphs(font: Font, content: string) {
  if (!content) return true;
  return Array.from(font.getGlyphIDs(content)).every((glyphId) => glyphId !== 0);
}

function glyphAdvance(
  font: Font,
  content: string,
  size: number,
  paint: Paint,
  pdfAdvance?: number,
  originalText?: string
) {
  const useMeasuredWidth = shouldUseMeasuredAdvance(originalText ?? content, pdfAdvance);
  if (!useMeasuredWidth && typeof pdfAdvance === "number" && Number.isFinite(pdfAdvance) && pdfAdvance >= 0) {
    return pdfAdvance * size;
  }
  const glyphs = font.getGlyphIDs(content);
  const widths = font.getGlyphWidths(glyphs, paint);
  const width = Array.from(widths).reduce((sum, item) => sum + item, 0);
  return width > 0 ? width : size * 0.5;
}

function shouldUseMeasuredAdvance(content: string, pdfAdvance?: number) {
  if (!content || typeof pdfAdvance !== "number" || !Number.isFinite(pdfAdvance)) {
    return false;
  }
  // PDF CID metrics for embedded Latin subset fonts in this project often collapse
  // to 1em advances, which makes halfwidth ASCII look like fullwidth text.
  return /^[\x20-\x7E]$/.test(content);
}

function splitGlyphsByLine(text: StructuredTextObject): LayoutGlyph[][] | null {
  if (!text.glyphs?.length) return null;
  const lines = text.content.split("\n").map((line) => Array.from(line));
  const glyphChars = text.glyphs.map((glyph) => glyph.ch);
  const flattenedChars = lines.flat();
  if (flattenedChars.length !== glyphChars.length) return null;
  for (let index = 0; index < flattenedChars.length; index += 1) {
    if (flattenedChars[index] !== glyphChars[index]) {
      return null;
    }
  }

  const result: LayoutGlyph[][] = [];
  let offset = 0;
  for (const line of lines) {
    result.push(text.glyphs.slice(offset, offset + line.length));
    offset += line.length;
  }
  return result;
}

function measuredViewportRect(
  origin: { x: number; y: number },
  angleDegrees: number,
  scaleX: number,
  width: number,
  fontSize: number,
  lineHeight: number,
  lineCount: number
): ViewportRect {
  const angle = (angleDegrees * Math.PI) / 180;
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const top = -fontSize;
  const bottom = Math.max(fontSize * 0.25, (lineCount - 1) * lineHeight + fontSize * 0.25);
  const points = [
    localToViewport(origin, cos, sin, scaleX, 0, top),
    localToViewport(origin, cos, sin, scaleX, width, top),
    localToViewport(origin, cos, sin, scaleX, 0, bottom),
    localToViewport(origin, cos, sin, scaleX, width, bottom)
  ];
  const padding = 2;
  const left = Math.min(...points.map((point) => point.x)) - padding;
  const right = Math.max(...points.map((point) => point.x)) + padding;
  const rectTop = Math.min(...points.map((point) => point.y)) - padding;
  const rectBottom = Math.max(...points.map((point) => point.y)) + padding;
  return { left, top: rectTop, width: right - left, height: rectBottom - rectTop };
}

function localToViewport(
  origin: { x: number; y: number },
  cos: number,
  sin: number,
  scaleX: number,
  x: number,
  y: number
) {
  return {
    x: origin.x + cos * x * scaleX - sin * y,
    y: origin.y + sin * x * scaleX + cos * y
  };
}

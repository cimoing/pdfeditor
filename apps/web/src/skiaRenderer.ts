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
import { pdfToViewport, type PageViewport } from "./viewport";

interface RenderInput {
  canvas: HTMLCanvasElement;
  viewport: PageViewport;
  backgroundUrl: string;
  texts: StructuredTextObject[];
  fonts: LoadedFontAsset[];
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
  private fontInfo = new Map<string, LoadedFontAsset>();
  private fallbackTypefaces: Typeface[] = [];
  private fallbackLoadStarted = false;
  private defaultTypeface: Typeface | null = null;
  private loggedFontDiagnostics = new Set<string>();

  async render(input: RenderInput): Promise<void> {
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
    this.drawStructuredText(
      canvasKit,
      skiaCanvas,
      input.viewport,
      input.texts
    );
    skiaCanvas.restore();
    surface.flush();
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
    this.fontInfo.clear();
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
    for (const key of [...this.fontInfo.keys()]) {
      if (!nextKeys.has(key)) {
        this.fontInfo.delete(key);
      }
    }

    for (const font of fonts) {
      if (!this.fontInfo.has(font.resource_name)) {
        this.fontInfo.set(font.resource_name, font);
      }
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
    texts: StructuredTextObject[]
  ) {
    for (const text of [...texts].sort((left, right) => left.z_index - right.z_index)) {
      if (!text.content) continue;

      const fontSize = Math.max(effectiveFontSize(text) * viewport.zoom, 1);
      const origin = pdfToViewport(viewport, text.transform[4], text.transform[5]);
      const xAxisEnd = pdfToViewport(
        viewport,
        text.transform[4] + text.transform[0],
        text.transform[5] + text.transform[1]
      );
      const scaleX = Math.hypot(xAxisEnd.x - origin.x, xAxisEnd.y - origin.y) / Math.max(fontSize, 1);
      const angle = (Math.atan2(xAxisEnd.y - origin.y, xAxisEnd.x - origin.x) * 180) / Math.PI;
      const lines = text.content.split("\n");
      const glyphLines = splitGlyphsByLine(text);
      const trustGlyphPositions = glyphLines ? shouldTrustGlyphPositions(text, glyphLines) : false;
      const useRunLayout = shouldRenderGroupedRuns(text, lines);
      const paints = createTextPaints(canvasKit, text, viewport.zoom);
      try {
        for (const paint of paints) {
          if (!useRunLayout && glyphLines && trustGlyphPositions) {
            this.drawGlyphLinesAbsolute(canvasKit, canvas, viewport, text, fontSize, angle, paint, glyphLines);
            continue;
          }

          canvas.save();
          canvas.translate(origin.x, origin.y);
          canvas.rotate(angle, 0, 0);
          canvas.scale(scaleX || 1, 1);
          const targetLineWidth = Math.max((text.bounds.size.width * viewport.zoom) / Math.max(scaleX || 1, 0.001), 1);

          if (useRunLayout) {
            this.drawRunsLine(canvasKit, canvas, text, fontSize, 0, paint, targetLineWidth);
          } else {
            for (const [index, line] of lines.entries()) {
              this.drawLine(
                canvasKit,
                canvas,
                text,
                fontSize,
                line,
                index * fontSize * 1.2,
                paint,
                glyphLines?.[index],
                targetLineWidth
              );
            }
          }
          canvas.restore();
        }
      } finally {
        for (const paint of paints) {
          paint.delete();
        }
      }
    }
  }

  private drawGlyphLinesAbsolute(
    canvasKit: CanvasKit,
    canvas: Canvas,
    viewport: PageViewport,
    text: StructuredTextObject,
    size: number,
    angle: number,
    paint: Paint,
    glyphLines: LayoutGlyph[][]
  ) {
    for (const line of glyphLines) {
      for (const glyph of line) {
        const plan = this.resolveGlyphPlan(canvasKit, text, size, glyph.ch, paint, glyph);
        const point = pdfToViewport(viewport, glyph.x, glyph.y);
        canvas.save();
        canvas.translate(point.x, point.y);
        canvas.rotate(angle, 0, 0);
        if (plan.drawScaleX !== 1) {
          canvas.scale(plan.drawScaleX, 1);
        }
        canvas.drawText(glyph.ch, plan.drawOffsetX, 0, paint, plan.font);
        canvas.restore();
        plan.font.delete();
      }
    }
  }

  private drawLine(
    canvasKit: CanvasKit,
    canvas: Canvas,
    text: StructuredTextObject,
    size: number,
    line: string,
    y: number,
    paint: Paint,
    glyphs?: LayoutGlyph[],
    targetWidth?: number
  ) {
    const chars = Array.from(line);
    const plans = chars.map((char, index) => this.resolveGlyphPlan(canvasKit, text, size, char, paint, glyphs?.[index]));
    const lineWidth = plans.reduce((sum, plan) => sum + plan.advance, 0);
    const fitScale =
      targetWidth && lineWidth > 0
        ? Math.min(targetWidth / lineWidth, 1)
        : 1;

    canvas.save();
    if (fitScale !== 1) {
      canvas.scale(fitScale, 1);
    }
    let x = 0;
    for (const [index, plan] of plans.entries()) {
      canvas.save();
      canvas.translate(x, 0);
      if (plan.drawScaleX !== 1) {
        canvas.scale(plan.drawScaleX, 1);
      }
      canvas.drawText(chars[index], plan.drawOffsetX, y, paint, plan.font);
      canvas.restore();
      x += plan.advance;
      plan.font.delete();
    }
    canvas.restore();
  }

  private drawRunsLine(
    canvasKit: CanvasKit,
    canvas: Canvas,
    text: StructuredTextObject,
    size: number,
    y: number,
    paint: Paint,
    targetWidth?: number
  ) {
    const runs = (text.runs ?? []).filter((run) => run.content);
    const plans = runs.map((run) => this.resolveTextPlan(canvasKit, text, size, run.content, paint, run.font_name));
    const lineWidth = plans.reduce((sum, plan) => sum + plan.advance, 0);
    const fitScale =
      targetWidth && lineWidth > 0
        ? Math.min(targetWidth / lineWidth, 1)
        : 1;

    canvas.save();
    if (fitScale !== 1) {
      canvas.scale(fitScale, 1);
    }
    let x = 0;
    for (const [index, plan] of plans.entries()) {
      canvas.drawText(runs[index].content, x, y, paint, plan.font);
      x += plan.advance;
      plan.font.delete();
    }
    canvas.restore();
  }

  private resolveGlyphPlan(
    canvasKit: CanvasKit,
    text: StructuredTextObject,
    size: number,
    content: string,
    paint: Paint,
    glyph?: LayoutGlyph
  ) {
    const fontName = glyph?.font_name ?? text.font_name;
    let resolved = this.fontForText(canvasKit, text, size, content, fontName);
    let measuredWidth = measureGlyphWidth(resolved.font, content, paint);

    // Some embedded CJK fonts map fullwidth Unicode codepoints (U+FF00–FFEF) to
    // narrow ASCII-style glyphs. Detect this by checking measured width and fall
    // through to a fallback font so the character renders at full width.
    if (resolved.source === "embedded" && isFullwidthContent(content) && measuredWidth < size * 0.5) {
      resolved.font.delete();
      resolved = this.fontForText(canvasKit, text, size, content, null);
      measuredWidth = measureGlyphWidth(resolved.font, content, paint);
    }

    const advance =
      resolved.source === "embedded"
        ? glyphAdvance(measuredWidth, size, glyph?.advance, content)
        : measuredWidth > 0
          ? measuredWidth
          : size * 0.5;
    const fit = fitGlyphWidth(advance, measuredWidth);
    if (resolved.source !== "embedded") {
      this.logFontFallback(text, content, resolved.source, resolved.fontName);
    }
    return {
      font: resolved.font,
      advance,
      drawScaleX: fit.scaleX,
      drawOffsetX: fit.offsetX
    };
  }

  private resolveTextPlan(
    canvasKit: CanvasKit,
    text: StructuredTextObject,
    size: number,
    content: string,
    paint: Paint,
    fontName?: string | null
  ) {
    const resolved = this.fontForText(canvasKit, text, size, content, fontName);
    const measuredWidth = measureGlyphWidth(resolved.font, content, paint);
    const advance = measuredWidth > 0 ? measuredWidth : Math.max(size * Array.from(content).length * 0.5, size * 0.5);
    if (resolved.source !== "embedded") {
      this.logFontFallback(text, content, resolved.source, resolved.fontName);
    }
    return { font: resolved.font, advance };
  }

  private fontForText(
    canvasKit: CanvasKit,
    text: StructuredTextObject,
    size: number,
    content: string,
    preferredFontName?: string | null
  ) {
    const embedded = preferredFontName ? this.typefaces.get(preferredFontName) : null;
    if (embedded) {
      const font = new canvasKit.Font(embedded, size);
      configureFont(font, this.isBoldText(text));
      if (fontHasGlyphs(font, content)) return { font, source: "embedded" as const, fontName: preferredFontName };
      font.delete();
    }

    for (const typeface of this.fallbackTypefaces) {
      const font = new canvasKit.Font(typeface, size);
      configureFont(font, this.isBoldText(text));
      if (fontHasGlyphs(font, content)) {
        return { font, source: "fallback" as const, fontName: preferredFontName ?? text.font_name };
      }
      font.delete();
    }

    const font = new canvasKit.Font(this.defaultTypeface, size);
    configureFont(font, this.isBoldText(text));
    return { font, source: "default" as const, fontName: preferredFontName ?? text.font_name };
  }

  private isBoldText(text: StructuredTextObject) {
    const fontInfo = text.font_name ? this.fontInfo.get(text.font_name) : undefined;
    return Boolean(fontInfo?.is_bold || (fontInfo?.font_weight ?? 400) >= 600);
  }

  private logFontFallback(
    text: StructuredTextObject,
    content: string,
    source: "fallback" | "default",
    fontName?: string | null
  ) {
    if (isIgnorableForGlyphCheck(content)) return;
    if (!fontName || !this.typefaces.has(fontName)) return;
    const key = `${text.id}:${fontName}:${content}:${source}`;
    if (this.loggedFontDiagnostics.has(key)) return;
    this.loggedFontDiagnostics.add(key);
    console.warn(
      `[CanvasKit] ${source} font used for text object ${text.id} (${fontName}) glyph ${JSON.stringify(content)}`
    );
  }
}

function fontHasGlyphs(font: Font, content: string) {
  if (!content) return true;
  if (isIgnorableForGlyphCheck(content)) return true;
  const chars = Array.from(content).filter((char) => !isIgnorableForGlyphCheck(char));
  if (chars.length === 0) return true;
  return Array.from(font.getGlyphIDs(chars.join(""))).every((glyphId) => glyphId !== 0);
}

function configureFont(font: Font, embolden: boolean) {
  font.setSubpixel(true);
  font.setEmbolden(embolden);
}

function effectiveFontSize(text: StructuredTextObject) {
  const yAxisScale = Math.hypot(text.transform[2], text.transform[3]);
  if (yAxisScale > 0.01) return yAxisScale;
  const xAxisScale = Math.hypot(text.transform[0], text.transform[1]);
  if (xAxisScale > 0.01) return xAxisScale;
  return text.font_size > 0 ? text.font_size : 1;
}

function createTextPaints(canvasKit: CanvasKit, text: StructuredTextObject, zoom: number) {
  const paints: Paint[] = [];
  if (shouldFillText(text.rendering_mode)) {
    const paint = new canvasKit.Paint();
    paint.setAntiAlias(true);
    paint.setStyle(canvasKit.PaintStyle.Fill);
    paint.setColor(canvasKit.Color(text.color.r, text.color.g, text.color.b, text.color.a / 255));
    paints.push(paint);
  }
  if (shouldStrokeText(text.rendering_mode)) {
    const paint = new canvasKit.Paint();
    paint.setAntiAlias(true);
    paint.setStyle(canvasKit.PaintStyle.Stroke);
    paint.setColor(
      canvasKit.Color(
        text.stroke_color.r,
        text.stroke_color.g,
        text.stroke_color.b,
        text.stroke_color.a / 255
      )
    );
    paint.setStrokeWidth(Math.max(text.stroke_width * zoom, 0.6));
    paints.push(paint);
  }
  if (paints.length === 0) {
    const paint = new canvasKit.Paint();
    paint.setAntiAlias(true);
    paint.setStyle(canvasKit.PaintStyle.Fill);
    paint.setColor(canvasKit.Color(text.color.r, text.color.g, text.color.b, text.color.a / 255));
    paints.push(paint);
  }
  return paints;
}

function shouldFillText(renderingMode: number) {
  return renderingMode === 0 || renderingMode === 2 || renderingMode === 4 || renderingMode === 6;
}

function shouldStrokeText(renderingMode: number) {
  return renderingMode === 1 || renderingMode === 2 || renderingMode === 5 || renderingMode === 6;
}

function isIgnorableForGlyphCheck(content: string) {
  return /^[\s\u00A0\u2000-\u200F\u2028-\u202F\u205F\u3000]+$/u.test(content);
}

function isFullwidthContent(content: string): boolean {
  const cp = content.codePointAt(0) ?? 0;
  // U+3000–303F: CJK Symbols and Punctuation (full-width, e.g. 、。)
  // U+FF00–FFEF: Fullwidth/halfwidth compatibility forms
  return content.length <= 2 && (
    (cp >= 0x3000 && cp <= 0x303F) ||
    (cp >= 0xFF00 && cp <= 0xFFEF)
  );
}

function glyphAdvance(
  measuredWidth: number,
  size: number,
  pdfAdvance?: number,
  originalText?: string
) {
  const useMeasuredWidth = shouldUseMeasuredAdvance(originalText ?? "", pdfAdvance);
  if (!useMeasuredWidth && typeof pdfAdvance === "number" && Number.isFinite(pdfAdvance) && pdfAdvance >= 0) {
    return pdfAdvance * size;
  }
  return measuredWidth > 0 ? measuredWidth : size * 0.5;
}

function measureGlyphWidth(font: Font, content: string, paint: Paint) {
  const glyphs = font.getGlyphIDs(content);
  const widths = font.getGlyphWidths(glyphs, paint);
  return Array.from(widths).reduce((sum, item) => sum + item, 0);
}

function fitGlyphWidth(expectedWidth: number, measuredWidth: number) {
  if (!(expectedWidth > 0) || !(measuredWidth > 0)) {
    return { scaleX: 1, offsetX: 0 };
  }
  if (measuredWidth > expectedWidth) {
    return { scaleX: expectedWidth / measuredWidth, offsetX: 0 };
  }
  if (Math.abs(measuredWidth - expectedWidth) < 0.01) {
    return { scaleX: 1, offsetX: 0 };
  }
  return { scaleX: 1, offsetX: (expectedWidth - measuredWidth) / 2 };
}

function shouldUseMeasuredAdvance(content: string, pdfAdvance?: number) {
  if (!content || typeof pdfAdvance !== "number" || !Number.isFinite(pdfAdvance)) {
    return false;
  }
  // PDF CID metrics in this project often collapse ASCII and punctuation
  // to coarse 1em advances, which makes halfwidth text too loose and
  // can push punctuation into neighboring objects.
  return isAsciiSingleChar(content) || isSinglePunctuation(content);
}

function isAsciiSingleChar(content: string) {
  return /^[\x20-\x7E]$/.test(content);
}

function isSinglePunctuation(content: string) {
  // U+3000-303F (CJK Symbols like \u3001\u3002) are excluded even though \p{P} covers them:
  // these are genuinely full-width characters whose PDF advances (1em) are correct.
  const cp = content.codePointAt(0) ?? 0;
  if (cp >= 0x3000 && cp <= 0x303F) return false;
  return /^[\p{P}\uFF00-\uFF65]$/u.test(content);
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

function shouldTrustGlyphPositions(text: StructuredTextObject, glyphLines: LayoutGlyph[][]) {
  const glyphs = glyphLines.flat();
  if (glyphs.length <= 1) {
    return true;
  }
  if ((text.runs?.length ?? 0) > 1) {
    return true;
  }
  const fontNames = new Set(glyphs.map((glyph) => glyph.font_name ?? ""));
  if (fontNames.size > 1) {
    return true;
  }
  for (let index = 1; index < glyphs.length; index += 1) {
    const previous = glyphs[index - 1];
    const current = glyphs[index];
    if (Math.abs((current.x - previous.x) - previous.advance) > 0.05) {
      return true;
    }
  }
  const unitAdvanceCount = glyphs.filter((glyph) => Math.abs(glyph.advance - 1) < 0.001).length;
  return unitAdvanceCount / glyphs.length < 0.8;
}

function shouldRenderGroupedRuns(text: StructuredTextObject, lines: string[]) {
  return (text.runs?.length ?? 0) > 1 && lines.length === 1;
}


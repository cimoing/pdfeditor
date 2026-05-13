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
        this.fontInfo.delete(key);
      }
    }

    for (const font of fonts) {
      this.fontInfo.set(font.resource_name, font);
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
      const glyphLines = splitGlyphsByLine(text);
      const trustGlyphPositions = glyphLines ? shouldTrustGlyphPositions(glyphLines) : false;
      const paints = createTextPaints(canvasKit, text, viewport.zoom);
      try {
        for (const paint of paints) {
          if (glyphLines && trustGlyphPositions) {
            this.drawGlyphLinesAbsolute(canvasKit, canvas, viewport, text, fontSize, angle, paint, glyphLines);
            continue;
          }

          canvas.save();
          canvas.translate(origin.x, origin.y);
          canvas.rotate(angle, 0, 0);
          canvas.scale(scaleX || 1, 1);
          const targetLineWidth = Math.max((text.bounds.size.width * viewport.zoom) / Math.max(scaleX || 1, 0.001), 1);

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

  private resolveGlyphPlan(
    canvasKit: CanvasKit,
    text: StructuredTextObject,
    size: number,
    content: string,
    paint: Paint,
    glyph?: LayoutGlyph
  ) {
    const resolved = this.fontForText(canvasKit, text, size, content);
    const measuredWidth = measureGlyphWidth(resolved.font, content, paint);
    const advance =
      resolved.source === "embedded"
        ? glyphAdvance(measuredWidth, size, glyph?.advance, content)
        : measuredWidth > 0
          ? measuredWidth
          : size * 0.5;
    const fit = fitGlyphWidth(advance, measuredWidth);
    if (resolved.source !== "embedded") {
      this.logFontFallback(text, content, resolved.source);
    }
    return {
      font: resolved.font,
      advance,
      drawScaleX: fit.scaleX,
      drawOffsetX: fit.offsetX
    };
  }

  private fontForText(
    canvasKit: CanvasKit,
    text: StructuredTextObject,
    size: number,
    content: string
  ) {
    const embedded = text.font_name ? this.typefaces.get(text.font_name) : null;
    if (embedded) {
      const font = new canvasKit.Font(embedded, size);
      configureFont(font, this.isBoldText(text));
      if (fontHasGlyphs(font, content)) return { font, source: "embedded" as const };
      font.delete();
    }

    for (const typeface of this.fallbackTypefaces) {
      const font = new canvasKit.Font(typeface, size);
      configureFont(font, this.isBoldText(text));
      if (fontHasGlyphs(font, content)) {
        return { font, source: "fallback" as const };
      }
      font.delete();
    }

    const font = new canvasKit.Font(this.defaultTypeface, size);
    configureFont(font, this.isBoldText(text));
    return { font, source: "default" as const };
  }

  private isBoldText(text: StructuredTextObject) {
    const fontInfo = text.font_name ? this.fontInfo.get(text.font_name) : undefined;
    return Boolean(fontInfo?.is_bold || (fontInfo?.font_weight ?? 400) >= 600);
  }

  private logFontFallback(text: StructuredTextObject, content: string, source: "fallback" | "default") {
    if (isIgnorableForGlyphCheck(content)) return;
    if (!text.font_name || !this.fontInfo.has(text.font_name)) return;
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
  if (isIgnorableForGlyphCheck(content)) return true;
  const chars = Array.from(content).filter((char) => !isIgnorableForGlyphCheck(char));
  if (chars.length === 0) return true;
  return Array.from(font.getGlyphIDs(chars.join(""))).every((glyphId) => glyphId !== 0);
}

function configureFont(font: Font, embolden: boolean) {
  font.setSubpixel(true);
  font.setEmbolden(embolden);
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
  return /^[\p{P}\u3000-\u303F\uFF00-\uFF65]$/u.test(content);
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

function shouldTrustGlyphPositions(glyphLines: LayoutGlyph[][]) {
  const glyphs = glyphLines.flat();
  if (glyphs.length <= 1) {
    return true;
  }
  const unitAdvanceCount = glyphs.filter((glyph) => Math.abs(glyph.advance - 1) < 0.001).length;
  return unitAdvanceCount / glyphs.length < 0.8;
}


import type { Rect } from "./pdfEditor";

export type Rotation = 0 | 90 | 180 | 270;

export interface PageViewportInput {
  pageIndex: number;
  pageWidth: number;
  pageHeight: number;
  zoom: number;
  rotation?: number;
  offsetX?: number;
  offsetY?: number;
  devicePixelRatio?: number;
}

export interface PageViewport {
  pageIndex: number;
  pageWidth: number;
  pageHeight: number;
  width: number;
  height: number;
  zoom: number;
  rotation: Rotation;
  offsetX: number;
  offsetY: number;
  devicePixelRatio: number;
  transform: Matrix2D;
  inverseTransform: Matrix2D;
}

export type Matrix2D = [number, number, number, number, number, number];

export interface ViewportPoint {
  x: number;
  y: number;
}

export interface ViewportRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export function composePageTransform(input: PageViewportInput): PageViewport {
  const zoom = input.zoom > 0 ? input.zoom : 1;
  const rotation = normalizeRotation(input.rotation ?? 0);
  const offsetX = input.offsetX ?? 0;
  const offsetY = input.offsetY ?? 0;
  const pageWidth = input.pageWidth;
  const pageHeight = input.pageHeight;
  const rotated = rotation === 90 || rotation === 270;
  const width = (rotated ? pageHeight : pageWidth) * zoom;
  const height = (rotated ? pageWidth : pageHeight) * zoom;
  const transform = pageToViewportMatrix(pageWidth, pageHeight, zoom, rotation, offsetX, offsetY);

  return {
    pageIndex: input.pageIndex,
    pageWidth,
    pageHeight,
    width,
    height,
    zoom,
    rotation,
    offsetX,
    offsetY,
    devicePixelRatio: input.devicePixelRatio ?? window.devicePixelRatio ?? 1,
    transform,
    inverseTransform: invertTransform(transform)
  };
}

export function pdfToViewport(viewport: PageViewport, x: number, y: number): ViewportPoint {
  return applyTransform(viewport.transform, x, y);
}

export function viewportToPdf(viewport: PageViewport, x: number, y: number): ViewportPoint {
  return applyTransform(viewport.inverseTransform, x, y);
}

export function pdfRectToViewportRect(viewport: PageViewport, rect: Rect): ViewportRect {
  const x0 = rect.origin.x;
  const y0 = rect.origin.y;
  const x1 = x0 + rect.size.width;
  const y1 = y0 + rect.size.height;
  const points = [
    pdfToViewport(viewport, x0, y0),
    pdfToViewport(viewport, x1, y0),
    pdfToViewport(viewport, x0, y1),
    pdfToViewport(viewport, x1, y1)
  ];
  const left = Math.min(...points.map((point) => point.x));
  const right = Math.max(...points.map((point) => point.x));
  const top = Math.min(...points.map((point) => point.y));
  const bottom = Math.max(...points.map((point) => point.y));
  return { left, top, width: right - left, height: bottom - top };
}

export function viewportRectToPdfRect(viewport: PageViewport, rect: ViewportRect): Rect {
  const x0 = rect.left;
  const y0 = rect.top;
  const x1 = x0 + rect.width;
  const y1 = y0 + rect.height;
  const points = [
    viewportToPdf(viewport, x0, y0),
    viewportToPdf(viewport, x1, y0),
    viewportToPdf(viewport, x0, y1),
    viewportToPdf(viewport, x1, y1)
  ];
  const left = Math.min(...points.map((point) => point.x));
  const right = Math.max(...points.map((point) => point.x));
  const bottom = Math.min(...points.map((point) => point.y));
  const top = Math.max(...points.map((point) => point.y));
  return {
    origin: { x: left, y: bottom },
    size: { width: right - left, height: top - bottom }
  };
}

export function invertTransform(matrix: Matrix2D): Matrix2D {
  const [a, b, c, d, e, f] = matrix;
  const determinant = a * d - b * c;
  if (Math.abs(determinant) < Number.EPSILON) {
    throw new Error("Cannot invert a singular page transform");
  }
  return [
    d / determinant,
    -b / determinant,
    -c / determinant,
    a / determinant,
    (c * f - d * e) / determinant,
    (b * e - a * f) / determinant
  ];
}

export function canvasBitmapSize(viewport: PageViewport): { width: number; height: number } {
  return {
    width: Math.max(1, Math.ceil(viewport.width * viewport.devicePixelRatio)),
    height: Math.max(1, Math.ceil(viewport.height * viewport.devicePixelRatio))
  };
}

export function normalizeRotation(rotation: number): Rotation {
  const normalized = ((rotation % 360) + 360) % 360;
  if (normalized === 90 || normalized === 180 || normalized === 270) return normalized;
  return 0;
}

function pageToViewportMatrix(
  pageWidth: number,
  pageHeight: number,
  zoom: number,
  rotation: Rotation,
  offsetX: number,
  offsetY: number
): Matrix2D {
  switch (rotation) {
    case 90:
      return [0, zoom, zoom, 0, offsetX, offsetY];
    case 180:
      return [-zoom, 0, 0, zoom, offsetX + pageWidth * zoom, offsetY];
    case 270:
      return [0, -zoom, -zoom, 0, offsetX + pageHeight * zoom, offsetY + pageWidth * zoom];
    case 0:
    default:
      return [zoom, 0, 0, -zoom, offsetX, offsetY + pageHeight * zoom];
  }
}

function applyTransform(matrix: Matrix2D, x: number, y: number): ViewportPoint {
  return {
    x: matrix[0] * x + matrix[2] * y + matrix[4],
    y: matrix[1] * x + matrix[3] * y + matrix[5]
  };
}

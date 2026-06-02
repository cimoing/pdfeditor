# 后端 wasm 使用说明

本文档说明 `pdfeditor-core` 编译为 WebAssembly 后暴露给 Web 端的接口、入参、出参和 JSON 结构。

## 构建与导入

构建 wasm：

```bash
cd apps/web
npm run wasm:build
```

浏览器侧导入：

```ts
import init, {
  pdf_open_document,
  pdf_page_bundle_by_handle,
  pdf_update_text_runs_by_handle,
  pdf_get_bytes_by_handle,
  pdf_close_document
} from "./wasm/pdfeditor_core";

await init();
```

所有 wasm API 失败时都会抛出 `JsValue`，当前内容为字符串错误信息。

## 基础约定

- `pdf_bytes: Uint8Array`：完整 PDF 文件 bytes。
- `page_number: number`：对外入参从 `1` 开始；传 `0` 会报错。
- `PageIndex`：JSON 结构中的页索引从 `0` 开始。
- `object_id: u64`：PDF 对象 ID 在 wasm 入参中使用 `BigInt(objectId)` 更稳妥；JSON 中序列化为数字。
- 坐标单位：页面结构中的坐标为 PDF 点，原点在 PDF 页面坐标系中，页面渲染到屏幕时需要前端做 viewport 转换。
- 颜色：后端结构使用 `{ r, g, b, a }`，编辑入参 `TextRunInput.color` 使用 `[r, g, b, a]`。

## 推荐调用流程

```ts
const fileBytes = new Uint8Array(await file.arrayBuffer());
const handle = pdf_open_document(fileBytes);

try {
  const bundleBytes = pdf_page_bundle_by_handle(handle, 1);
  const { metadata, payload } = parsePageBundle(bundleBytes);

  const text = metadata.structure.text[0];
  pdf_update_text_runs_by_handle(
    handle,
    BigInt(text.id),
    JSON.stringify([
      {
        content: "新的文本",
        font_name: text.font_name,
        font_size: text.font_size,
        color: [text.color.r, text.color.g, text.color.b, text.color.a]
      }
    ]),
    0,
    0,
    "",
    JSON.stringify({
      replace_spaces_with_displacements: false,
      digit_font_name: null,
      compress_punctuation: false
    })
  );

  const updatedPdfBytes = pdf_get_bytes_by_handle(handle);
} finally {
  pdf_close_document(handle);
}
```

`parsePageBundle` 见“页面加载包结构”。

## wasm API

### 文档生命周期

| 函数 | 入参 | 出参 | 说明 |
| --- | --- | --- | --- |
| `pdf_open_document(pdf_bytes)` | `Uint8Array` | `number` | 打开 PDF，返回内存文档 handle。 |
| `pdf_close_document(handle)` | `number` | `void` | 释放 handle 对应的内存文档。 |
| `pdf_get_bytes_by_handle(handle)` | `number` | `Uint8Array` | 将 handle 中的当前文档序列化为 PDF bytes，用于下载或导出。 |

### 页面读取

| 函数 | 入参 | 出参 | 说明 |
| --- | --- | --- | --- |
| `pdf_page_to_json(pdf_bytes, page_number)` | `Uint8Array`, `number` | `string` | 返回 `PageStructure` JSON 字符串，不包含背景 PNG、图片 PNG 和字体 bytes。 |
| `pdf_page_structure_by_handle(handle, page_number)` | `number`, `number` | `string` | 返回 `PageStructure` JSON 字符串；适合编辑后刷新结构。 |
| `pdf_page_bundle(pdf_bytes, page_number)` | `Uint8Array`, `number` | `Uint8Array` | 返回页面加载包，包含结构 JSON、背景 PNG、图片 PNG、字体资源。 |
| `pdf_page_bundle_by_handle(handle, page_number)` | `number`, `number` | `Uint8Array` | handle 版本页面加载包，避免重复打开 PDF。 |
| `pdf_page_background_png(pdf_bytes, page_number)` | `Uint8Array`, `number` | `Uint8Array` | 返回页面背景 PNG bytes。 |
| `pdf_image_object_png(pdf_bytes, page_number, image_object_id)` | `Uint8Array`, `number`, `u64` | `Uint8Array` | 返回指定图片对象的 PNG bytes。 |

### 命中测试与编辑预览

| 函数 | 入参 | 出参 | 说明 |
| --- | --- | --- | --- |
| `pdf_hit_test(pdf_bytes, page_number, pdf_x, pdf_y)` | `Uint8Array`, `number`, `number`, `number` | `string` | 返回 `HitTestResult | null` JSON。 |
| `pdf_hit_test_by_handle(handle, page_number, pdf_x, pdf_y)` | `number`, `number`, `number`, `number` | `string` | handle 版本命中测试。 |
| `pdf_start_text_edit(pdf_bytes, object_id)` | `Uint8Array`, `u64` | `string` | 返回 `TextEditSessionInfo` JSON。 |
| `pdf_start_text_edit_by_handle(handle, object_id)` | `number`, `u64` | `string` | handle 版本开始编辑。 |
| `pdf_preview_text_layout(pdf_bytes, object_id, text)` | `Uint8Array`, `u64`, `string` | `string` | 返回 `TextLayoutPreview` JSON，不修改 PDF。 |
| `pdf_preview_text_layout_by_handle(handle, object_id, text)` | `number`, `u64`, `string` | `string` | handle 版本预览。 |

### 文本保存

| 函数 | 入参 | 出参 | 说明 |
| --- | --- | --- | --- |
| `pdf_commit_text_edit(pdf_bytes, object_id, text)` | `Uint8Array`, `u64`, `string` | `Uint8Array` | 替换单个文本对象内容并返回新 PDF bytes。 |
| `pdf_commit_text_edit_by_handle(handle, object_id, text)` | `number`, `u64`, `string` | `Uint8Array` | handle 版本；会修改内存文档并返回新 PDF bytes。 |
| `pdf_update_text_by_handle(handle, object_id, text)` | `number`, `u64`, `string` | `void` | 修改内存文档，不立即序列化。 |
| `pdf_apply_text_edits(pdf_bytes, edits_json)` | `Uint8Array`, `string` | `Uint8Array` | 批量编辑并返回新 PDF bytes；`edits_json` 为 `TextEditRequest`。 |
| `pdf_apply_text_edits_by_handle(handle, edits_json)` | `number`, `string` | `Uint8Array` | handle 版本批量编辑。 |
| `pdf_update_text_runs_by_handle(handle, object_id, runs_json, origin_dx, origin_dy, clip_bounds_json, typography_json)` | 见下方 | `void` | 用多个富文本 run 替换文本对象，并可移动起点、设置裁剪和排版参数。 |

`pdf_update_text_runs_by_handle` 入参说明：

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `handle` | `number` | 是 | `pdf_open_document` 返回的文档句柄。 |
| `object_id` | `u64` | 是 | 被编辑的文本对象 ID。JS 调用时建议传 `BigInt(id)`。 |
| `runs_json` | `string` | 是 | `TextRunInput[]` 的 JSON 字符串。 |
| `origin_dx` | `number` | 是 | 文本起点横向偏移，单位为 PDF 点。 |
| `origin_dy` | `number` | 是 | 文本起点纵向偏移，单位为 PDF 点。 |
| `clip_bounds_json` | `string` | 否 | 为空字符串表示不设置裁剪；非空时为 `Rect` JSON。 |
| `typography_json` | `string` | 否 | 为空字符串表示默认排版参数；非空时为 `TextTypography` JSON。 |

### 字体资源

| 函数 | 入参 | 出参 | 说明 |
| --- | --- | --- | --- |
| `pdf_page_fonts_to_json(pdf_bytes, page_number)` | `Uint8Array`, `number` | `string` | 返回当前页字体资源元数据数组 `FontAssetInfo[]`。 |
| `pdf_font_asset(pdf_bytes, page_number, resource_name)` | `Uint8Array`, `number`, `string` | `Uint8Array` | 返回指定 PDF 字体资源 bytes。 |
| `pdf_set_cjk_font_by_handle(handle, woff_bytes)` | `number`, `Uint8Array` | `boolean` | 注册 CJK fallback 字体。当前要求 WOFF1，成功后编辑时可嵌入，避免中文方块。 |
| `pdf_set_local_font_by_handle(handle, font_key, font_bytes)` | `number`, `string`, `Uint8Array` | `boolean` | 注册系统/本地字体用于后续嵌入。当前接受 TrueType-flavoured SFNT、WOFF1、TTC；CFF/OpenType 会返回 `false`。 |

`font_key` 是前端资源名去掉 `__localfont__:` 前缀后的值。保存 run 时如果 `font_name` 使用 `__localfont__:<key>`，前端应先调用 `pdf_set_local_font_by_handle(handle, key, fontBytes)`。

## 页面加载包结构

`pdf_page_bundle` 和 `pdf_page_bundle_by_handle` 返回二进制包：

```text
4 bytes: metadata JSON 长度，big-endian u32
N bytes: PageBundleInfo JSON
remaining bytes: payload 二进制区
```

解析示例：

```ts
function parsePageBundle(bytes: Uint8Array) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const jsonLength = view.getUint32(0, false);
  const jsonStart = 4;
  const jsonEnd = jsonStart + jsonLength;
  const metadata = JSON.parse(new TextDecoder().decode(bytes.subarray(jsonStart, jsonEnd)));
  const payload = bytes.subarray(jsonEnd);
  return { metadata, payload };
}

function assetBytes(payload: Uint8Array, asset: BinaryAssetInfo) {
  return payload.subarray(asset.offset, asset.offset + asset.length);
}
```

### PageBundleInfo

```ts
interface PageBundleInfo {
  structure: PageStructure;
  background_png: BinaryAssetInfo;
  images: ImageAssetInfo[];
  fonts: FontAssetBundleInfo[];
}
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `structure` | `PageStructure` | 页面结构。 |
| `background_png` | `BinaryAssetInfo` | 背景 PNG 在 payload 中的位置。 |
| `images` | `ImageAssetInfo[]` | 页面图片对象的 PNG 资源。 |
| `fonts` | `FontAssetBundleInfo[]` | 页面字体资源。 |

### BinaryAssetInfo

```ts
interface BinaryAssetInfo {
  file_name: string;
  mime_type: string;
  offset: number;
  length: number;
}
```

| 字段 | 说明 |
| --- | --- |
| `file_name` | 建议文件名。 |
| `mime_type` | MIME 类型，例如 `image/png`、`font/ttf`。 |
| `offset` | 资源在 payload 中的起始偏移。 |
| `length` | 资源字节长度。 |

### ImageAssetInfo

```ts
interface ImageAssetInfo {
  id: number;
  file_name: string;
  width_px: number;
  height_px: number;
  asset: BinaryAssetInfo;
}
```

### FontAssetInfo / FontAssetBundleInfo

```ts
interface FontAssetInfo {
  resource_name: string;
  family_name: string;
  font_weight: number;
  is_bold: boolean;
  file_name: string;
  mime_type: string;
  format: string;
}

interface FontAssetBundleInfo extends FontAssetInfo {
  asset: BinaryAssetInfo;
}
```

| 字段 | 说明 |
| --- | --- |
| `resource_name` | PDF 页面资源中的字体名，文本对象的 `font_name` 会引用它。 |
| `family_name` | 字体家族名。 |
| `font_weight` | 字重，通常为 `400`、`700` 等。 |
| `is_bold` | 是否识别为粗体。 |
| `file_name` | 字体文件名。 |
| `mime_type` | 字体 MIME 类型。 |
| `format` | 字体格式，例如 `truetype`、`opentype`、`cff`、`woff`。 |
| `asset` | 仅 `FontAssetBundleInfo` 有，指向 payload 中字体 bytes。 |

## JSON 结构体

### 基础类型

```ts
type PageIndex = number;      // JSON 中从 0 开始
type PdfObjectId = number;    // u64 序列化为 number
type TextObjectId = number;
type ImageObjectId = number;

interface Point {
  x: number;
  y: number;
}

interface Size {
  width: number;
  height: number;
}

interface Rect {
  origin: Point;
  size: Size;
}

interface Color {
  r: number;
  g: number;
  b: number;
  a: number;
}
```

### PageStructure

```ts
interface PageStructure {
  page: PageInfo;
  text: StructuredTextObject[];
  visual_text: StructuredVisualTextObject[];
  images: StructuredImageObject[];
  watermarks: StructuredWatermark[];
  annotations: StructuredAnnotation[];
  bookmarks: BookmarkItem[];
}
```

| 字段 | 说明 |
| --- | --- |
| `page` | 页基础信息。 |
| `text` | 可编辑文本对象。 |
| `visual_text` | 视觉文本对象，通常用于辅助绘制或不可完整编辑文本。 |
| `images` | 页面图片对象。 |
| `watermarks` | 水印对象。 |
| `annotations` | 注释对象。 |
| `bookmarks` | 书签信息。 |

### PageInfo

```ts
interface PageInfo {
  index: PageIndex;
  size: Size;
  rotation: number;
}
```

| 字段 | 说明 |
| --- | --- |
| `index` | 0-based 页索引。 |
| `size` | 页面尺寸，单位 PDF 点。 |
| `rotation` | 页面旋转角度，单位度。 |

### StructuredTextObject

```ts
interface StructuredTextObject {
  id: TextObjectId;
  bounds: Rect;
  content: string;
  font_name: string | null;
  font_size: number;
  color: Color;
  stroke_color: Color;
  stroke_width: number;
  rendering_mode: number;
  char_spacing: number;
  word_spacing: number;
  horizontal_scaling: number;
  transform: [number, number, number, number, number, number];
  angle_degrees: number;
  z_index: number;
  glyphs: LayoutGlyph[];
  runs: TextRun[];
  punct_width_squeeze?: boolean;
  font_features?: string[];
  clip_bounds?: Rect;
  typography: TextTypography;
}
```

| 字段 | 说明 |
| --- | --- |
| `id` | 文本对象 ID，后续编辑接口使用该值。 |
| `bounds` | 文本对象边界框。 |
| `content` | 合并后的文本内容。 |
| `font_name` | PDF 字体资源名，可能为空。 |
| `font_size` | 字号。 |
| `color` | 填充颜色。 |
| `stroke_color` | 描边颜色。 |
| `stroke_width` | 描边宽度。 |
| `rendering_mode` | PDF 文本渲染模式。 |
| `char_spacing` | 字符间距。 |
| `word_spacing` | 单词间距。 |
| `horizontal_scaling` | 水平缩放百分比，默认 `100`。 |
| `transform` | 文本矩阵 `[a, b, c, d, e, f]`。 |
| `angle_degrees` | 文本旋转角度。 |
| `z_index` | 页面绘制顺序。 |
| `glyphs` | 字形级布局信息。 |
| `runs` | 富文本 run 列表。 |
| `punct_width_squeeze` | 字体本身是否定义了较窄的全角标点宽度。为 `false` 时可能省略。 |
| `font_features` | 检测到的 OpenType 特性，可能包含 `palt`、`halt`、`kern`、`liga`、`fwid`、`hwid`。为空时可能省略。 |
| `clip_bounds` | PDF 内容流中识别出的裁剪矩形；没有裁剪时省略。 |
| `typography` | 排版识别和保存参数。 |

### TextRun

```ts
interface TextRun {
  content: string;
  font_name: string | null;
  font_size: number;
  color: Color;
}
```

### LayoutGlyph

```ts
interface LayoutGlyph {
  ch: string;
  glyph_id: number | null;
  font_name: string | null;
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
```

| 字段 | 说明 |
| --- | --- |
| `ch` | 字符内容。 |
| `glyph_id` | 字体 glyph ID；未知时为 `null`。 |
| `font_name` | 该字形使用的字体资源名。 |
| `x` / `y` | 字形起始位置。 |
| `advance` | 字形推进宽度，排版时应优先使用。 |
| `width` | 字形可见宽度估算。 |
| `bbox` | 字形边界框。 |
| `svg_*` | Type3 字体等场景下提取的 SVG 路径信息。 |

### TextTypography

```ts
interface TextTypography {
  replace_spaces_with_displacements: boolean;
  digit_font_name?: string | null;
  compress_punctuation: boolean;
  detected_tj_displacements: boolean;
  detected_space_displacements: boolean;
  detected_punctuation: boolean;
  detected_digit_font_name?: string | null;
}
```

| 字段 | 说明 |
| --- | --- |
| `replace_spaces_with_displacements` | 保存时是否把普通空格转成 `TJ` 位移。 |
| `digit_font_name` | 保存时数字使用的字体资源名；为空则沿用 run 字体。 |
| `compress_punctuation` | 保存时是否对 CJK/全角标点应用 `TJ` 压缩。 |
| `detected_tj_displacements` | 原 PDF 是否使用过 `TJ` 数值位移。 |
| `detected_space_displacements` | 原 PDF 是否检测到疑似通过位移表达空格。 |
| `detected_punctuation` | 原 PDF 是否检测到适合压缩处理的标点排版。 |
| `detected_digit_font_name` | 原 PDF 是否检测到 ASCII 数字使用了独立字体。 |

前三个字段通常由前端作为保存参数传回后端；后四个字段主要是后端识别结果，供前端默认值和提示使用。

### StructuredVisualTextObject

```ts
interface StructuredVisualTextObject {
  id: TextObjectId;
  bounds: Rect;
  font_name: string | null;
  font_size: number;
  transform: [number, number, number, number, number, number];
  angle_degrees: number;
  z_index: number;
}
```

### StructuredImageObject

```ts
interface StructuredImageObject {
  id: ImageObjectId;
  name: string | null;
  source_file: string | null;
  bounds: Rect;
  transform: [number, number, number, number, number, number];
  angle_degrees: number;
  width_px: number | null;
  height_px: number | null;
  color_space: string | null;
  bits_per_component: number | null;
  filters: string[];
  byte_len: number;
  z_index: number;
}
```

### StructuredAnnotation

```ts
interface StructuredAnnotation {
  id: PdfObjectId | null;
  subtype: string | null;
  bounds: Rect | null;
  contents: string | null;
  name: string | null;
  flags: number | null;
}
```

### StructuredWatermark

```ts
interface StructuredWatermark {
  kind: string;
  object_id: PdfObjectId;
  bounds: Rect;
  content: string | null;
  source: string;
}
```

### BookmarkItem

```ts
interface BookmarkItem {
  title: string;
  page: PageIndex | null;
  level: number;
}
```

## 命中测试与编辑结构

### HitTestResult

`pdf_hit_test*` 返回 `HitTestResult | null`。

```ts
interface HitTestResult {
  object_id: PdfObjectId;
  object_type: string;
  page: PageIndex;
  local_position: Point;
  text_run_index: number | null;
  glyph_index: number | null;
  bbox: Rect;
  matrix: [number, number, number, number, number, number];
}
```

| 字段 | 说明 |
| --- | --- |
| `object_id` | 命中的对象 ID。 |
| `object_type` | 对象类型，例如 `text`、`image`。 |
| `page` | 0-based 页索引。 |
| `local_position` | 命中点在对象局部坐标中的位置。 |
| `text_run_index` | 命中的文本 run 索引；非文本或未知时为 `null`。 |
| `glyph_index` | 命中的 glyph 索引；未知时为 `null`。 |
| `bbox` | 命中对象边界框。 |
| `matrix` | 对象矩阵。 |

### TextEditSessionInfo

```ts
interface TextEditSessionInfo {
  object_id: TextObjectId;
  page: PageIndex;
  original_text: string;
  group_object_ids: TextObjectId[];
  bbox: Rect;
  matrix: [number, number, number, number, number, number];
  font_id: string | null;
  font_size: number;
  writing_mode: string | null;
  glyphs: LayoutGlyph[];
  typography: TextTypography;
}
```

| 字段 | 说明 |
| --- | --- |
| `object_id` | 编辑目标文本对象 ID。 |
| `page` | 0-based 页索引。 |
| `original_text` | 原始文本内容。 |
| `group_object_ids` | 后端识别出的连续文本组 ID；保存时可能影响同组对象清理与重排。 |
| `bbox` | 编辑区域边界框。 |
| `matrix` | 文本矩阵。 |
| `font_id` | 原字体资源名。 |
| `font_size` | 原字号。 |
| `writing_mode` | 书写模式；当前通常为 `null`。 |
| `glyphs` | 原始字形布局。 |
| `typography` | 后端识别出的排版信息。 |

### TextLayoutPreview

```ts
interface TextLayoutPreview {
  object_id: TextObjectId;
  text: string;
  group_object_ids: TextObjectId[];
  glyphs: LayoutGlyph[];
  bbox: Rect;
  overflow: boolean;
  typography: TextTypography;
}
```

| 字段 | 说明 |
| --- | --- |
| `object_id` | 预览目标对象。 |
| `text` | 预览文本。 |
| `group_object_ids` | 文本组 ID。 |
| `glyphs` | 预览布局字形。 |
| `bbox` | 预览边界框。 |
| `overflow` | 是否超出原对象区域。 |
| `typography` | 预览使用或继承的排版信息。 |

## 编辑入参结构

### TextEditRequest

`pdf_apply_text_edits*` 的 `edits_json`：

```ts
interface TextEditRequest {
  edits: TextEdit[];
}
```

### TextEdit

```ts
interface TextEdit {
  type: "replace_text" | "update_text" | "replace_runs";
  id: TextObjectId;
  content?: string;
  runs?: TextRunInput[];
}
```

| 字段 | 说明 |
| --- | --- |
| `type` | `replace_text` / `update_text` 使用 `content`；`replace_runs` 使用 `runs`。 |
| `id` | 文本对象 ID。 |
| `content` | 替换后的纯文本；未传默认为空字符串。 |
| `runs` | 富文本 run 输入；未传默认为空数组。 |

示例：

```json
{
  "edits": [
    {
      "type": "replace_text",
      "id": 12345,
      "content": "新的文本"
    },
    {
      "type": "replace_runs",
      "id": 67890,
      "runs": [
        {
          "content": "PDF",
          "font_name": "__pdfeditor_builtin_sans__",
          "font_size": 12,
          "color": [0, 0, 0, 255]
        }
      ]
    }
  ]
}
```

### TextRunInput

`runs_json` 和 `TextEdit.runs` 都使用该结构。

```ts
interface TextRunInput {
  content: string;
  font_name: string | null;
  font_size: number;
  color: [number, number, number, number];
}
```

| 字段 | 说明 |
| --- | --- |
| `content` | run 文本内容。 |
| `font_name` | PDF 字体资源名、内置哨兵字体名，或本地字体资源名。 |
| `font_size` | 字号。 |
| `color` | RGBA 数组，每项范围 `0..255`。 |

常用内置字体哨兵：

| 值 | 说明 |
| --- | --- |
| `__pdfeditor_builtin_monospace__` | 内置等宽字体。 |
| `__pdfeditor_builtin_serif__` | 内置衬线字体。 |
| `__pdfeditor_builtin_sans__` | 内置无衬线字体。 |
| `__cjk_fallback__` | 已注册的 CJK fallback 字体。 |
| `__localfont__:<key>` | 前端系统/本地字体资源名；保存前需注册对应 `key`。 |

## 裁剪与排版保存

### clip_bounds_json

为空字符串表示不裁剪；否则传 `Rect`：

```json
{
  "origin": { "x": 72, "y": 700 },
  "size": { "width": 300, "height": 18 }
}
```

后端保存时会将该裁剪矩形写入 PDF 内容流，使超出边框的内容在 PDF 查看器中也被裁掉。

### typography_json

为空字符串表示使用默认值：

```json
{
  "replace_spaces_with_displacements": false,
  "digit_font_name": null,
  "compress_punctuation": false,
  "detected_tj_displacements": false,
  "detected_space_displacements": false,
  "detected_punctuation": false,
  "detected_digit_font_name": null
}
```

保存参数通常只需要关注：

- `replace_spaces_with_displacements`：空格是否改写为 `TJ` 位移。
- `digit_font_name`：数字是否改用特殊字体。
- `compress_punctuation`：是否启用标点压缩。

`detected_*` 字段是后端识别信息，前端可以用于默认开关和提示；保存时传回不会影响识别过程本身。

## 字体嵌入注意事项

- `pdf_set_cjk_font_by_handle` 接受 WOFF1，并在后续需要 CJK fallback 时嵌入到 PDF。
- `pdf_set_local_font_by_handle` 当前只接受可转换为 TrueType-flavoured SFNT 的字体，包括 TTF、TrueType OTF、WOFF1、TTC。CFF/OpenType 字体会返回 `false`。
- 后端保存时会对后嵌入字体做子集化，只保留本次文档实际使用到的字符，避免导出的 PDF 过大。
- 如果某个 run 的 `font_name` 指向本地字体，前端必须先注册该字体 bytes；否则保存时无法使用该字体。

## 与现有 Web 前端的关系

现有 Web 前端对这些 API 做了封装：

- `apps/web/src/pdfEditor.ts`：wasm 初始化、页面加载包解析、字体加载、文本编辑和导出。
- `apps/web/src/App.vue`：编辑框、富文本 run、裁剪框、排版参数和系统字体注册。

新增调用建议优先复用 `pdfEditor.ts` 中的封装，除非需要直接验证 wasm 层行为。

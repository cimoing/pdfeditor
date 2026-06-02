# WebAssembly 编译说明

完整 wasm API、入参和出参结构见：[后端 wasm 使用说明](后端wasm使用说明.md)。

## 目标

`pdfeditor-core` 支持编译为 `wasm32-unknown-unknown`，用于 Web 端打开 PDF bytes，获得内存文档 handle，并通过 handle 读取页面结构、编辑文本和导出 PDF。

当前 wasm 暴露能力：

- `pdf_open_document(pdf_bytes)`
- `pdf_page_bundle(handle, page_number)`
- `pdf_page_structure(handle, page_number)`
- `pdf_hit_test(handle, page_number, pdf_x, pdf_y)`
- `pdf_start_text_edit(handle, object_id)`
- `pdf_preview_text_layout(handle, object_id, text)`
- `pdf_apply_text_edits(handle, edits_json)`
- `pdf_get_bytes(handle)`
- `pdf_set_cjk_font(handle, woff_bytes)`
- `pdf_set_local_font(handle, font_key, font_bytes)`
- `pdf_close_document(handle)`
- `page_number` 从 1 开始
- 输入/输出均基于内存数据，不依赖本地文件路径

## 编译

安装 wasm target：

```bash
rustup target add wasm32-unknown-unknown
```

编译 core wasm：

```bash
cargo build -p pdfeditor-core --target wasm32-unknown-unknown --features wasm
```

输出文件：

```text
target/wasm32-unknown-unknown/debug/pdfeditor_core.wasm
```

发布构建：

```bash
cargo build -p pdfeditor-core --target wasm32-unknown-unknown --features wasm --release
```

Web 前端项目已经把这一步封装到 npm：

```bash
cd apps/web
npm run wasm:build
```

## JavaScript 调用

建议使用 `wasm-bindgen` 生成 JS glue code：

```bash
wasm-bindgen \
  target/wasm32-unknown-unknown/release/pdfeditor_core.wasm \
  --target web \
  --out-dir web/pkg
```

浏览器侧示例：

```js
import init, {
  pdf_open_document,
  pdf_page_structure,
  pdf_page_bundle,
  pdf_apply_text_edits,
  pdf_get_bytes,
  pdf_close_document
} from "./pkg/pdfeditor_core.js";

await init();

const file = document.querySelector("input[type=file]").files[0];
const bytes = new Uint8Array(await file.arrayBuffer());
const handle = pdf_open_document(bytes);

try {
  const json = pdf_page_structure(handle, 1);
  const page = JSON.parse(json);
  console.log(page.page.size);
} finally {
  pdf_close_document(handle);
}
```

## Web 编辑流程

推荐前端把可视页面内容交给 CanvasKit/Skia 绘制，DOM 只保留控件和透明交互层：

```text
skia page layer:    pdf_page_bundle() 中的 background_png、字体资源和结构化文本
interaction layer:  hit test、文本编辑预览与提交 API
```

页面加载：

```js
const handle = pdf_open_document(bytes);
const bundle = pdf_page_bundle(handle, 1);
const { structure, background_png, fonts } = parsePageBundle(bundle);
await skiaRenderer.render({ structure, background_png, fonts });
```

文本编辑后保存：

```js
const edits = {
  edits: [
    {
      type: "replace_text",
      id: 12345,
      content: "新的文本"
    }
  ]
};

pdf_apply_text_edits(handle, JSON.stringify(edits));
const updatedPdfBytes = pdf_get_bytes(handle);
```

`id` 来自 `page.text[].id`。

## 当前限制

- wasm API 当前支持页面结构导出、页面加载包、文本编辑、字体注册和 PDF bytes 导出。
- 当前编辑操作以已有文本对象为单位，不新增或删除 PDF 对象。
- 图片替换、对象移动、复杂文字重排后续再扩展。
- 原生 CLI 仍负责本地文件路径模式和批量导出文件模式。

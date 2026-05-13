# WebAssembly 编译说明

## 目标

`pdfeditor-core` 支持编译为 `wasm32-unknown-unknown`，用于 Web 端直接解析 PDF bytes 并输出页面结构 JSON。

当前 wasm MVP 暴露能力：

- `pdf_page_to_json(pdf_bytes, page_number)`
- `pdf_page_background_png(pdf_bytes, page_number)`
- `pdf_image_object_png(pdf_bytes, page_number, image_object_id)`
- `pdf_apply_text_edits(pdf_bytes, edits_json)`
- `page_number` 从 1 开始
- 输入/输出均基于内存 bytes，不依赖本地文件路径

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
import init, { pdf_page_to_json } from "./pkg/pdfeditor_core.js";

await init();

const file = document.querySelector("input[type=file]").files[0];
const bytes = new Uint8Array(await file.arrayBuffer());
const json = pdf_page_to_json(bytes, 1);
const page = JSON.parse(json);

console.log(page.page.size);
```

## Web 编辑流程

推荐前端把可视页面内容交给 CanvasKit/Skia 绘制，DOM 只保留控件和透明交互层：

```text
skia page layer:    pdf_page_bundle() 中的 background_png、字体资源和结构化文本
interaction layer:  hit test、文本编辑预览与提交 API
```

页面加载：

```js
const bundle = pdf_page_bundle(bytes, 1);
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

const updatedPdfBytes = pdf_apply_text_edits(bytes, JSON.stringify(edits));
```

`id` 来自 `page.text[].id`。

## 当前限制

- wasm API 当前支持页面结构导出、背景 PNG bytes、图片对象 PNG bytes、文本对象内容替换。
- 当前编辑操作以已有文本对象为单位，不新增或删除 PDF 对象。
- 图片替换、对象移动、复杂文字重排后续再扩展。
- 原生 CLI 仍负责本地文件路径模式和批量导出文件模式。

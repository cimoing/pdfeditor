# 项目结构与 Web 前端说明

## 项目结构

```text
pdfeditor/
  crates/
    pdfeditor-core/    Rust 核心层、PDF 解析编辑、wasm API
    pdfeditor-cli/     命令行工具
  apps/
    web/               Vue Web demo
  docs/                设计和使用说明
```

## Web Demo

前端使用 Vue + Vite，核心能力来自 `pdfeditor-core` 编译出的 WebAssembly。

Demo 支持：

- 选择本地 PDF 文件。
- 指定页码并解析页面。
- 使用 CanvasKit/Skia 渲染页面背景 PNG 和结构化文本。
- 通过透明交互层选择文本对象。
- 修改指定文本对象内容。
- 保存并下载更新后的 PDF。

## npm 命令

首次使用时安装前端依赖：

```bash
cd apps/web
npm install
```

首次使用 wasm 构建链路时安装 `wasm-bindgen`：

```bash
npm run setup:wasm
```

构建 wasm：

```bash
npm run wasm:build
```

启动开发服务：

```bash
npm run dev
```

生产构建：

```bash
npm run build
```

也可以从仓库根目录执行：

```bash
npm run web:build
```

## 前端分层

Web 编辑器的可视页面内容由 CanvasKit/Skia 统一绘制，DOM 只负责控件、文件选择和透明交互热区：

```text
skia page layer:    页面基础背景 PNG + 结构化文本对象
interaction layer:  透明文本对象热区、内联编辑框、布局预览
```

保存时，前端把文本修改整理为 JSON，调用：

```text
pdf_apply_text_edits(pdf_bytes, edits_json)
```

返回值是更新后的 PDF bytes，可直接生成 Blob 下载。

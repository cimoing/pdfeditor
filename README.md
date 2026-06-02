# PDF Editor

PDF Editor 是一个面向 PDF 文本解析、编辑和导出的项目。核心能力由 Rust 实现，Web 前端通过 WebAssembly 调用核心库，在浏览器中完成 PDF 页面加载、文本对象选择、富文本编辑、字体嵌入和导出。

## 项目结构

```text
pdfeditor/
  crates/
    pdfeditor-core/    Rust 核心层，负责 PDF 解析、编辑、字体和 wasm API
    pdfeditor-cli/     命令行入口
  apps/
    web/               Vue Web 前端
  docs/                项目说明和设计文档
```

## 环境要求

- Rust stable
- Node.js 22 或兼容版本
- npm
- wasm32 目标：`rustup target add wasm32-unknown-unknown`
- wasm-bindgen CLI：`cargo install wasm-bindgen-cli --version 0.2.121 --locked`

也可以在 Web 前端目录中执行：

```bash
npm run setup:wasm
```

## 本地开发

安装前端依赖：

```bash
cd apps/web
npm ci
```

启动 Web 前端开发服务：

```bash
npm run dev
```

如果 wasm 已经构建完成，仅启动 Vite：

```bash
npm run dev:nowasm
```

## 构建

从仓库根目录检查 Rust 工作区：

```bash
cargo check --workspace --all-targets
```

检查 wasm 目标：

```bash
cargo check -p pdfeditor-core --target wasm32-unknown-unknown --features wasm
```

构建 Web 前端：

```bash
npm run web:build
```

构建产物输出到 `apps/web/dist`。

## GitHub Pages

仓库包含 GitHub Actions 工作流 `.github/workflows/build.yml`。每次 push 会构建 Rust 和 Web 前端，并将 `apps/web/dist` 发布为 GitHub Pages artifact。

使用前请在 GitHub 仓库设置中启用 Pages，并将发布来源设置为 GitHub Actions。

## 文档

- [WebAssembly 编译说明](docs/WebAssembly编译说明.md)
- [后端 wasm 使用说明](docs/后端wasm使用说明.md)
- [CLI 使用说明](docs/CLI使用说明.md)
- [项目结构与 Web 前端说明](docs/项目结构与Web前端说明.md)

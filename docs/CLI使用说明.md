# CLI 使用说明

## 1. 功能

当前 CLI 提供 PDF 文本替换能力：

- 指定 PDF 文件。
- 查找指定文本。
- 替换为新文本。
- 可指定替换次数。
- 可通过参数允许替换文本超出原文本边界。
- 默认会根据替换后的文本自动扩展文本对象边界。
- 可通过参数手动修改文本对象边界，用于覆盖自动计算结果。
- 未指定替换次数时替换全部匹配项。
- 支持基于 PDF `ToUnicode` CMap 解码后的文本匹配。
- 支持查找内容跨多个 PDF 文本绘制对象的情况。

## 2. 命令格式

```bash
cargo run -p pdfeditor-cli -- replace --file <input.pdf> --find <old> --replace <new> [--count <n>] [--bounds <x,y,width,height>] [--allow-overflow] [--output <output.pdf> | --in-place]
```

参数说明：

- `--file` 或 `-f`：输入 PDF 文件路径。
- `--find`：需要查找的文本。
- `--replace`：替换后的文本。
- `--count` 或 `-n`：最大替换次数。不指定则替换全部。
- `--bounds`：手动修改被替换文本对象的边界，格式为 `x,y,width,height`。
- `--allow-overflow`：允许替换后的文本超出原文本对象边界。
- `--output` 或 `-o`：输出 PDF 文件路径。
- `--in-place`：直接覆盖原 PDF 文件。

如果没有指定 `--output` 和 `--in-place`，CLI 会默认输出到同目录下的 `<原文件名>.edited.pdf`。

## 3. 示例

替换全部：

```bash
cargo run -p pdfeditor-cli -- replace --file sample.pdf --find "Hello" --replace "World"
```

只替换前 2 次：

```bash
cargo run -p pdfeditor-cli -- replace --file sample.pdf --find "Hello" --replace "World" --count 2 --output sample.updated.pdf
```

允许超出原文本边界：

```bash
cargo run -p pdfeditor-cli -- replace --file sample.pdf --find "Hello" --replace "Longer replacement text" --allow-overflow --output sample.updated.pdf
```

手动指定文本对象边界后替换：

```bash
cargo run -p pdfeditor-cli -- replace --file sample.pdf --find "Hello" --replace "Longer replacement text" --bounds 72,72,500,40 --output sample.updated.pdf
```

直接覆盖原文件：

```bash
cargo run -p pdfeditor-cli -- replace --file sample.pdf --find "Hello" --replace "World" --in-place
```

## 4. 当前限制

- 当前基于 `LopdfEngine`，支持简单 `Tj`、`TJ` 文本绘制操作。
- 已支持常见 `ToUnicode` CMap 中的 `bfchar`、`bfrange` 映射。
- 默认情况下，替换后的文本仍需位于原文本对象矩形范围内，超出会拒绝修改。
- CLI 会默认根据替换后的文本估算所需宽高，并自动扩大文本对象边界。
- 使用 `--allow-overflow` 后会跳过边界校验，可能导致文本视觉上覆盖其他内容。
- 使用 `--bounds` 会修改当前编辑模型中的文本对象边界，用于替换前的范围校验；它不会移动 PDF 中已有文本的实际绘制位置。
- 暂不支持扫描件 OCR。
- 暂不支持没有 `ToUnicode` 映射的复杂字体编码和复杂文字重排。
- 暂不支持图片内容替换。

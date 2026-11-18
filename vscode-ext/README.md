# tools-regex 正则设计器（VSCode 插件）

铁路图 + 人话层 + 灾难性回溯检测 + 六方言代码导出——**核心由 Rust wasm 驱动**，
与桌面版（tools-regex）同一套解析器/布局/引擎矩阵/导出器（见仓库根 README）。

## 使用

命令面板（Ctrl+Shift+P）→ **`tools-regex：打开正则设计器`**。
选中编辑器中的正则文本后执行命令，会作为初始正则带入。

## 打包与安装

```bash
# 构建 wasm（在仓库根目录）
bash scripts/build_ext.sh

# 编译 TS + 打包 .vsix（本目录）
npm install && npm run compile
npx @vscode/vsce package --allow-missing-repository --no-dependencies

# 本地安装
code --install-extension tools-regex-designer-0.1.0.vsix
```

## 已知边界

- webview 中文使用 VSCode 环境字体（插件不内嵌字体，控制体积）
- 大文件扫描/回溯时间轴等桌面级功能不在插件范围（见桌面版）

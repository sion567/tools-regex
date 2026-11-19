# tools-regex — Rust 正则可视化与调试工具

设计方案（唯一权威）：[Rust正则可视化工具设计方案.md](./Rust正则可视化工具设计方案.md)。

**当前状态：M0–M4 全部里程碑收官 + 收官打磨 + 生态双件（浏览器版 & VSCode 插件）完成**，
95 项测试全绿、clippy 全 target 零警告。

## 结构

```
crates/
├── regex-ir/        # 统一 IR + 方言特性矩阵 + used_features（零依赖）
├── regex-parser/    # PCRE 超集递归下降解析器（带 span 错误 + 宽松部分解析）
├── regex-plain/     # 人话层：正则 → 中文逐段解释（span 对齐）
├── regex-diagram/   # IR → 铁路图布局（零 UI 依赖；Shape 携 span；圆角轨道）
├── regex-exec/      # 引擎矩阵 + interp（教学解释器 VM）+ danger（回溯检测）+ stream（大文件）
├── regex-export/    # SVG 导出 + golden 快照 + codegen（六方言代码导出）
└── app/             # eframe 壳：工作台(TEXT/TEXTS) / 反推正则 / 速查表 / 大文件扫描 / 闯关教学 / 代码导出 / spike 下拉
fixtures/            # 设计方案第 12 章示例的可执行化（示例 1/2/4，TOML）
```

## 快速开始

```bash
cargo run -p app                 # 六个功能 Tab + spike 下拉；正则支持 JS 风格 /pattern/flags
cargo test --workspace           # 95 项测试（fixture + golden + fuzz + VM + 闯关自验证）
bash scripts/lint_deps.sh        # 依赖纪律
bash scripts/build_release.sh    # 桌面发布构建（单二进制 target/release/regex.exe）
bash scripts/build_web.sh        # 浏览器版构建（crates/app/dist/，见下文说明）
```

## 日志与崩溃排查

桌面版日志写在** exe 旁边**的 `regex.log`（写不进则退回 `%TEMP%`），并同步镜像 stderr；
浏览器版走 DevTools console。`app/src/logging.rs` 自带 panic 钩子：应用崩溃时，
panic 消息、源码位置和 backtrace 会**先落盘再退出**——窗口闪退后去日志文件里看
`[PANIC]` 段即可定位。打点只发生在事件处（启动/切 Tab/选关/检查/看参考解/下一关），
egui 按帧渲染，不做按帧打点。

## M4 能力清单（已完成）

- **闯关教学系统**（设计方案第八章落地）：E 组渐进关卡 L0–L8（字面量→字符类→量词区间→
  分支→捕获组→多行锚点→贪婪转懒惰→反向引用→环视）+ Boss 关（动手修复 `(a+)+b`，
  功能对但仍有回溯风险也过不了）+ 方言翻译实战 P1–P3（命名组 `(?<n>)`↔`(?P<n>)` 双向 +
  DFA 家族特性门禁）。
  **检查器**：Match 关命中集合精确对齐（多杀/漏杀都不过，错位给针对性提示）；Port 关三重校验
  （命名写法 textual 检查 → 特性矩阵门禁 → 与源正则命中等价）。
  **自验证**：每关内置参考解，测试强制"参考解必通关"——期望集合抄错立刻暴露（实测抓出
  L4/L5 我手算错两处区间）。
- **多语言代码导出（codegen）**：Java/Python/JS/Go/Rust 五方言即用片段。三件套：
  ① 特性门禁——目标方言 No 的特性**拒绝生成**"看似能跑实则报错"的代码 + 改写建议
  （区别于 Regex101 只标红，设计方案 7.5 第 3 条）；② 命名组语法逐方言改写；③ 字符串转义
  翻译（Java 双反斜杠、Python raw、JS `/` 逃逸、Go raw/引号双路）。Partial 特性（如 JS
  后顾 ES2018+）生成但头部提醒。
- **M3/M2/M1 全部能力保持**（铁路图箭头/圆角、引擎矩阵、大文件扫描、人话层联动等）。

## 人工验收清单（M4 增量）

- [ ] 「闯关教学」Tab：L0 输入 `dog` 通关；L0 输入 `cat|dog|cow` 被拒（多杀提示）
- [ ] L5 用 `^https?://`（无 (?m)）过不了；`(?m)^https?://` 通关
- [ ] L6 贪婪 `<.+>` 被拒、`<[^>]*>` 或 `<.+?>` 通关
- [ ] Boss：`(a+)+b` 命中对但被拒（"仍带灾难性回溯风险"）；`a+b` 通关
- [ ] P1 写 Java 写法被拒（"Python/Go 只认 (?P<name>)"）；P3 输入含环视的正则被特性门禁拒
- [ ] 「代码导出」Tab：命名组示例 → Java 显示 `(?<k>)`、Python 显示 `(?P<k>)`、各组"复制"
- [ ] 导出 `\b(\w+) \1\b` → Go/Rust 显示红色"拒绝"+ 改写建议；Java/Python 正常
- [ ] 导出日期正则 → Java 代码里是 `\\b(\\d{4})`（双重转义可见）

## 测试资产（95 项）

ir 3 / parser 16（含 `(?:` 回归锁、used_features 端到端）/ plain 7 / diagram 16（箭头×3 +
徽标上轨）/ fuzz 3 / export 16（SVG 3 + golden 4 + codegen 9 含 DotNet）/ exec 27
（矩阵 5 + interp 12 含标志语义回归 + danger 3 + stream 5 + fixtures 2）/ lessons 4
（**参考解全关卡自验证** + 反例 + 移植门禁 + 语法助手）/ wasm-api 3
（analyze 全链路 + 错误/风险/拒绝导出 + 空模式）

## 收官打磨（M4 后增量，已完成）

- **连接线箭头**：汇入点实心三角（concat 入口 / 交替车道 / 回环再入），egui 与 SVG 双后端
  同一几何——M0 起挂着的"已知边界"清账
- **量词徽标上轨**：{m,n} 次数标注贴回环轨上缘（设计方案 6.2 "重复区间正式化"的落地形态）
- **.NET 方言导出**：C# 片段（verbatim 字符串 + Groups 访问）；顺手勘误 7.2 矩阵——
  .NET 无占有量词语法（原文"7+"有误），矩阵改为 No 并进门禁（原子组放行）
- **release 构建验证**：scripts/build_release.sh（构建 + release profile 冒烟测试）

## 过程实录（M4 真 bug）

- **dev-dep path 循环**把 regex-ir 复制成两个版本（ir 测试里用 parser 导致）——端到端测试
  移到 parser 侧，ir 恢复零依赖并立碑警示
- 命名组"源语法"在 IR 层不保留 → 语法类提醒改挂在真正的 Partial 特性上（诚实设计）
- 自验证测试抓出我手算错 L4/L5 期望区间两处——"参考解必通关"机制的价值实证

## 浏览器版（wasm 探索项，已落地）

设计方案 9.4 的"探索 WASM 浏览器版"已交付为**双 edition 工程**：

```
crates/app：lib（cdylib，#[wasm_bindgen(start)] 入口）+ bin（桌面 main）
           —— 两个形态共享同一套 UI；stream/file_scan/线程按 target cfg 门控
crates/app/dist/：wasm 构建产物（app.js + app_bg.wasm + index.html）
scripts/build_web.sh：构建脚本
```

- 构建：`bash scripts/build_web.sh`（前置 `rustup target add wasm32-unknown-unknown`；
  `cargo install wasm-bindgen-cli@<Cargo.lock 版本>`；trunk 可选——本仓库用 cargo+wasm-bindgen
  直连管线，绕开 trunk 的 github 下载依赖）
- 桌面/浏览器差异（设计方案 9.4 两个 edition 的落点）：大文件扫描页在浏览器里显式说明
  "属桌面版"；匹配扫描在 wasm 上同步执行（样本小）；中文 UI 在浏览器降级为默认字体
  （无 Windows 字体路径，探索版已知边界）
- **验证状态**：wasm32 编译零警告；release 产物 4.8MB；真浏览器加载零 JS 错误、
  eframe WebRunner 完整启动（探针 `RUNNER-START-OK`）。IAB 内嵌 webview 存在宿主限制：
  `requestAnimationFrame` 不派发（探针证实 `RAF-NOT-FIRED`，且 `visibilityState=visible`）——
  egui 帧循环依赖 rAF，故在该宿主内不渲染；标准浏览器前台标签不受影响。
  代码内保留探针（RUNNER-START-OK / UPDATE-FRAME），真浏览器打开后 console 可见。
- 本地体验：`cd crates/app/dist && python -m http.server 8899` → 浏览器开
  http://127.0.0.1:8899

## VSCode 插件（已落地）

```
crates/wasm-api/    # 绑定层：analyze(pattern, sample) 单入口 → 设计器全部数据（JSON）
vscode-ext/         # 插件工程（TypeScript + webview + wasm）
├── media/designer.html + wasm_api.js + wasm_api_bg.wasm（1.6MB）
├── src/extension.ts（命令 toolsRegex.openDesigner；选中正则文本自动带入）
└── 打包：npm run compile && npx @vscode/vsce package
```

- **webview 设计器**：铁路图（同一布局引擎，含箭头/徽标）、人话层逐段联动、匹配高亮、
  灾难性回溯红标、六方言代码导出（拒绝式门禁）——全部跑在 wasm 里，与桌面版同一套 Rust 逻辑
- **真机验证**：designer 页在真实浏览器加载 wasm 全链路通过（截图验收：示例 1 命中 2 处高亮、
  13 段人话、6 张代码卡；输入 `(a+)+b` 实时显示"⚠ 回溯风险 1 处"+ 嵌套回环重绘）
- **安装**：`vscode-ext/tools-regex-designer-0.1.0.vsix` 已打包并装入本地 VSCode
  （命令面板 → "tools-regex：打开正则设计器"）
- wasm 重建：`bash scripts/build_ext.sh`

## 收官说明与后续方向

设计方案第十章 M0–M4 全部落地（含 wasm 浏览器版与 VSCode 插件两个生态项）。
可选后续：插件发布到 Marketplace（需要 publisher 账号）、人话层"中文→正则"反向。

其余可选打磨（不阻塞收官）：人话层"中文→正则"反向（设计方案已砍出 v1）。

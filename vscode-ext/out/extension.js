"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.activate = activate;
exports.deactivate = deactivate;
const vscode = __importStar(require("vscode"));
const fs = __importStar(require("fs"));
const path = __importStar(require("path"));
/**
 * tools-regex 设计器（VSCode 插件壳）。
 * 核心 crates 编 wasm（crates/wasm-api）→ webview 加载 → 展示铁路图/人话层/
 * 匹配高亮/六方言代码导出——与桌面版同一套 Rust 逻辑。
 */
function activate(context) {
    const cmd = vscode.commands.registerCommand("toolsRegex.openDesigner", async () => {
        const editor = vscode.window.activeTextEditor;
        const selected = editor && !editor.selection.isEmpty ? editor.document.getText(editor.selection) : undefined;
        const panel = vscode.window.createWebviewPanel("toolsRegexDesigner", "tools-regex 设计器", vscode.ViewColumn.Beside, {
            enableScripts: true,
            // wasm 实例化需要 'wasm-unsafe-eval'；资源仅限插件目录
            localResourceRoots: [vscode.Uri.joinPath(context.extensionUri, "media")],
        });
        panel.webview.html = buildHtml(context.extensionUri, panel.webview, selected);
    });
    context.subscriptions.push(cmd);
}
function mediaUri(extensionUri, webview, ...p) {
    return webview.asWebviewUri(vscode.Uri.joinPath(extensionUri, "media", ...p)).toString();
}
function buildHtml(extensionUri, webview, initialPattern) {
    const mediaDir = path.join(extensionUri.fsPath, "media");
    const template = fs.readFileSync(path.join(mediaDir, "designer.html"), "utf8");
    const nonce = Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
    const csp = [
        `default-src 'none'`,
        // inline 注入脚本靠 nonce 放行（下方 <script nonce=...> 与之配套）
        `script-src ${webview.cspSource} 'wasm-unsafe-eval' 'nonce-${nonce}'`,
        `style-src ${webview.cspSource} 'unsafe-inline'`,
        `img-src ${webview.cspSource} data:`,
        `font-src ${webview.cspSource}`,
    ].join("; ");
    return template
        .replace("<head>", `<head>\n<meta http-equiv="Content-Security-Policy" content="${csp}">`)
        .replace("window.__URIS__ =", 
    // 占位符先于模块脚本执行：webview URI 注入
    `window.__URIS__ = { appJs: "${mediaUri(extensionUri, webview, "wasm_api.js")}", wasm: "${mediaUri(extensionUri, webview, "wasm_api_bg.wasm")}" }; void 0, window.__URIS__ =`)
        .replace("window.__URIS__ =", `window.__INIT_PATTERN__ = ${JSON.stringify(initialPattern ?? null)}; void 0, window.__URIS__ =`)
        .replace(/<script(?! type="module")/g, `<script nonce="${nonce}"`)
        .replace(/<script type="module"/g, `<script type="module" nonce="${nonce}"`);
}
function deactivate() { }

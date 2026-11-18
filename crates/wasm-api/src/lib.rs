//! wasm 绑定层：VSCode 插件 webview / 浏览器复用的统一入口（设计方案 M4 后置项落地）。
//!
//! 设计原则：单入口 + JSON 出参——把解析/人话/布局/引擎/导出全部在 Rust 侧算好，
//! JS 侧只做展示。核心 crates 保持零 UI 依赖（本 crate 是唯一的粘合层）。

use serde::Serialize;

/// 设计器一次交互的全部结果
#[derive(Serialize)]
struct AnalyzeResult {
    /// 解析是否完整成功（宽松解析下部分成功也算有内容）
    ok: bool,
    /// 解析错误（列号 + 提示）；完整合法为 null
    error: Option<String>,
    /// 人话层一句话流程
    summary: String,
    /// 人话分段（span 指回源码，供联动高亮）
    segments: Vec<SegmentDto>,
    /// 铁路图 SVG（与桌面版同一布局引擎）
    svg: String,
    /// 样本文本命中区间（字节）
    matches: Vec<(usize, usize)>,
    /// 实际服务的引擎（真身/近似），双编译失败为 null
    engine: Option<EngineDto>,
    /// 双引擎合并报错
    compile_error: Option<String>,
    /// 灾难性回溯风险描述
    danger: Vec<String>,
    /// 六方言代码导出（值以 "!" 开头 = 拒绝导出 + 理由）
    code: std::collections::BTreeMap<String, String>,
}

#[derive(Serialize)]
struct SegmentDto {
    s: usize,
    e: usize,
    t: String,
}

#[derive(Serialize)]
struct EngineDto {
    label: String,
    approx: bool,
}

fn analyze_impl(pattern: &str, sample: &str) -> AnalyzeResult {
    use regex_export::codegen::{export_code, TARGETS};

    let (node, err) = regex_parser::parse_partial(pattern);
    let error = err.map(|e| e.to_string());

    let segments: Vec<SegmentDto> = regex_plain::segments(&node)
        .into_iter()
        .map(|s| SegmentDto {
            s: s.span.start as usize,
            e: s.span.end as usize,
            t: s.text,
        })
        .collect();
    let summary = regex_plain::summary(&node);
    let danger: Vec<String> = regex_exec::danger::check(&node)
        .into_iter()
        .map(|r| r.desc)
        .collect();

    let svg = if matches!(node, regex_ir::Node::Empty) && error.is_some() {
        String::new()
    } else {
        regex_export::diagram_to_svg(&regex_diagram::layout(&node))
    };

    let sc = regex_exec::Scanner::new(pattern);
    let engine = sc.engine().map(|e| EngineDto { label: e.label().to_string(), approx: e.approximate() });
    let compile_error = sc.error().map(str::to_string);
    let matches = sc.find_in(sample);

    let mut code = std::collections::BTreeMap::new();
    for d in TARGETS {
        let entry = match export_code(&node, pattern, d) {
            Ok(c) => c,
            Err(e) => format!("!{e}"),
        };
        code.insert(d.lang_label().to_string(), entry);
    }

    AnalyzeResult {
        ok: error.is_none(),
        error,
        summary,
        segments,
        svg,
        matches,
        engine,
        compile_error,
        danger,
        code,
    }
}

/// 单入口：pattern + sample → 设计器全部数据（JSON）
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn analyze(pattern: &str, sample: &str) -> Result<wasm_bindgen::prelude::JsValue, wasm_bindgen::prelude::JsValue> {
    let json = serde_json::to_string(&analyze_impl(pattern, sample))
        .map_err(|e| wasm_bindgen::prelude::JsValue::from_str(&e.to_string()))?;
    Ok(wasm_bindgen::prelude::JsValue::from_str(&json))
}

// native 测试/调试入口（wasm-bindgen 宏在 native 下不导出）
#[cfg(not(target_arch = "wasm32"))]
pub fn analyze_json(pattern: &str, sample: &str) -> String {
    serde_json::to_string(&analyze_impl(pattern, sample)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_example1_full_pipeline() {
        let v: serde_json::Value = serde_json::from_str(&analyze_json(
            r"\b(\d{4})-(\d{2})-(\d{2})\b",
            "Contract signed on 2024-08-15, amended 2025-01-01 (see 2025/02/03 doc).",
        ))
        .unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        assert!(v["error"].is_null());
        assert!(v["svg"].as_str().unwrap().starts_with("<svg"));
        assert!(v["summary"].as_str().unwrap().contains("词边界"));
        // 命中两处日期（fixture 同源数据）
        assert_eq!(v["matches"].as_array().unwrap().len(), 2);
        // 引擎：真身
        assert_eq!(v["engine"]["approx"], serde_json::Value::Bool(false));
        // 六方言导出齐全且 Java 转义生效
        assert_eq!(v["code"].as_object().unwrap().len(), 6);
        assert!(v["code"]["Java（java.util.regex）"].as_str().unwrap().contains("\\\\b"));
        // 无风险
        assert!(v["danger"].as_array().unwrap().is_empty());
    }

    #[test]
    fn analyze_error_and_danger_and_refusal() {
        // 解析错误：宽松前缀照出，error 带列号
        let v: serde_json::Value = serde_json::from_str(&analyze_json("(ab", "abc")).unwrap();
        assert!(v["error"].as_str().unwrap().contains("闭合"));
        // ReDoS 风险可见
        let v: serde_json::Value = serde_json::from_str(&analyze_json("(a+)+b", "aaaaab")).unwrap();
        assert_eq!(v["danger"].as_array().unwrap().len(), 1);
        // DFA 拒绝导出：Go 侧给 "!" 前缀理由
        let v: serde_json::Value = serde_json::from_str(&analyze_json(r"\b(\w+) \1\b", "the the")).unwrap();
        let go = v["code"]["Go（regexp）"].as_str().unwrap();
        assert!(go.starts_with('!') && go.contains("反向引用"), "{go}");
    }

    #[test]
    fn analyze_empty_pattern() {
        let v: serde_json::Value = serde_json::from_str(&analyze_json("", "abc")).unwrap();
        assert!(v["segments"].as_array().unwrap().is_empty());
    }
}

//! CJK 字体加载 —— spike 0：egui 默认字体不含中文字形，不装字体全是"豆腐块"。
//! 依次尝试 Windows 常见中文字体，装进 Proportional 与 Monospace 的兜底位。

pub fn install_cjk(ctx: &egui::Context) -> Result<(), String> {
    let candidates = [
        "C:/Windows/Fonts/msyh.ttc",    // 微软雅黑
        "C:/Windows/Fonts/Deng.ttf",    // 等线
        "C:/Windows/Fonts/simhei.ttf",  // 黑体
        "C:/Windows/Fonts/simsun.ttc",  // 宋体
    ];
    let (path, bytes) = candidates
        .iter()
        .find_map(|p| std::fs::read(p).ok().map(|b| (*p, b)))
        .ok_or_else(|| "未找到 Windows 中文字体（msyh/Deng/simhei/simsun）".to_string())?;

    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("cjk".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    // 追加到 families 队尾 = 兜底字体：缺字形时才用（不影响 ASCII 主字体）
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("cjk".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("cjk".into());
    ctx.set_fonts(fonts);
    crate::logging::info(&format!("[fonts] CJK 字体已加载：{path}"));
    Ok(())
}

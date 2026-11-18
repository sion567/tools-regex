//! Spike 1：中文 IME。
//! 验收点：组合文本是否内联显示在光标处、候选窗位置、组合中退格、上屏结果正确性。

#[derive(Default)]
pub struct State {
    single: String,
    multi: String,
}

pub fn show(ui: &mut egui::Ui, st: &mut State) {
    ui.heading("Spike 1｜中文 IME（egui / Windows）");
    ui.label(
        egui::RichText::new("目标：用系统输入法（微软拼音等）在下面两个输入框打中文，逐条核对：")
            .weak(),
    );
    ui.vertical(|ui| {
        ui.label("1. 拼音组合期：预编辑文本是否内联出现在光标处（而不是框外/左上角）？");
        ui.label("2. 候选词窗口是否跟随光标位置？");
        ui.label("3. 组合中按退格：只删拼音字母，不会把整段组合文本干掉？");
        ui.label("4. 上屏后：下方「码点」显示的 U+XXXX 是否是期望的汉字？");
        ui.label("5. 多行框里粘贴一段中文日志，滚动/编辑是否正常？");
    });
    ui.add_space(8.0);

    ui.label("单行输入：");
    ui.add(
        egui::TextEdit::singleline(&mut st.single)
            .hint_text("在这里用拼音输入「正则表达式」")
            .desired_width(f32::INFINITY),
    );
    ui.label("多行输入：");
    ui.add(
        egui::TextEdit::multiline(&mut st.multi)
            .hint_text("粘贴一段中文日志试试（例如：2026-11-17 处理订单超时，已重试 3 次）")
            .desired_rows(6)
            .desired_width(f32::INFINITY),
    );

    ui.add_space(8.0);
    ui.separator();
    codepoints(ui, "单行", &st.single);
    codepoints(ui, "多行", &st.multi);
}

fn codepoints(ui: &mut egui::Ui, name: &str, s: &str) {
    let chars = s.chars().collect::<Vec<_>>();
    let tail: Vec<String> = chars
        .iter()
        .rev()
        .take(12)
        .rev()
        .map(|c| format!("{c} U+{:04X}", *c as u32))
        .collect();
    ui.label(format!("{name}：{} 字符 / {} 字节", chars.len(), s.len()));
    if !tail.is_empty() {
        ui.monospace(tail.join("  "));
    } else {
        ui.weak("(空)");
    }
}

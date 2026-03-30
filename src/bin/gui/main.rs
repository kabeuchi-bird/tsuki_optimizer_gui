#![windows_subsystem = "windows"]
// gui — tsuki_optimize GUI エントリポイント
//
// eframe (egui) を使用し、最適化の進行をリアルタイム表示する。

mod app;
mod draw;
mod log_writer;
mod update;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 780.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "tsuki_optimize GUI",
        options,
        Box::new(|cc| {
            setup_japanese_fonts(&cc.egui_ctx);
            Ok(Box::new(app::App::new()))
        }),
    )
}

/// 日本語フォント（IPAゴシック）をバイナリに埋め込み、egui に登録する
fn setup_japanese_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "ipag".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../../assets/ipag.ttf"
        ))),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("ipag".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("ipag".to_owned());

    ctx.set_fonts(fonts);
}

const SAMPLE_CORPUS: &str = "\
こんにちは。今日はいい天気ですね。\
日本語入力の配列を最適化するためのプログラムです。\
タブーサーチを用いて月配列の改変版を探索します。\
かな文字の打鍵数と難易度を評価して最良の配置を求めます。\
てにをはなどの助詞や、よく使う動詞・形容詞が打ちやすくなるように配置します。\
";

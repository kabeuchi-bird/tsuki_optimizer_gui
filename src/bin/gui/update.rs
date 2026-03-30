use eframe::egui;

use super::app::App;
use super::log_writer::ColorMode;

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_updates();

        // ── ツールバー ──
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.running {
                    if ui.button("⏹ 停止").clicked() {
                        self.stop_search();
                    }
                } else if ui.button("▶ 開始").clicked() {
                    self.start_search();
                }

                ui.separator();
                ui.label("seed:");
                ui.add(egui::TextEdit::singleline(&mut self.seed_str).desired_width(80.0));
                ui.label("iter:");
                ui.add(egui::TextEdit::singleline(&mut self.iter_str).desired_width(80.0));
                ui.label("restart:");
                ui.add(egui::TextEdit::singleline(&mut self.restart_str).desired_width(60.0));
                ui.separator();
                ui.label("corpus:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.corpus_path_str).desired_width(120.0),
                );
                ui.separator();
                ui.label("keyboard:");
                egui::ComboBox::from_id_salt("kb_size")
                    .selected_text(&self.keyboard_size_str_input)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.keyboard_size_str_input,
                            "3x10".to_string(),
                            "3x10",
                        );
                        ui.selectable_value(
                            &mut self.keyboard_size_str_input,
                            "3x11".to_string(),
                            "3x11",
                        );
                    });
            });

            // 設定ファイルエラー表示
            if let Some(ref err) = self.config_error {
                ui.colored_label(egui::Color32::RED, format!("⚠ config.toml エラー: {err}"));
            }
        });

        // ── ステータスバー ──
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(ref upd) = self.latest_update {
                    ui.label(format!(
                        "iter: {} | restarts: {} | current: {:.4} | best: {:.4} | phase: {:?}",
                        upd.iter, upd.restarts, upd.current_score, upd.best_score, upd.phase
                    ));
                } else {
                    ui.label("待機中...");
                }
            });
        });

        // ── メインエリア ──
        egui::CentralPanel::default().show(ctx, |ui| {
            // 色分けモード選択
            let prev_mode = self.color_mode;
            ui.horizontal(|ui| {
                ui.label("表示モード:");
                ui.radio_value(
                    &mut self.color_mode,
                    ColorMode::Fitness,
                    "フィットネスマップ",
                );
                ui.radio_value(
                    &mut self.color_mode,
                    ColorMode::Frequency,
                    "頻度ヒートマップ",
                );
                ui.radio_value(
                    &mut self.color_mode,
                    ColorMode::FingerLoad,
                    "指負荷バランス",
                );
                ui.radio_value(&mut self.color_mode, ColorMode::Log, "ログ");
                ui.separator();
                ui.checkbox(&mut self.show_layer2, "Layer 2 表示");
            });
            if self.color_mode != prev_mode {
                self.invalidate_color_cache();
            }

            ui.separator();

            // 上半分: キーボード表示 + スコア推移グラフ
            let available_height = ui.available_height();
            let top_height = available_height * 0.55;

            ui.horizontal(|ui| {
                // 左側: キーボード表示
                let kb_width = ui.available_width() * 0.45;
                ui.allocate_ui_with_layout(
                    egui::vec2(kb_width, top_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| match self.color_mode {
                        ColorMode::FingerLoad => self.draw_finger_load(ui),
                        ColorMode::Log => self.draw_log(ui),
                        _ => self.draw_keyboard(ui),
                    },
                );

                ui.separator();

                // 右側: スコア推移グラフ
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), top_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        self.draw_score_graph(ui);
                    },
                );
            });

            ui.separator();

            // 下半分: スコア内訳
            self.draw_score_info(ui);
        });

        // 探索中はフレーム更新を継続
        if self.running {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

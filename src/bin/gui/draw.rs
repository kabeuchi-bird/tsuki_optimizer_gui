use eframe::egui;
use egui::epaint::StrokeKind;
use egui_plot::{Line, PlotPoints, VLine};

use tsuki_optimize::chars::{CharId, CHAR_LIST, DAKUTEN_ID, HANDAKUTEN_ID, MAX_CHARS, VOID_CHAR_FIRST};
use tsuki_optimize::corpus::Corpus;
use tsuki_optimize::cost::{score_breakdown_data, Weights};
use tsuki_optimize::layout::{
    col_to_finger, keystrokes_for_slot, slot_col, slot_hand, Hand, KeyboardSize,
    SHIFT_SLOT_SENTINEL,
};
use tsuki_optimize::search::SearchUpdate;

use super::app::App;
use super::log_writer::{ColorData, ColorMode};

// ──────────────────────────────────────────────────────────────
// 描画ヘルパー
// ──────────────────────────────────────────────────────────────
impl App {
    pub fn draw_keyboard(&mut self, ui: &mut egui::Ui) {
        let Some(ref upd) = self.latest_update else {
            ui.label("探索を開始してください");
            return;
        };
        let layout = &upd.best_layout;
        let kp = layout.kp;
        let nc = kp.num_cols as usize;
        let npl = kp.num_slots_per_layer as usize;

        // 色分けキャッシュ: 更新がなければ再計算しない
        if self.cached_color_data.is_none() {
            self.cached_color_data = Some(precompute_color_data(
                self.color_mode,
                upd,
                self.corpus.as_ref(),
                self.weights.as_ref(),
            ));
        }
        let color_data = self.cached_color_data.as_ref().unwrap();

        let layers: &[(&str, usize)] = if self.show_layer2 {
            &[("Layer 1", 0), ("Layer 2", npl)]
        } else {
            &[("Layer 1", 0)]
        };

        for &(label, slot_offset) in layers {
            ui.label(egui::RichText::new(label).strong().size(14.0));

            let cell_size = egui::vec2(36.0, 36.0);
            let spacing = 3.0;

            for row in 0..3usize {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    for col in 0..nc {
                        let slot = slot_offset + row * nc + col;

                        let is_shift = kp.size == KeyboardSize::K3x11
                            && slot_offset == 0
                            && (slot == kp.shift_left as usize || slot == kp.shift_right as usize);

                        let char_id = layout.slot_to_char[slot];
                        let ch = if is_shift {
                            if slot == kp.shift_left as usize {
                                '☆'
                            } else {
                                '★'
                            }
                        } else if char_id == SHIFT_SLOT_SENTINEL || char_id >= VOID_CHAR_FIRST {
                            '□'
                        } else {
                            CHAR_LIST[char_id as usize]
                        };

                        let bg_color = if is_shift {
                            // 3x11 の専用シフトキー: ヒートマップ時はシフト打鍵頻度で着色
                            if let ColorData::Frequency {
                                max_freq,
                                shift_freq,
                            } = color_data
                            {
                                let idx = if slot == kp.shift_left as usize {
                                    0
                                } else {
                                    1
                                };
                                shift_slot_color(shift_freq[idx], *max_freq)
                            } else {
                                egui::Color32::from_rgb(160, 160, 160)
                            }
                        } else if char_id == SHIFT_SLOT_SENTINEL || char_id >= VOID_CHAR_FIRST {
                            egui::Color32::from_rgb(200, 200, 200)
                        } else {
                            // 3x10 のシフトキー兼文字キー（D/K）はシフト打鍵分を加算
                            let extra = if let ColorData::Frequency { shift_freq, .. } = color_data
                            {
                                let s = layout.char_to_slot[char_id as usize];
                                if s == kp.shift_left {
                                    shift_freq[0]
                                } else if s == kp.shift_right {
                                    shift_freq[1]
                                } else {
                                    0.0
                                }
                            } else {
                                0.0
                            };
                            char_color(
                                char_id,
                                layout.char_to_slot[char_id as usize],
                                &self.latest_update.as_ref().unwrap().unigrams,
                                color_data,
                                extra,
                            )
                        };

                        let is_l2 = slot_offset > 0;
                        let stroke_color = if is_l2 {
                            egui::Color32::from_rgb(150, 150, 150)
                        } else {
                            egui::Color32::from_rgb(60, 60, 60)
                        };
                        let stroke_width = if is_l2 { 1.0 } else { 2.0 };

                        let (rect, _response) =
                            ui.allocate_exact_size(cell_size, egui::Sense::hover());
                        let rect = rect.shrink(spacing * 0.5);

                        ui.painter().rect(
                            rect,
                            4.0,
                            bg_color,
                            egui::Stroke::new(stroke_width, stroke_color),
                            StrokeKind::Middle,
                        );

                        if is_l2 {
                            let inner = rect.shrink(2.0);
                            ui.painter().rect_stroke(
                                inner,
                                3.0,
                                egui::Stroke::new(0.5, egui::Color32::from_rgb(120, 120, 120)),
                                StrokeKind::Middle,
                            );
                        }

                        let text_color = if bg_color.r() as u32
                            + bg_color.g() as u32
                            + bg_color.b() as u32
                            > 400
                        {
                            egui::Color32::BLACK
                        } else {
                            egui::Color32::WHITE
                        };

                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            ch.to_string(),
                            egui::FontId::proportional(16.0),
                            text_color,
                        );
                    }
                });
            }

            ui.add_space(6.0);
        }
    }

    pub fn draw_log(&self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("検索ログ").strong().size(14.0));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.log_buffer.as_str())
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY),
                );
            });
    }

    pub fn draw_finger_load(&self, ui: &mut egui::Ui) {
        let Some(ref upd) = self.latest_update else {
            ui.label("探索を開始してください");
            return;
        };
        let layout = &upd.best_layout;
        let kp = layout.kp;

        let mut finger_load = [0.0f64; 8];
        for c in 0..kp.num_chars as CharId {
            if c >= VOID_CHAR_FIRST {
                continue;
            }
            let freq = upd.unigrams[c as usize];
            if freq == 0.0 {
                continue;
            }
            let slot = layout.char_to_slot[c as usize];
            let ks = keystrokes_for_slot(slot, kp);
            for &s in ks.as_slice() {
                let finger = col_to_finger(slot_col(s, kp.num_cols)) as usize;
                finger_load[finger] += freq;
            }
        }

        // プリセット有効時: シフト→かな→゛/゜ のシフト打鍵省略分を差し引く
        if let (Some(ref corpus), Some(ref weights)) = (&self.corpus, &self.weights) {
            let omit = compute_shift_omit(layout, corpus, weights);
            let left_finger = col_to_finger(slot_col(kp.shift_left, kp.num_cols)) as usize;
            let right_finger = col_to_finger(slot_col(kp.shift_right, kp.num_cols)) as usize;
            finger_load[left_finger] = (finger_load[left_finger] - omit[0]).max(0.0);
            finger_load[right_finger] = (finger_load[right_finger] - omit[1]).max(0.0);
        }

        let finger_names = [
            "左小", "左薬", "左中", "左人", "右人", "右中", "右薬", "右小",
        ];
        let max_load = finger_load
            .iter()
            .cloned()
            .fold(0.0f64, f64::max)
            .max(1e-10);

        ui.label(egui::RichText::new("指負荷バランス").strong().size(14.0));
        ui.add_space(8.0);

        let bar_max_width = (ui.available_width() - 80.0).max(100.0);

        for (i, &load) in finger_load.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(finger_names[i]).monospace());
                let ratio = load / max_load;
                let bar_width = (ratio * bar_max_width as f64) as f32;

                let color = if i < 4 {
                    egui::Color32::from_rgb(70, 130, 200)
                } else {
                    egui::Color32::from_rgb(70, 180, 120)
                };

                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(bar_max_width, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(bar_width, 18.0)),
                    3.0,
                    color,
                );
                ui.painter().text(
                    rect.min + egui::vec2(bar_width + 4.0, 9.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{:.1}%", load * 100.0),
                    egui::FontId::proportional(11.0),
                    egui::Color32::GRAY,
                );
            });
        }
    }

    pub fn draw_score_graph(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("スコア推移").strong().size(14.0));

        if self.score_history.is_empty() {
            ui.label("データなし");
            return;
        }

        let current_points: PlotPoints = self.score_history.iter().map(|&(x, y)| [x, y]).collect();
        let best_points: PlotPoints = self.best_history.iter().map(|&(x, y)| [x, y]).collect();

        let current_line = Line::new(current_points)
            .name("current")
            .color(egui::Color32::from_rgba_premultiplied(150, 150, 200, 120))
            .width(1.0);
        let best_line = Line::new(best_points)
            .name("best")
            .color(egui::Color32::from_rgb(50, 120, 220))
            .width(2.5);

        let window_width = 10_000.0;
        let max_iter = self.score_history.last().map(|&(x, _)| x).unwrap_or(0.0);

        // 500iter 以下はスライディングウィンドウではなく全データを表示
        let warmup = max_iter <= 500.0;
        let min_iter = if warmup { 0.0 } else { (max_iter - window_width).max(0.0) };

        let (mut y_min, mut y_max) = (f64::MAX, f64::MIN);
        for &(x, y) in self.score_history.iter().chain(self.best_history.iter()) {
            if x >= min_iter && x <= max_iter {
                y_min = y_min.min(y);
                y_max = y_max.max(y);
            }
        }
        if y_min >= y_max {
            y_min = 0.0;
            y_max = 1.0;
        }
        let y_margin = (y_max - y_min) * 0.05;

        let follow = self.graph_follow;

        let resp = egui_plot::Plot::new("score_plot")
            .legend(egui_plot::Legend::default())
            .x_axis_label("iteration")
            .y_axis_label("score")
            .allow_drag(true)
            .allow_zoom(true)
            .allow_scroll(true)
            .grid_spacing(egui::Rangef::new(80.0, 200.0))
            .show(ui, |plot_ui| {
                plot_ui.line(current_line);
                plot_ui.line(best_line);
                for &restart_iter in &self.restart_iters {
                    plot_ui.vline(
                        VLine::new(restart_iter)
                            .color(egui::Color32::from_rgba_premultiplied(220, 80, 80, 100))
                            .width(1.0),
                    );
                }
                // ウォームアップ中は毎フレーム範囲を再設定
                // ウォームアップ後は自動追従モード時のみ再設定
                if warmup || follow {
                    plot_ui.set_plot_bounds(egui_plot::PlotBounds::from_min_max(
                        [min_iter, y_min - y_margin],
                        [max_iter + window_width * 0.02, y_max + y_margin],
                    ));
                }
            });

        // ドラッグ・スクロール操作で自動追従を解除、ダブルクリックで復帰
        if resp.response.dragged() || resp.response.hovered() && ui.input(|i| i.smooth_scroll_delta.length() > 0.0) {
            self.graph_follow = false;
        }
        if resp.response.double_clicked() {
            self.graph_follow = true;
        }
    }

    pub fn draw_score_info(&self, ui: &mut egui::Ui) {
        let Some(ref upd) = self.latest_update else {
            return;
        };

        ui.label(egui::RichText::new("スコア情報").strong().size(14.0));
        ui.horizontal(|ui| {
            ui.label(format!("最良スコア: {:.4}", upd.best_score));
            ui.separator();
            ui.label(format!("現在スコア: {:.4}", upd.current_score));
            if let Some(init) = self.initial_score {
                if init > 0.0 {
                    let improvement = (init - upd.best_score) / init * 100.0;
                    ui.separator();
                    ui.label(format!("改善率: {:.2}%", improvement));
                }
            }
            ui.separator();
            ui.label(format!("イテレーション: {}", upd.iter));
            ui.separator();
            ui.label(format!("再起動回数: {}", upd.restarts));
            ui.separator();
            if ui.button("配列をコピー").clicked() {
                let mut buf = Vec::new();
                upd.best_layout.display(&mut buf);
                if let Ok(text) = String::from_utf8(buf) {
                    ui.ctx().copy_text(text);
                }
            }
        });

        // スコア内訳パネル
        if let (Some(ref corpus), Some(ref weights)) = (&self.corpus, &self.weights) {
            let bd = score_breakdown_data(&upd.best_layout, corpus, weights);
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("スコア内訳（最良解）")
                    .strong()
                    .size(14.0),
            );
            egui::Grid::new("breakdown_grid")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.label("打鍵数コスト:");
                    ui.label(format!(
                        "{:.4}  （平均打鍵数 {:.4}, 1打鍵カバー率 {:.1}%）",
                        bd.stroke_cost,
                        bd.total_strokes,
                        bd.l1_coverage * 100.0
                    ));
                    ui.end_row();

                    ui.label("難易度コスト:");
                    ui.label(format!("{:.4}", bd.uni_cost));
                    ui.end_row();

                    ui.label("バイグラムコスト:");
                    ui.label(format!("{:.4}", bd.bi_cost));
                    ui.end_row();

                    ui.label("準交互ボーナス:");
                    ui.label(format!("{:.4}", bd.tri_cost));
                    ui.end_row();

                    ui.label("合計スコア:");
                    ui.label(egui::RichText::new(format!("{:.4}", bd.total)).strong());
                    ui.end_row();
                });
        }
    }

    /// 色分けモードが変わったときにキャッシュを無効化する
    pub fn invalidate_color_cache(&mut self) {
        self.cached_color_data = None;
    }
}

// ──────────────────────────────────────────────────────────────
// 色分けヘルパー（フリー関数）
// ──────────────────────────────────────────────────────────────

/// 色分けに必要な事前計算データ
fn precompute_color_data(
    color_mode: ColorMode,
    upd: &SearchUpdate,
    corpus: Option<&Corpus>,
    weights: Option<&Weights>,
) -> ColorData {
    let layout = &upd.best_layout;
    let kp = layout.kp;
    let nc = kp.num_chars;

    match color_mode {
        ColorMode::Fitness => {
            let mut freq_sorted: Vec<(CharId, f64)> = (0..nc as CharId)
                .filter(|&c| c < VOID_CHAR_FIRST)
                .map(|c| (c, upd.unigrams[c as usize]))
                .collect();
            freq_sorted.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
            let mut freq_rank = [0u8; MAX_CHARS];
            for (rank, &(c, _)) in freq_sorted.iter().enumerate() {
                freq_rank[c as usize] = rank as u8;
            }

            let mut slot_sorted: Vec<(u8, f64)> = (0..kp.num_slots as u8)
                .filter(|&s| layout.slot_to_char[s as usize] != SHIFT_SLOT_SENTINEL)
                .map(|s| {
                    let physical = if (s as usize) < kp.num_slots_per_layer as usize {
                        s
                    } else {
                        s - kp.num_slots_per_layer
                    };
                    let r =
                        (physical as usize % (kp.num_cols as usize * 3)) / kp.num_cols as usize;
                    let c = slot_col(physical, kp.num_cols) as usize;
                    let l2_penalty = if (s as usize) >= kp.num_slots_per_layer as usize {
                        3.0
                    } else {
                        0.0
                    };
                    let row_d = [1.3, 0.9, 1.5][r];
                    let center = (kp.num_cols as f64 - 1.0) / 2.0;
                    let col_d = ((c as f64 - center).abs() / center) * 0.8;
                    (s, row_d + col_d + l2_penalty)
                })
                .collect();
            slot_sorted.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
            let mut slot_rank = [0u8; 66]; // MAX_SLOTS
            for (rank, &(s, _)) in slot_sorted.iter().enumerate() {
                slot_rank[s as usize] = rank as u8;
            }

            ColorData::Fitness {
                freq_rank,
                slot_rank,
                num_valid: freq_sorted.len() as f32,
            }
        }
        ColorMode::Frequency => {
            // L2文字のシフト打鍵頻度を集計
            let mut shift_freq = [0.0f64; 2];
            for c in 0..kp.num_chars as CharId {
                if c >= VOID_CHAR_FIRST {
                    continue;
                }
                let freq = upd.unigrams[c as usize];
                if freq == 0.0 {
                    continue;
                }
                let slot = layout.char_to_slot[c as usize];
                if (slot as usize) >= kp.num_slots_per_layer as usize {
                    // L2文字: 左手域→shift_right(idx=1)、右手域→shift_left(idx=0)
                    let physical = slot - kp.num_slots_per_layer;
                    if slot_hand(physical, kp.num_cols) == Hand::Left {
                        shift_freq[1] += freq; // shift_right
                    } else {
                        shift_freq[0] += freq; // shift_left
                    }
                }
            }
            // プリセット有効時: シフト→かな→゛/゜ のシフト打鍵省略分を差し引く
            if let (Some(corpus), Some(weights)) = (corpus, weights) {
                let omit = compute_shift_omit(layout, corpus, weights);
                shift_freq[0] = (shift_freq[0] - omit[0]).max(0.0);
                shift_freq[1] = (shift_freq[1] - omit[1]).max(0.0);
            }
            let max_freq = upd
                .unigrams
                .iter()
                .cloned()
                .fold(0.0f64, f64::max)
                .max(shift_freq[0])
                .max(shift_freq[1])
                .max(1e-10);
            ColorData::Frequency {
                max_freq,
                shift_freq,
            }
        }
        ColorMode::FingerLoad | ColorMode::Log => ColorData::None,
    }
}

/// シフトキー頻度から色を計算（ヒートマップモード用）
fn shift_slot_color(freq: f64, max_freq: f64) -> egui::Color32 {
    let ratio = (freq / max_freq).min(1.0);
    let t = ((1.0 + ratio * 99.0).ln() / 100.0f64.ln()) as f32;
    if t < 0.5 {
        let s = t * 2.0;
        egui::Color32::from_rgb(
            (100.0 + s * 100.0) as u8,
            (140.0 - s * 60.0) as u8,
            (220.0 - s * 40.0) as u8,
        )
    } else {
        let s = (t - 0.5) * 2.0;
        egui::Color32::from_rgb(
            (200.0 + s * 55.0) as u8,
            (80.0 + s * 40.0) as u8,
            (180.0 - s * 160.0) as u8,
        )
    }
}

/// 1文字の色を計算（extra_freq: シフト打鍵等の追加頻度）
fn char_color(
    char_id: CharId,
    slot: u8,
    unigrams: &[f64; MAX_CHARS],
    data: &ColorData,
    extra_freq: f64,
) -> egui::Color32 {
    match data {
        ColorData::Fitness {
            freq_rank,
            slot_rank,
            num_valid,
        } => {
            let fr = freq_rank[char_id as usize] as f32;
            let sr = slot_rank[slot as usize] as f32;
            let mismatch = (fr - sr).abs() / (num_valid * 0.3);
            let t = mismatch.min(1.0);
            if t < 0.5 {
                let s = t * 2.0;
                egui::Color32::from_rgb(
                    (46.0 + s * 209.0) as u8,
                    (160.0 + s * 95.0) as u8,
                    (67.0 - s * 67.0) as u8,
                )
            } else {
                let s = (t - 0.5) * 2.0;
                egui::Color32::from_rgb((255.0 - s * 35.0) as u8, (255.0 - s * 205.0) as u8, 0)
            }
        }
        ColorData::Frequency { max_freq, .. } => {
            let freq = unigrams[char_id as usize] + extra_freq;
            let ratio = (freq / max_freq).min(1.0);
            let t = ((1.0 + ratio * 99.0).ln() / 100.0f64.ln()) as f32;
            if t < 0.5 {
                let s = t * 2.0;
                egui::Color32::from_rgb(
                    (100.0 + s * 100.0) as u8,
                    (140.0 - s * 60.0) as u8,
                    (220.0 - s * 40.0) as u8,
                )
            } else {
                let s = (t - 0.5) * 2.0;
                egui::Color32::from_rgb(
                    (200.0 + s * 55.0) as u8,
                    (80.0 + s * 40.0) as u8,
                    (180.0 - s * 160.0) as u8,
                )
            }
        }
        ColorData::None => egui::Color32::from_rgb(220, 220, 220),
    }
}

/// コーパスから特定バイグラム (c1, c2) の頻度を検索する
fn lookup_bigram_freq(corpus: &Corpus, c1: CharId, c2: CharId) -> f64 {
    for &idx in &corpus.bigram_adj[c1 as usize] {
        let bg = &corpus.bigrams[idx];
        if bg.c1 == c1 && bg.c2 == c2 {
            return bg.freq;
        }
    }
    0.0
}

/// プリセット有効時にシフト打鍵が省略される頻度をシフトキー別に返す
/// [0] = shift_left の省略分, [1] = shift_right の省略分
fn compute_shift_omit(
    layout: &tsuki_optimize::layout::Layout,
    corpus: &Corpus,
    weights: &Weights,
) -> [f64; 2] {
    let kp = layout.kp;
    let mut omit = [0.0f64; 2];
    for c in 0..kp.num_chars as CharId {
        if c >= VOID_CHAR_FIRST {
            continue;
        }
        let slot = layout.char_to_slot[c as usize];
        if (slot as usize) < kp.num_slots_per_layer as usize {
            continue; // L1 はシフト不要
        }
        let physical = slot - kp.num_slots_per_layer;
        let shift_idx = if slot_hand(physical, kp.num_cols) == Hand::Left {
            1 // 左手文字 → shift_right
        } else {
            0 // 右手文字 → shift_left
        };
        if weights.daku_l2_trigger[c as usize] {
            omit[shift_idx] += lookup_bigram_freq(corpus, c, DAKUTEN_ID);
        }
        if weights.handaku_l2_trigger[c as usize] {
            omit[shift_idx] += lookup_bigram_freq(corpus, c, HANDAKUTEN_ID);
        }
    }
    omit
}

// gui.rs — tsuki_optimize GUI エントリポイント
//
// eframe (egui) を使用し、最適化の進行をリアルタイム表示する。

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use eframe::egui;
use egui::epaint::StrokeKind;
use egui_plot::{Line, PlotPoints, VLine};

use tsuki_optimize::chars::{CharId, CHAR_LIST, MAX_CHARS, VOID_CHAR_FIRST};
use tsuki_optimize::config::Config;
use tsuki_optimize::corpus::Corpus;
use tsuki_optimize::cost::{score_breakdown_data, Weights};
use tsuki_optimize::layout::{
    col_to_finger, slot_col, KeyboardParams, KeyboardSize, SHIFT_SLOT_SENTINEL,
};
use tsuki_optimize::search::{self, SearchContext, SearchPhase, SearchUpdate};

// ──────────────────────────────────────────────────────────────
// メイン
// ──────────────────────────────────────────────────────────────
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
            Ok(Box::new(App::new()))
        }),
    )
}

/// 日本語フォント（IPAゴシック）をバイナリに埋め込み、egui に登録する
fn setup_japanese_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "ipag".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/ipag.ttf"
        ))),
    );

    // Proportional / Monospace の両方で日本語フォントをフォールバックに追加
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

// ──────────────────────────────────────────────────────────────
// 色分けモード
// ──────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorMode {
    Fitness,
    Frequency,
    FingerLoad,
    Log,
}

// ──────────────────────────────────────────────────────────────
// アプリケーション状態
// ──────────────────────────────────────────────────────────────
struct App {
    // 設定入力
    seed_str: String,
    iter_str: String,
    restart_str: String,
    corpus_path_str: String,
    keyboard_size_str_input: String,

    // 探索スレッド制御
    stop_flag: Arc<AtomicBool>,
    rx: Option<mpsc::Receiver<SearchUpdate>>,
    running: bool,

    // 最新の探索状態
    latest_update: Option<SearchUpdate>,

    // スコア内訳表示用（探索開始時にコピーを保持）
    corpus: Option<Corpus>,
    weights: Option<Weights>,

    // ログ表示用
    log_rx: Option<mpsc::Receiver<String>>,
    log_buffer: String,

    // スコア推移グラフ用データ
    score_history: Vec<(f64, f64)>, // (iter, current_score)
    best_history: Vec<(f64, f64)>,  // (iter, best_score)
    restart_iters: Vec<f64>,        // リスタート発生イテレーション

    // 表示設定
    color_mode: ColorMode,
    show_layer2: bool,
}

impl App {
    fn new() -> Self {
        App {
            seed_str: String::new(),
            iter_str: "50000".to_string(),
            restart_str: "3000".to_string(),
            corpus_path_str: "corpus.txt".to_string(),
            keyboard_size_str_input: "3x10".to_string(),
            stop_flag: Arc::new(AtomicBool::new(false)),
            rx: None,
            running: false,
            latest_update: None,
            corpus: None,
            weights: None,
            log_rx: None,
            log_buffer: String::new(),
            score_history: Vec::new(),
            best_history: Vec::new(),
            restart_iters: Vec::new(),
            color_mode: ColorMode::Fitness,
            show_layer2: false,
        }
    }

    fn start_search(&mut self) {
        // 設定読み込み
        let config_path = Path::new("config.toml");
        let toml_config = if config_path.exists() {
            Config::from_file(config_path).unwrap_or_default()
        } else {
            Config::default()
        };

        let kp = match self.keyboard_size_str_input.as_str() {
            "3x11" => KeyboardParams::k3x11(),
            _ => KeyboardParams::k3x10(),
        };

        let exclusive_pairs = toml_config.build_exclusive_pairs();
        let mut search_config = toml_config.build_search_config();
        let weights = toml_config.build_weights(kp);

        if let Ok(v) = self.iter_str.parse() {
            search_config.max_iter = v;
        }
        if let Ok(v) = self.restart_str.parse() {
            search_config.restart_after = v;
        }

        let seed: u64 = if self.seed_str.is_empty() {
            rand::random()
        } else {
            self.seed_str.parse().unwrap_or_else(|_| rand::random())
        };

        let corpus_path = &self.corpus_path_str;
        let corpus = if Path::new(corpus_path).exists() {
            Corpus::from_file(Path::new(corpus_path))
                .unwrap_or_else(|_| Corpus::from_str(SAMPLE_CORPUS))
        } else {
            Corpus::from_str(SAMPLE_CORPUS)
        };

        // GUI側でスコア内訳計算用にコピーを保持
        self.corpus = Some(corpus.clone());
        self.weights = Some(weights.clone());

        // 状態リセット
        self.score_history.clear();
        self.best_history.clear();
        self.restart_iters.clear();
        self.latest_update = None;
        self.log_buffer.clear();
        self.stop_flag.store(false, Ordering::Relaxed);
        self.running = true;

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);

        // ログ用チャネル
        let (log_tx, log_rx) = mpsc::channel();
        self.log_rx = Some(log_rx);

        // ログファイル作成
        let log_path = format!("log/{}.log", tsuki_optimize::local_timestamp());
        let log_file = {
            if let Some(parent) = Path::new(&log_path).parent() {
                std::fs::create_dir_all(parent).ok();
            }
            match File::create(&log_path) {
                Ok(f) => Some(BufWriter::new(f)),
                Err(_) => None,
            }
        };

        let stop_flag = Arc::clone(&self.stop_flag);

        std::thread::spawn(move || {
            use rand::rngs::SmallRng;
            use rand::SeedableRng;

            let mut log_writer = GuiLogWriter {
                tx: log_tx,
                file: log_file,
            };

            let mut rng = SmallRng::seed_from_u64(seed);
            let ctx = SearchContext {
                corpus: &corpus,
                weights: &weights,
                pairs: &exclusive_pairs,
            };

            let initial = search::build_initial_layout(&ctx, kp, &mut log_writer);
            let report_flag = Arc::new(AtomicBool::new(false));

            search::run(
                initial,
                &ctx,
                &search_config,
                &mut rng,
                &stop_flag,
                &report_flag,
                &mut move |update: &SearchUpdate| {
                    let _ = tx.send(update.clone());
                },
                &mut log_writer,
            );
        });
    }

    fn stop_search(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }

    fn poll_updates(&mut self) {
        if let Some(ref rx) = self.rx {
            loop {
                match rx.try_recv() {
                    Ok(update) => {
                        let iter = update.iter as f64;
                        self.score_history.push((iter, update.current_score));
                        self.best_history.push((iter, update.best_score));
                        if update.phase == SearchPhase::Restarting {
                            self.restart_iters.push(iter);
                        }
                        if update.phase == SearchPhase::Finished {
                            self.running = false;
                        }
                        self.latest_update = Some(update);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        // Search thread exited (finished or panicked)
                        self.running = false;
                        break;
                    }
                }
            }
        }

        // ログメッセージを drain
        if let Some(ref log_rx) = self.log_rx {
            while let Ok(text) = log_rx.try_recv() {
                self.log_buffer.push_str(&text);
            }
            // メモリ上限（512KB）を超えたら先頭からトリミング
            const MAX_LOG_SIZE: usize = 512 * 1024;
            if self.log_buffer.len() > MAX_LOG_SIZE {
                let trim_at = self.log_buffer.len() - MAX_LOG_SIZE;
                if let Some(newline_pos) = self.log_buffer[trim_at..].find('\n') {
                    self.log_buffer.drain(..trim_at + newline_pos + 1);
                }
            }
        }
    }
}

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
                ui.add(egui::TextEdit::singleline(&mut self.corpus_path_str).desired_width(120.0));
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

// ──────────────────────────────────────────────────────────────
// 描画ヘルパー
// ──────────────────────────────────────────────────────────────
impl App {
    fn draw_keyboard(&self, ui: &mut egui::Ui) {
        let Some(ref upd) = self.latest_update else {
            ui.label("探索を開始してください");
            return;
        };
        let layout = &upd.best_layout;
        let kp = layout.kp;
        let nc = kp.num_cols as usize;
        let npl = kp.num_slots_per_layer as usize;

        // 色分けに必要な事前計算
        let color_data = self.precompute_color_data(upd);

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

                        // シフトキースロット
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
                            egui::Color32::from_rgb(160, 160, 160)
                        } else if char_id == SHIFT_SLOT_SENTINEL || char_id >= VOID_CHAR_FIRST {
                            egui::Color32::from_rgb(200, 200, 200)
                        } else {
                            self.char_color(char_id, &color_data)
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

                        // 破線（L2）の表現: 内側に点線的な2本目の枠
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

    fn draw_log(&self, ui: &mut egui::Ui) {
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

    fn draw_finger_load(&self, ui: &mut egui::Ui) {
        let Some(ref upd) = self.latest_update else {
            ui.label("探索を開始してください");
            return;
        };
        let layout = &upd.best_layout;
        let kp = layout.kp;

        // 指別負荷を計算（実際のコーパス頻度を使用）
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
            let physical = if (slot as usize) < kp.num_slots_per_layer as usize {
                slot
            } else {
                slot - kp.num_slots_per_layer
            };
            let finger = col_to_finger(slot_col(physical, kp.num_cols)) as usize;
            finger_load[finger] += freq;
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
                    // 左手: 青系
                    egui::Color32::from_rgb(70, 130, 200)
                } else {
                    // 右手: 緑系
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

    fn draw_score_graph(&self, ui: &mut egui::Ui) {
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

        egui_plot::Plot::new("score_plot")
            .legend(egui_plot::Legend::default())
            .x_axis_label("iteration")
            .y_axis_label("score")
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
            });
    }

    fn draw_score_info(&self, ui: &mut egui::Ui) {
        let Some(ref upd) = self.latest_update else {
            return;
        };

        ui.label(egui::RichText::new("スコア情報").strong().size(14.0));
        ui.horizontal(|ui| {
            ui.label(format!("最良スコア: {:.4}", upd.best_score));
            ui.separator();
            ui.label(format!("現在スコア: {:.4}", upd.current_score));
            ui.separator();
            ui.label(format!("イテレーション: {}", upd.iter));
            ui.separator();
            ui.label(format!("再起動回数: {}", upd.restarts));
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

    // ── 色分けヘルパー ──

    /// 色分けに必要な事前計算データ
    fn precompute_color_data(&self, upd: &SearchUpdate) -> ColorData {
        let layout = &upd.best_layout;
        let kp = layout.kp;
        let nc = kp.num_chars;

        match self.color_mode {
            ColorMode::Fitness => {
                // 頻度ランク（降順: 最頻出=0）と難易度ランク（昇順: 最も打ちやすい=0）を計算
                let mut freq_sorted: Vec<(CharId, f64)> = (0..nc as CharId)
                    .filter(|&c| c < VOID_CHAR_FIRST)
                    .map(|c| (c, upd.unigrams[c as usize]))
                    .collect();
                freq_sorted.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
                let mut freq_rank = [0u8; MAX_CHARS];
                for (rank, &(c, _)) in freq_sorted.iter().enumerate() {
                    freq_rank[c as usize] = rank as u8;
                }

                // スロット難易度ランク: 設計書では Weights.slot_difficulty を使う想定だが、
                // slot_difficulty はスカラー配列でなく [row][col] の2次元構造であり、
                // L2 スロットのペナルティも含めた総合的な順位付けが必要なため、
                // ここでは行・列・レイヤーに基づく近似値で代用する。
                // 実用上、ホーム段中央 > 上段 > 下段 > L2 の順序は slot_difficulty と一致する。
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
                        // L2スロットは追加ペナルティ（2打鍵なので打ちにくい）
                        let l2_penalty = if (s as usize) >= kp.num_slots_per_layer as usize {
                            3.0
                        } else {
                            0.0
                        };
                        // 行・列ベースの簡易難易度
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
                let max_freq = upd
                    .unigrams
                    .iter()
                    .cloned()
                    .fold(0.0f64, f64::max)
                    .max(1e-10);
                ColorData::Frequency { max_freq }
            }
            ColorMode::FingerLoad | ColorMode::Log => ColorData::None,
        }
    }

    /// 1文字の色を計算
    fn char_color(&self, char_id: CharId, data: &ColorData) -> egui::Color32 {
        match data {
            ColorData::Fitness {
                freq_rank,
                slot_rank,
                num_valid,
            } => {
                let upd = self.latest_update.as_ref().unwrap();
                let slot = upd.best_layout.char_to_slot[char_id as usize];
                let fr = freq_rank[char_id as usize] as f32;
                let sr = slot_rank[slot as usize] as f32;
                // ズレ = |頻度ランク - 難易度ランク| / 有効文字数
                let mismatch = (fr - sr).abs() / num_valid;
                // 0.0（良い配置）→ 緑, 0.5 → 黄, 1.0（悪い配置）→ 赤
                let t = mismatch.min(1.0);
                if t < 0.5 {
                    // 緑 → 黄
                    let s = t * 2.0;
                    egui::Color32::from_rgb(
                        (46.0 + s * 209.0) as u8,
                        (160.0 + s * 95.0) as u8,
                        (67.0 - s * 67.0) as u8,
                    )
                } else {
                    // 黄 → 赤
                    let s = (t - 0.5) * 2.0;
                    egui::Color32::from_rgb((255.0 - s * 35.0) as u8, (255.0 - s * 205.0) as u8, 0)
                }
            }
            ColorData::Frequency { max_freq } => {
                let upd = self.latest_update.as_ref().unwrap();
                let freq = upd.unigrams[char_id as usize];
                let t = (freq / max_freq).min(1.0) as f32;
                // 低頻度（青紫）→ 高頻度（赤オレンジ）
                egui::Color32::from_rgb(
                    (80.0 + t * 175.0) as u8,
                    (100.0 + t * 60.0 - t * t * 120.0) as u8,
                    (220.0 - t * 200.0) as u8,
                )
            }
            ColorData::None => egui::Color32::from_rgb(220, 220, 220),
        }
    }
}

/// 色分けモードの事前計算データ
enum ColorData {
    Fitness {
        freq_rank: [u8; MAX_CHARS],
        slot_rank: [u8; 66],
        num_valid: f32,
    },
    Frequency {
        max_freq: f64,
    },
    None,
}

// ──────────────────────────────────────────────────────────────
// GuiLogWriter: ログテキストを GUI チャネル + ファイルに書き込む
// ──────────────────────────────────────────────────────────────
struct GuiLogWriter {
    tx: mpsc::Sender<String>,
    file: Option<BufWriter<File>>,
}

impl Write for GuiLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let _ = self.tx.send(text.into_owned());
        if let Some(ref mut f) = self.file {
            let _ = f.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(ref mut f) = self.file {
            let _ = f.flush();
        }
        Ok(())
    }
}

const SAMPLE_CORPUS: &str = "\
こんにちは。今日はいい天気ですね。\
日本語入力の配列を最適化するためのプログラムです。\
タブーサーチを用いて月配列の改変版を探索します。\
かな文字の打鍵数と難易度を評価して最良の配置を求めます。\
てにをはなどの助詞や、よく使う動詞・形容詞が打ちやすくなるように配置します。\
";

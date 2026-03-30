use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use tsuki_optimize::config::Config;
use tsuki_optimize::corpus::Corpus;
use tsuki_optimize::cost::{score, Weights};
use tsuki_optimize::layout::KeyboardParams;
use tsuki_optimize::search::{self, SearchContext, SearchPhase, SearchUpdate};

use super::log_writer::{ColorData, ColorMode, GuiLogWriter};
use super::SAMPLE_CORPUS;

// ──────────────────────────────────────────────────────────────
// アプリケーション状態
// ──────────────────────────────────────────────────────────────
pub struct App {
    // 設定入力
    pub seed_str: String,
    pub iter_str: String,
    pub restart_str: String,
    pub corpus_path_str: String,
    pub keyboard_size_str_input: String,

    // 探索スレッド制御
    pub stop_flag: Arc<AtomicBool>,
    pub rx: Option<mpsc::Receiver<SearchUpdate>>,
    pub running: bool,

    // 最新の探索状態
    pub latest_update: Option<SearchUpdate>,
    pub initial_score: Option<f64>,

    // スコア内訳表示用（探索開始時にコピーを保持）
    pub corpus: Option<Corpus>,
    pub weights: Option<Weights>,

    // ログ表示用
    pub log_rx: Option<mpsc::Receiver<String>>,
    pub log_buffer: String,

    // スコア推移グラフ用データ
    pub score_history: Vec<(f64, f64)>, // (iter, current_score)
    pub best_history: Vec<(f64, f64)>,  // (iter, best_score)
    pub restart_iters: Vec<f64>,        // リスタート発生イテレーション

    // 表示設定
    pub color_mode: ColorMode,
    pub show_layer2: bool,

    // 色分けキャッシュ（latest_update 更新時にリセット）
    pub cached_color_data: Option<ColorData>,

    // 設定ファイルエラー
    pub config_error: Option<String>,
}

impl App {
    pub fn new() -> Self {
        // config.toml があれば読み込み、GUI の初期値に反映する
        let config_path = Path::new("config.toml");
        let toml_config = if config_path.exists() {
            Config::from_file(config_path).unwrap_or_default()
        } else {
            Config::default()
        };
        let search_config = toml_config.build_search_config();
        let corpus_path = toml_config.corpus_path(None);
        let keyboard_size = toml_config
            .run
            .keyboard_size
            .as_deref()
            .unwrap_or("3x10")
            .to_string();

        App {
            seed_str: String::new(),
            iter_str: search_config.max_iter.to_string(),
            restart_str: search_config.restart_after.to_string(),
            corpus_path_str: corpus_path,
            keyboard_size_str_input: keyboard_size,
            stop_flag: Arc::new(AtomicBool::new(false)),
            rx: None,
            running: false,
            latest_update: None,
            initial_score: None,
            corpus: None,
            weights: None,
            log_rx: None,
            log_buffer: String::new(),
            score_history: Vec::new(),
            best_history: Vec::new(),
            restart_iters: Vec::new(),
            color_mode: ColorMode::Fitness,
            show_layer2: false,
            cached_color_data: None,
            config_error: None,
        }
    }

    pub fn start_search(&mut self) {
        self.config_error = None;

        // 設定読み込み
        let config_path = Path::new("config.toml");
        let toml_config = if config_path.exists() {
            match Config::from_file(config_path) {
                Ok(c) => c,
                Err(e) => {
                    self.config_error = Some(e);
                    return;
                }
            }
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

        let corpus_path = self.corpus_path_str.clone();
        let corpus = if Path::new(&corpus_path).exists() {
            Corpus::from_file(Path::new(&corpus_path))
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
        self.initial_score = None;
        self.cached_color_data = None;
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
            let l1_only = toml_config.build_l1_only_set();
            let ctx = SearchContext {
                corpus: &corpus,
                weights: &weights,
                pairs: &exclusive_pairs,
                l1_only: &l1_only,
            };

            // 設定サマリー出力
            tsuki_optimize::write_config_summary(
                &mut log_writer,
                &kp,
                &corpus_path,
                seed,
                &search_config,
                &weights,
                &toml_config,
                &exclusive_pairs,
            );

            let initial = search::build_initial_layout(&ctx, kp, &mut log_writer);
            let initial_score = score(&initial, &corpus, &weights);
            tsuki_optimize::write_initial_layout(&mut log_writer, &initial, &corpus, &weights);

            let report_flag = Arc::new(AtomicBool::new(false));

            let best_layout = search::run(
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

            // 最終結果をログに出力
            tsuki_optimize::write_final_result(
                &mut log_writer,
                &best_layout,
                &corpus,
                &weights,
                initial_score,
            );
            let _ = log_writer.flush();
        });
    }

    pub fn stop_search(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }

    pub fn poll_updates(&mut self) {
        if let Some(ref rx) = self.rx {
            loop {
                match rx.try_recv() {
                    Ok(update) => {
                        let iter = update.iter as f64;
                        self.score_history.push((iter, update.current_score));
                        self.best_history.push((iter, update.best_score));
                        if self.initial_score.is_none() {
                            self.initial_score = Some(update.current_score);
                        }
                        if update.phase == SearchPhase::Restarting {
                            self.restart_iters.push(iter);
                        }
                        if update.phase == SearchPhase::Finished {
                            self.running = false;
                        }
                        self.latest_update = Some(update);
                        self.cached_color_data = None;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
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

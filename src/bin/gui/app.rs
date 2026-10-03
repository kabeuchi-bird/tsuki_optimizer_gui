use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use tsuki_optimize::config::{keyboard_params_from_str, Config};
use tsuki_optimize::corpus::Corpus;
use tsuki_optimize::cost::Weights;
use tsuki_optimize::search::{self, SearchPhase, SearchUpdate};
use tsuki_optimize::yoon::{YoonMode, YoonSetup};

use super::log_writer::{ChannelWriter, ColorData, ColorMode};

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
    pub initial_layout_str_input: String,
    /// 拗音方式: "none" / "hybrid"
    pub yoon_mode_str_input: String,
    /// 子音セット（空欄でデフォルト11種）
    pub consonants_str_input: String,

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
    /// 拗音面（第3層）を表示するか。hybrid で探索したときのみ意味を持つ。
    pub show_yoon: bool,

    // 色分けキャッシュ（latest_update 更新時にリセット）
    pub cached_color_data: Option<ColorData>,

    // 設定ファイルエラー
    pub config_error: Option<String>,

    // グラフ: ユーザーが操作（ドラッグ/ズーム）したら自動追従を止める
    pub graph_follow: bool,
}

impl App {
    pub fn new() -> Self {
        // config.toml があれば読み込み、GUI の初期値に反映する
        let (toml_config, config_error) = match Config::load_or_default(Path::new("config.toml")) {
            Ok(c) => (c, None),
            Err(e) => (
                Config::default(),
                Some(format!("config.toml 読み込みエラー（デフォルト値で起動）: {e}")),
            ),
        };
        let search_config = toml_config.build_search_config();
        let corpus_path = toml_config.corpus_path(None);
        let keyboard_size = toml_config
            .run
            .keyboard_size
            .as_deref()
            .unwrap_or("3x10")
            .to_string();
        let initial_layout = toml_config
            .run
            .initial_layout
            .as_deref()
            .unwrap_or("2-263")
            .to_string();
        let yoon_mode = toml_config
            .yoon
            .mode
            .as_deref()
            .unwrap_or("none")
            .to_string();
        let consonants = toml_config
            .yoon
            .consonants
            .as_deref()
            .unwrap_or("")
            .to_string();

        App {
            seed_str: String::new(),
            iter_str: search_config.max_iter.to_string(),
            restart_str: search_config.restart_after.to_string(),
            corpus_path_str: corpus_path,
            keyboard_size_str_input: keyboard_size,
            initial_layout_str_input: initial_layout,
            yoon_mode_str_input: yoon_mode,
            consonants_str_input: consonants,
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
            show_yoon: true,
            cached_color_data: None,
            config_error,
            graph_follow: true,
        }
    }

    pub fn start_search(&mut self) {
        self.config_error = None;
        if let Err(e) = self.try_start_search() {
            self.config_error = Some(e);
            self.running = false;
        }
    }

    fn try_start_search(&mut self) -> Result<(), String> {
        // 設定読み込み
        let toml_config = Config::load_or_default(Path::new("config.toml"))?;

        let kp = keyboard_params_from_str(&self.keyboard_size_str_input);

        // 拗音方式の解決（kp の拡張・コーパス分解テーブル・l1_only 追加を一括で得る）。
        // build_weights より前に kp を確定させる必要がある。
        let yoon_mode = YoonMode::from_config_str(&self.yoon_mode_str_input);
        let consonants = if self.consonants_str_input.trim().is_empty() {
            None
        } else {
            Some(self.consonants_str_input.trim())
        };
        let yoon = YoonSetup::resolve(kp, yoon_mode, consonants)?;
        let kp = yoon.kp;

        let exclusive_pairs = toml_config.build_exclusive_pairs();
        let mut search_config = toml_config.build_search_config();
        let weights = toml_config.build_weights(kp);

        // パラメータ入力欄のパース（空欄はデフォルト維持、不正値はエラー）
        if let Some(v) = parse_field("iter", &self.iter_str)? {
            search_config.max_iter = v;
        }
        if let Some(v) = parse_field("restart", &self.restart_str)? {
            search_config.restart_after = v;
        }

        search_config.initial_layout_mode =
            search::InitialLayoutMode::from_config_str(&self.initial_layout_str_input);

        let seed: u64 = parse_field("seed", &self.seed_str)?.unwrap_or_else(rand::random);

        let corpus_path = self.corpus_path_str.clone();
        let corpus = tsuki_optimize::load_corpus(&corpus_path, yoon.table.as_ref())?;

        // GUI側でスコア内訳計算用にコピーを保持
        self.corpus = Some(corpus.clone());
        self.weights = Some(weights.clone());

        // 状態リセット
        self.score_history.clear();
        self.best_history.clear();
        self.restart_iters.clear();
        self.graph_follow = true;
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
        let log_file = tsuki_optimize::create_log_file(&log_path)?;

        let stop_flag = Arc::clone(&self.stop_flag);
        let mut log_writer = tsuki_optimize::LogTee::new(
            ChannelWriter(log_tx),
            log_file,
            Arc::clone(&self.stop_flag),
        );
        let run = tsuki_optimize::Run {
            toml_config,
            yoon,
            search_config,
            weights,
            exclusive_pairs,
            corpus,
            corpus_path,
            seed,
        };

        std::thread::spawn(move || {
            // GUI には SIGUSR1 相当の中間報告がないので常に false
            let report_flag = Arc::new(AtomicBool::new(false));
            run.execute(&mut log_writer, &stop_flag, &report_flag, &mut |update: &SearchUpdate| {
                let _ = tx.send(update.clone());
            });
        });
        Ok(())
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

/// 入力欄をパースする。空欄は None（デフォルト維持）、不正値はエラーメッセージ
fn parse_field<T: FromStr>(name: &str, s: &str) -> Result<Option<T>, String>
where
    T::Err: std::fmt::Display,
{
    if s.is_empty() {
        return Ok(None);
    }
    s.parse()
        .map(Some)
        .map_err(|e| format!("{name} の値が不正です ('{s}'): {e}"))
}

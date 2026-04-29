// main.rs — tsuki_optimize エントリポイント
//
// 使い方:
//   cargo run --release -- [CLIオプション]
//
// 設定の優先順位（高 → 低）:
//   1. CLIオプション
//   2. TOMLファイル（--config で指定 or デフォルト: config.toml）
//   3. ハードコードされたデフォルト値
//
// CLIオプション:
//   --config        <path>  設定ファイルのパス       (default: config.toml)
//   --corpus        <path>  コーパスファイルパス     (toml: run.corpus)
//   --seed          <n>     乱数シード               (toml: run.seed)
//   --iter          <n>     最大イテレーション数     (toml: run.max_iter)
//   --restart       <n>     再起動閾値               (toml: run.restart_after)
//   --max-restarts  <n>     最大再起動回数           (toml: run.max_restarts)
//   --inter-sample  <n>     層間サンプリング数       (toml: run.inter_sample)
//   --stroke-scale  <f>     打鍵数スケール           (toml: weights.stroke_scale)
//   --log-interval  <n>     ログ間隔                 (toml: run.log_interval)
//   --keyboard-size <s>     キーボードサイズ         (toml: run.keyboard_size)
//                           "3x10"（デフォルト）または "3x11"
//   --log           <path>  ログファイルパス         (省略時: log/YYMMDD_HHMMSS.log)

use rand::rngs::SmallRng;
use rand::SeedableRng;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tsuki_optimize::config::Config;
use tsuki_optimize::corpus::Corpus;
use tsuki_optimize::cost::score;
use tsuki_optimize::layout;
use tsuki_optimize::search;

// ──────────────────────────────────────────────────────────────
// TeeWriter: stderr とログファイルの両方に書き込む
//
// ログファイル書き込みに失敗した場合は stop_flag を立てて探索を中断する。
// 書き込みエラーは io_error に保持し、探索終了後に呼び出し側から参照する。
// ──────────────────────────────────────────────────────────────
struct TeeWriter {
    file: Option<BufWriter<File>>,
    stop_flag: Arc<AtomicBool>,
    io_error: Option<String>,
}

impl TeeWriter {
    fn new(file: File, stop_flag: Arc<AtomicBool>) -> Self {
        TeeWriter {
            file: Some(BufWriter::new(file)),
            stop_flag,
            io_error: None,
        }
    }

    fn record_error(&mut self, e: io::Error) {
        if self.io_error.is_none() {
            let msg = format!("ログファイル書き込みエラー: {e}");
            eprintln!("エラー: {msg} → 探索を中断します。");
            self.io_error = Some(msg);
            self.stop_flag.store(true, Ordering::Relaxed);
            // これ以上のファイル書き込み試行を停止
            self.file = None;
        }
    }
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        if let Some(ref mut f) = self.file {
            if let Err(e) = f.write_all(buf) {
                self.record_error(e);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        if let Some(ref mut f) = self.file {
            if let Err(e) = f.flush() {
                self.record_error(e);
            }
        }
        Ok(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cli = parse_cli(&args[1..]);

    // ── 設定ファイル読み込み ──────────────────────
    let config_path_str = cli
        .get("--config")
        .map(|s| s.as_str())
        .unwrap_or("config.toml");
    let config_path = Path::new(config_path_str);

    let toml_config = if config_path.exists() {
        match Config::from_file(config_path) {
            Ok(c) => {
                eprintln!("設定ファイル読み込み: {}", config_path.display());
                c
            }
            Err(e) => {
                eprintln!("エラー: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        if config_path_str != "config.toml" {
            eprintln!(
                "エラー: 設定ファイルが見つかりません: {}",
                config_path.display()
            );
            std::process::exit(1);
        }
        eprintln!("設定ファイルなし → デフォルト値で起動します");
        Config::default()
    };

    // ── キーボードサイズ決定（CLI > TOML > デフォルト）──
    // CLIの --keyboard-size が TOML の run.keyboard_size を上書きする
    let kp = if let Some(ks) = cli.get("--keyboard-size") {
        match ks.as_str() {
            "3x11" => layout::KeyboardParams::k3x11(),
            "3x10" => layout::KeyboardParams::k3x10(),
            other => {
                eprintln!(
                    "警告: 不明な --keyboard-size '{}' → 3x10 を使用します",
                    other
                );
                layout::KeyboardParams::k3x10()
            }
        }
    } else {
        toml_config.build_keyboard_params()
    };

    // ── 排他配置ペア制約 ──────────────────────────
    let exclusive_pairs = toml_config.build_exclusive_pairs();

    // ── 設定ビルド ───────────────────────────────
    let mut search_config = toml_config.build_search_config();
    let mut weights = toml_config.build_weights(kp);

    if let Some(v) = cli.get("--iter") {
        search_config.max_iter = parse_cli_value("--iter", v);
    }
    if let Some(v) = cli.get("--restart") {
        search_config.restart_after = parse_cli_value("--restart", v);
    }
    if let Some(v) = cli.get("--max-restarts") {
        search_config.max_restarts = parse_cli_value("--max-restarts", v);
    }
    if let Some(v) = cli.get("--inter-sample") {
        search_config.inter_sample = parse_cli_value("--inter-sample", v);
    }
    if let Some(v) = cli.get("--log-interval") {
        search_config.log_interval = parse_cli_value("--log-interval", v);
    }
    if let Some(v) = cli.get("--stroke-scale") {
        weights.stroke_scale = parse_cli_value("--stroke-scale", v);
    }

    let corpus_path = toml_config.corpus_path(cli.get("--corpus").map(|s| s.as_str()));
    let seed = {
        let cli_seed = cli.get("--seed").map(|s| parse_cli_value::<u64>("--seed", s));
        toml_config.seed(cli_seed)
    };

    // ── コーパス読み込み ─────────────────────────
    let corpus_file = Path::new(&corpus_path);
    if !corpus_file.exists() {
        eprintln!(
            "エラー: コーパスファイルが見つかりません: {}",
            corpus_path
        );
        std::process::exit(1);
    }
    let corpus = match Corpus::from_file(corpus_file) {
        Ok(c) => {
            eprintln!("コーパス: {}", corpus_file.display());
            c
        }
        Err(e) => {
            eprintln!("エラー: コーパス読み込み失敗: {}", e);
            std::process::exit(1);
        }
    };
    if corpus.is_empty() {
        eprintln!("エラー: コーパスに認識可能な文字が含まれていません: {}", corpus_path);
        std::process::exit(1);
    }

    // ── シグナルハンドラ登録用のフラグを先に準備 ──
    let stop_flag = Arc::new(AtomicBool::new(false));
    let report_flag = Arc::new(AtomicBool::new(false));

    // ── ログファイル作成 + TeeWriter ─────────────
    let log_path = cli.get("--log").cloned().unwrap_or_else(|| {
        let ts = tsuki_optimize::local_timestamp();
        format!("log/{}.log", ts)
    });

    if let Some(parent) = Path::new(&log_path).parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "エラー: ログディレクトリを作成できません ({}): {}",
                    parent.display(),
                    e
                );
                std::process::exit(1);
            }
        }
    }
    let log_file = match File::create(&log_path) {
        Ok(f) => {
            eprintln!("ログファイル: {}", log_path);
            f
        }
        Err(e) => {
            eprintln!(
                "エラー: ログファイルを作成できません ({log_path}): {e}"
            );
            std::process::exit(1);
        }
    };

    let mut out = TeeWriter::new(log_file, Arc::clone(&stop_flag));

    // ── コーパス統計表示 ──────────────────────────
    tsuki_optimize::write_corpus_stats(&mut out, &corpus.stats);

    // ── 設定検証 ────────────────────────────────
    search_config.validate(&mut out);

    // ── 設定サマリ表示 ───────────────────────────
    tsuki_optimize::write_config_summary(
        &mut out,
        &kp,
        &corpus_path,
        seed,
        &search_config,
        &weights,
        &toml_config,
        &exclusive_pairs,
    );

    // ── 初期解生成 ───────────────────────────────
    let l1_only = toml_config.build_l1_only_set();
    let ctx = search::SearchContext {
        corpus: &corpus,
        weights: &weights,
        pairs: &exclusive_pairs,
        l1_only: &l1_only,
    };
    let initial_layout = search::build_initial_layout(&ctx, kp, &mut out);
    let initial_score = score(&initial_layout, &corpus, &weights);
    tsuki_optimize::write_initial_layout(&mut out, &initial_layout, &corpus, &weights);

    // ── シグナルハンドラ登録 ─────────────────────
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGINT, SIGUSR1};
        use signal_hook::flag;
        flag::register(SIGINT, Arc::clone(&stop_flag)).expect("SIGINTハンドラの登録に失敗しました");
        flag::register(SIGUSR1, Arc::clone(&report_flag))
            .expect("SIGUSR1ハンドラの登録に失敗しました");
    }

    // ── タブーサーチ ─────────────────────────────
    let mut rng = SmallRng::seed_from_u64(seed);
    let best_layout = search::run(
        initial_layout,
        &ctx,
        &search_config,
        &mut rng,
        &stop_flag,
        &report_flag,
        &mut |_| {},
        &mut out,
    );

    // ── 結果表示 ─────────────────────────────────
    tsuki_optimize::write_final_result(&mut out, &best_layout, &corpus, &weights, initial_score);
    let _ = out.flush();

    // ── ログファイル書き込みエラーのチェック ──
    if let Some(err) = out.io_error.as_ref() {
        eprintln!("エラー: {err}");
        std::process::exit(1);
    }
}

fn parse_cli(args: &[String]) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut i = 0;
    while i < args.len() {
        if args[i].starts_with("--") && i + 1 < args.len() && !args[i + 1].starts_with("--") {
            map.insert(args[i].clone(), args[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
    }
    map
}

/// CLI 引数の値をパースし、失敗したらエラー出力して終了する
fn parse_cli_value<T>(name: &str, value: &str) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match value.parse::<T>() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("エラー: {name} の値が不正です ('{value}'): {e}");
            std::process::exit(1);
        }
    }
}

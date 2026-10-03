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
//                           "3x10"（デフォルト）/ "3x10_single_shift" / "3x11"
//   --yoon          <s>     拗音方式                 (toml: yoon.mode)
//                           "none"（デフォルト）/ "hybrid"
//   --log           <path>  ログファイルパス         (省略時: log/YYMMDD_HHMMSS.log)

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tsuki_optimize::config::{keyboard_params_from_str, Config};
use tsuki_optimize::search;

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
        eprintln!("設定ファイル読み込み: {}", config_path.display());
        Config::from_file(config_path).unwrap_or_else(|e| {
            eprintln!("エラー: {}", e);
            std::process::exit(1);
        })
    } else if config_path_str == "config.toml" {
        eprintln!("設定ファイルなし → デフォルト値で起動します");
        Config::default()
    } else {
        // 明示指定されたファイルが無いのはエラー（デフォルトの config.toml だけは省略可）
        eprintln!("エラー: 設定ファイルが見つかりません: {}", config_path.display());
        std::process::exit(1);
    };

    // ── キーボードサイズ決定（CLI > TOML > デフォルト）──
    // CLIの --keyboard-size が TOML の run.keyboard_size を上書きする
    let kp = if let Some(ks) = cli.get("--keyboard-size") {
        keyboard_params_from_str(ks)
    } else {
        toml_config.build_keyboard_params()
    };

    // ── 拗音方式決定（CLI > TOML > デフォルト）──
    // CLIの --yoon が TOML の yoon.mode を上書きする
    let yoon_mode = match cli.get("--yoon") {
        Some(s) => tsuki_optimize::yoon::YoonMode::from_config_str(s),
        None => toml_config.build_yoon_mode(),
    };

    // ── 拗音テーブル構築 + キーボードパラメータ拡張 ──
    let yoon = match tsuki_optimize::yoon::YoonSetup::resolve(
        kp,
        yoon_mode,
        toml_config.yoon.consonants.as_deref(),
    ) {
        Ok(y) => y,
        Err(e) => {
            eprintln!("エラー: {}", e);
            std::process::exit(1);
        }
    };
    let kp = yoon.kp;

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
    if let Some(v) = cli.get("--initial-layout") {
        search_config.initial_layout_mode = search::InitialLayoutMode::from_config_str(v);
    }

    let corpus_path = toml_config.corpus_path(cli.get("--corpus").map(|s| s.as_str()));
    let seed = {
        let cli_seed = cli.get("--seed").map(|s| parse_cli_value::<u64>("--seed", s));
        toml_config.seed(cli_seed)
    };

    // ── コーパス読み込み ─────────────────────────
    let corpus = tsuki_optimize::load_corpus(&corpus_path, yoon.table.as_ref()).unwrap_or_else(|e| {
        eprintln!("エラー: {e}");
        std::process::exit(1);
    });
    eprintln!("コーパス: {corpus_path}");

    let stop_flag = Arc::new(AtomicBool::new(false));
    let report_flag = Arc::new(AtomicBool::new(false));

    // ── ログファイル作成（stderr とログファイルの両方に書く）──
    let log_path = cli
        .get("--log")
        .cloned()
        .unwrap_or_else(|| format!("log/{}.log", tsuki_optimize::local_timestamp()));
    let log_file = tsuki_optimize::create_log_file(&log_path).unwrap_or_else(|e| {
        eprintln!("エラー: {e}");
        std::process::exit(1);
    });
    eprintln!("ログファイル: {log_path}");
    let mut out = tsuki_optimize::LogTee::new(std::io::stderr(), log_file, Arc::clone(&stop_flag));

    // ── シグナルハンドラ登録 ─────────────────────
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGINT, SIGUSR1};
        use signal_hook::flag;
        flag::register(SIGINT, Arc::clone(&stop_flag)).expect("SIGINTハンドラの登録に失敗しました");
        flag::register(SIGUSR1, Arc::clone(&report_flag))
            .expect("SIGUSR1ハンドラの登録に失敗しました");
    }

    // ── 探索 ────────────────────────────────────
    tsuki_optimize::Run {
        toml_config,
        yoon,
        search_config,
        weights,
        exclusive_pairs,
        corpus,
        corpus_path,
        seed,
    }
    .execute(&mut out, &stop_flag, &report_flag, &mut |_| {});

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

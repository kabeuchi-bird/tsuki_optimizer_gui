// main.rs — tsuki_optimize エントリポイント
//
// 使い方:
//   cargo run --release -- [CLIオプション]
//
// 設定の優先順位（高 → 低）:
//   1. CLIオプション
//   2. TOMLファイル（--config で指定 or デフォルト: tsuki_optimize.toml）
//   3. ハードコードされたデフォルト値
//
// CLIオプション:
//   --config        <path>  設定ファイルのパス       (default: tsuki_optimize.toml)
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

mod chars;
mod config;
mod corpus;
mod cost;
mod layout;
mod search;

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use signal_hook::consts::{SIGINT, SIGUSR1};
use signal_hook::flag;

use chars::CHAR_LIST;
use config::{Config, keyboard_size_str};
use corpus::Corpus;
use cost::{score, score_breakdown};

// ──────────────────────────────────────────────────────────────
// TeeWriter: stderr とログファイルの両方に書き込む
// ──────────────────────────────────────────────────────────────
struct TeeWriter {
    file: Option<BufWriter<File>>,
}

impl TeeWriter {
    fn new(file: Option<File>) -> Self {
        TeeWriter {
            file: file.map(BufWriter::new),
        }
    }
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        if let Some(ref mut f) = self.file {
            let _ = f.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        if let Some(ref mut f) = self.file {
            let _ = f.flush();
        }
        Ok(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cli = parse_cli(&args[1..]);

    // ── 設定ファイル読み込み ──────────────────────
    let config_path_str = cli.get("--config")
        .map(|s| s.as_str())
        .unwrap_or("tsuki_optimize.toml");
    let config_path = Path::new(config_path_str);

    let toml_config = if config_path.exists() {
        match Config::from_file(config_path) {
            Ok(c) => { eprintln!("設定ファイル読み込み: {}", config_path.display()); c }
            Err(e) => { eprintln!("エラー: {}", e); std::process::exit(1); }
        }
    } else {
        if config_path_str != "tsuki_optimize.toml" {
            eprintln!("エラー: 設定ファイルが見つかりません: {}", config_path.display());
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
            other  => {
                eprintln!("警告: 不明な --keyboard-size '{}' → 3x10 を使用します", other);
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
    let mut weights       = toml_config.build_weights(kp);

    if let Some(v) = cli.get("--iter")         { search_config.max_iter        = v.parse().unwrap_or(search_config.max_iter); }
    if let Some(v) = cli.get("--restart")      { search_config.restart_after   = v.parse().unwrap_or(search_config.restart_after); }
    if let Some(v) = cli.get("--max-restarts") { search_config.max_restarts    = v.parse().unwrap_or(search_config.max_restarts); }
    if let Some(v) = cli.get("--inter-sample") { search_config.inter_sample    = v.parse().unwrap_or(search_config.inter_sample); }
    if let Some(v) = cli.get("--log-interval") { search_config.log_interval    = v.parse().unwrap_or(search_config.log_interval); }
    if let Some(v) = cli.get("--stroke-scale") { weights.stroke_scale          = v.parse().unwrap_or(weights.stroke_scale); }

    let corpus_path = toml_config.corpus_path(cli.get("--corpus").map(|s| s.as_str()));
    let seed        = toml_config.seed(cli.get("--seed").and_then(|s| s.parse().ok()));

    // ── コーパス読み込み ─────────────────────────
    let corpus_file = Path::new(&corpus_path);
    let corpus = if corpus_file.exists() {
        match Corpus::from_file(corpus_file) {
            Ok(c) => { eprintln!("コーパス: {}", corpus_file.display()); c }
            Err(e) => { eprintln!("コーパス読み込みエラー: {}", e); std::process::exit(1); }
        }
    } else {
        eprintln!("コーパスファイルが見つかりません: {}  → サンプルテキストで起動", corpus_path);
        Corpus::from_str(SAMPLE_CORPUS)
    };

    // ── ログファイル作成 + TeeWriter ─────────────
    let log_path = cli.get("--log")
        .cloned()
        .unwrap_or_else(|| {
            let ts = utc_timestamp();
            format!("log/{}.log", ts)
        });

    let log_file = {
        if let Some(parent) = Path::new(&log_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match File::create(&log_path) {
            Ok(f)  => { eprintln!("ログファイル: {}", log_path); Some(f) }
            Err(e) => { eprintln!("ログファイル作成失敗: {} ({})", log_path, e); None }
        }
    };

    let mut out = TeeWriter::new(log_file);

    // ── 設定サマリ表示 ───────────────────────────
    let _ = writeln!(out, "\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let _ = writeln!(out, " tsuki_optimize 実行設定");
    let _ = writeln!(out, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let _ = writeln!(out, " keyboard_size = {}", keyboard_size_str(&kp));
    let _ = writeln!(out, " corpus        = {}", corpus_path);
    let _ = writeln!(out, " seed          = {}", seed);
    let _ = writeln!(out, " max_iter      = {}", search_config.max_iter);
    let _ = writeln!(out, " restart_after = {}", search_config.restart_after);
    let _ = writeln!(out, " max_restarts  = {}", search_config.max_restarts);
    let _ = writeln!(out, " tabu           l1={} l2={} inter={}", search_config.tabu_l1, search_config.tabu_l2, search_config.tabu_inter);
    let _ = writeln!(out, " inter_sample  = {}", search_config.inter_sample);
    let _ = writeln!(out, " perturbation  = {} swaps/restart", search_config.perturbation_swaps);
    let _ = writeln!(out, " tenure         grow_threshold={:.2}  grow_interval={}  max_scale={:.1}",
        search_config.tenure_grow_threshold,
        search_config.tenure_grow_interval,
        search_config.tenure_max_scale);
    let _ = writeln!(out, " stroke_scale  = {:.1}", weights.stroke_scale);
    let _ = writeln!(out, " penalties      same_key={:.1}  same_finger={:.1}  upper_lower={:.1}  same_hand={:.2}",
        weights.same_key_penalty, weights.same_finger_penalty,
        weights.upper_lower_jump, weights.same_hand_base);
    let _ = writeln!(out, " bonuses        alt={:.2}  outroll={:.2}  inroll={:.2}  quasi_alt={:.2}",
        weights.alternation_bonus, weights.outroll_bonus,
        weights.inroll_bonus, weights.quasi_alt_bonus);
    if exclusive_pairs.is_empty() {
        let _ = writeln!(out, " exclusive_pairs = (なし)");
    } else {
        for pair in &exclusive_pairs {
            let a: String = pair.group_a.iter().map(|&c| CHAR_LIST[c as usize]).collect();
            let b: String = pair.group_b.iter().map(|&c| CHAR_LIST[c as usize]).collect();
            let _ = writeln!(out, " exclusive_pair  A={}  B={}", a, b);
        }
    }
    let _ = writeln!(out, " slot_difficulty:");
    let nc = kp.num_cols as usize;
    for (r, row) in weights.slot_difficulty.iter().enumerate() {
        let label = ["  上段(row0)", "  中段(row1)", "  下段(row2)"][r];
        let _ = writeln!(out, "{} {:?}", label, &row[..nc]);
    }
    let _ = writeln!(out, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // ── 初期解生成 ───────────────────────────────
    let ctx = search::SearchContext { corpus: &corpus, weights: &weights, pairs: &exclusive_pairs };
    let initial_layout = search::build_initial_layout(&ctx, kp, &mut out);
    let _ = writeln!(out, "【初期解】");
    initial_layout.display(&mut out);
    let initial_score = score(&initial_layout, &corpus, &weights);
    score_breakdown(&initial_layout, &corpus, &weights, &mut out);

    // ── シグナルハンドラ登録 ─────────────────────
    let stop_flag = Arc::new(AtomicBool::new(false));
    flag::register(SIGINT, Arc::clone(&stop_flag))
        .expect("SIGINTハンドラの登録に失敗しました");
    let report_flag = Arc::new(AtomicBool::new(false));
    flag::register(SIGUSR1, Arc::clone(&report_flag))
        .expect("SIGUSR1ハンドラの登録に失敗しました");

    // ── タブーサーチ ─────────────────────────────
    let mut rng = SmallRng::seed_from_u64(seed);
    let best_layout = search::run(initial_layout, &ctx, &search_config, &mut rng, &stop_flag, &report_flag, &mut out);

    // ── 結果表示 ─────────────────────────────────
    let _ = writeln!(out, "\n【最適化結果】");
    best_layout.display(&mut out);
    score_breakdown(&best_layout, &corpus, &weights, &mut out);

    let score_best = score(&best_layout, &corpus, &weights);
    let _ = writeln!(out, "\n初期スコア : {:.4}", initial_score);
    let _ = writeln!(out, "最良スコア : {:.4}", score_best);
    let _ = writeln!(out, "改善幅     : {:.4}  ({:.2}%)",
        initial_score - score_best,
        (initial_score - score_best) / initial_score.abs() * 100.0);
    let _ = out.flush();
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

/// UTC タイムスタンプ文字列（YYMMDD_HHMMSS）を生成する
fn utc_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    // Howard Hinnant's civil_from_days algorithm
    let days = (secs / 86400) as i64;
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1461 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let tod = secs % 86400;
    format!("{:02}{:02}{:02}_{:02}{:02}{:02}",
        y % 100, m, d, tod / 3600, (tod % 3600) / 60, tod % 60)
}

const SAMPLE_CORPUS: &str = "\
こんにちは。今日はいい天気ですね。\
日本語入力の配列を最適化するためのプログラムです。\
タブーサーチを用いて月配列の改変版を探索します。\
かな文字の打鍵数と難易度を評価して最良の配置を求めます。\
てにをはなどの助詞や、よく使う動詞・形容詞が打ちやすくなるように配置します。\
";

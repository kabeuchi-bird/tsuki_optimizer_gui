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
//   --config   <path>  設定ファイルのパス     (default: tsuki_optimize.toml)
//   --corpus   <path>  コーパスファイルパス   (toml: run.corpus)
//   --seed     <n>     乱数シード             (toml: run.seed)
//   --iter     <n>     最大イテレーション数   (toml: run.max_iter)
//   --restart  <n>     再起動閾値             (toml: run.restart_after)
//   --max-restarts <n> 最大再起動回数         (toml: run.max_restarts)
//   --inter-sample <n> 層間サンプリング数     (toml: run.inter_sample)
//   --stroke-scale <f> 打鍵数スケール         (toml: weights.stroke_scale)
//   --log-interval <n> ログ間隔               (toml: run.log_interval)

mod chars;
mod config;
mod corpus;
mod cost;
mod layout;
mod search;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use signal_hook::consts::{SIGINT, SIGUSR1};
use signal_hook::flag;

use config::Config;
use corpus::Corpus;
use cost::{score, score_breakdown};

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

    // ── CLIオプションでTOML値を上書き ────────────
    let mut search_config = toml_config.build_search_config();
    let mut weights       = toml_config.build_weights();

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

    // ── 設定サマリ表示 ───────────────────────────
    eprintln!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    eprintln!(" tsuki_optimize 実行設定");
    eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    eprintln!(" corpus        = {}", corpus_path);
    eprintln!(" seed          = {}", seed);
    eprintln!(" max_iter      = {}", search_config.max_iter);
    eprintln!(" restart_after = {}", search_config.restart_after);
    eprintln!(" max_restarts  = {}", search_config.max_restarts);
    eprintln!(" tabu           l1={} l2={} inter={}", search_config.tabu_l1, search_config.tabu_l2, search_config.tabu_inter);
    eprintln!(" inter_sample  = {}", search_config.inter_sample);
    eprintln!(" perturbation  = {} swaps/restart", search_config.perturbation_swaps);
    eprintln!(" tenure         grow_threshold={:.2}  grow_interval={}  max_scale={:.1}",
        search_config.tenure_grow_threshold,
        search_config.tenure_grow_interval,
        search_config.tenure_max_scale);
    eprintln!(" stroke_scale  = {:.1}", weights.stroke_scale);
    eprintln!(" penalties      same_key={:.1}  same_finger={:.1}  upper_lower={:.1}  same_hand={:.2}",
        weights.same_key_penalty, weights.same_finger_penalty,
        weights.upper_lower_jump, weights.same_hand_base);
    eprintln!(" bonuses        alt={:.2}  outroll={:.2}  inroll={:.2}  quasi_alt={:.2}",
        weights.alternation_bonus, weights.outroll_bonus,
        weights.inroll_bonus, weights.quasi_alt_bonus);
    eprintln!(" slot_difficulty:");
    for (r, row) in weights.slot_difficulty.iter().enumerate() {
        let label = ["  上段(row0)", "  中段(row1)", "  下段(row2)"][r];
        eprintln!("{} {:?}", label, row);
    }
    eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // ── 初期解生成 ───────────────────────────────
    let initial_layout = search::build_initial_layout(&corpus);
    eprintln!("【初期解】");
    initial_layout.display();
    score_breakdown(&initial_layout, &corpus, &weights);

    // ── シグナルハンドラ登録 ─────────────────────
    // SIGINT (Ctrl+C): 探索を止めてベストを出力して終了
    // SIGUSR1 (kill -USR1 <pid>): 現在のベストを出力して探索は継続
    // SIGINT  → 探索を止めてベストを出力して終了
    let stop_flag = Arc::new(AtomicBool::new(false));
    flag::register(SIGINT, Arc::clone(&stop_flag))
        .expect("SIGINTハンドラの登録に失敗しました");
    // SIGUSR1 → 現在のベストをログに出力して探索は継続
    //           使い方: kill -USR1 $(pgrep tsuki_optimize)
    let report_flag = Arc::new(AtomicBool::new(false));
    flag::register(SIGUSR1, Arc::clone(&report_flag))
        .expect("SIGUSR1ハンドラの登録に失敗しました");

    // ── タブーサーチ ─────────────────────────────
    let mut rng = SmallRng::seed_from_u64(seed);
    let best_layout = search::run(initial_layout, &corpus, &weights, &search_config, &mut rng, &stop_flag, &report_flag);

    // ── 結果表示 ─────────────────────────────────
    println!("\n【最適化結果】");
    best_layout.display();
    score_breakdown(&best_layout, &corpus, &weights);

    let score_init = score(&search::build_initial_layout(&corpus), &corpus, &weights);
    let score_best = score(&best_layout, &corpus, &weights);
    println!("\n初期スコア : {:.4}", score_init);
    println!("最良スコア : {:.4}", score_best);
    println!("改善幅     : {:.4}  ({:.2}%)",
        score_init - score_best,
        (score_init - score_best) / score_init.abs() * 100.0);
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

const SAMPLE_CORPUS: &str = "\
こんにちは。今日はいい天気ですね。\
日本語入力の配列を最適化するためのプログラムです。\
タブーサーチを用いて月配列の改変版を探索します。\
かな文字の打鍵数と難易度を評価して最良の配置を求めます。\
てにをはなどの助詞や、よく使う動詞・形容詞が打ちやすくなるように配置します。\
";

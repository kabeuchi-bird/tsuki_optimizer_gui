// lib.rs — tsuki_optimize ライブラリクレート
//
// CLI (main.rs) と GUI (bin/gui.rs) の両方から利用される。

pub mod chars;
pub mod config;
pub mod corpus;
pub mod cost;
pub mod layout;
pub mod search;

/// ローカルタイムのタイムスタンプ文字列（YYMMDD_HHMMSS）を生成する
pub fn local_timestamp() -> String {
    chrono::Local::now().format("%y%m%d_%H%M%S").to_string()
}

use std::io::Write;

/// 設定サマリーを出力する（CLI / GUI 共通）
#[allow(clippy::too_many_arguments)]
pub fn write_config_summary(
    out: &mut impl Write,
    kp: &layout::KeyboardParams,
    corpus_path: &str,
    seed: u64,
    search_config: &search::SearchConfig,
    weights: &cost::Weights,
    toml_config: &config::Config,
    exclusive_pairs: &[layout::ExclusivePair],
) {
    let _ = writeln!(out, "\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let _ = writeln!(out, " tsuki_optimize 実行設定");
    let _ = writeln!(out, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let _ = writeln!(out, " keyboard_size = {}", config::keyboard_size_str(kp));
    let _ = writeln!(out, " corpus        = {}", corpus_path);
    let _ = writeln!(out, " seed          = {}", seed);
    let _ = writeln!(out, " max_iter      = {}", search_config.max_iter);
    let _ = writeln!(out, " restart_after = {}", search_config.restart_after);
    let _ = writeln!(out, " max_restarts  = {}", search_config.max_restarts);
    let _ = writeln!(
        out,
        " tabu           l1={} l2={} inter={}",
        search_config.tabu_l1, search_config.tabu_l2, search_config.tabu_inter
    );
    let _ = writeln!(out, " inter_sample  = {}", search_config.inter_sample);
    let _ = writeln!(
        out,
        " perturbation  = {} swaps/restart",
        search_config.perturbation_swaps
    );
    let _ = writeln!(
        out,
        " tenure         grow_threshold={:.2}  grow_interval={}  max_scale={:.1}",
        search_config.tenure_grow_threshold,
        search_config.tenure_grow_interval,
        search_config.tenure_max_scale
    );
    let _ = writeln!(out, " stroke_scale  = {:.1}", weights.stroke_scale);
    let _ = writeln!(
        out,
        " penalties      same_key={:.1}  same_finger={:.1}  upper_lower={:.1}  same_hand={:.2}",
        weights.same_key_penalty,
        weights.same_finger_penalty,
        weights.upper_lower_jump,
        weights.same_hand_base
    );
    let _ = writeln!(
        out,
        " bonuses        alt={:.2}  outroll={:.2}  inroll={:.2}  quasi_alt={:.2}",
        weights.alternation_bonus,
        weights.outroll_bonus,
        weights.inroll_bonus,
        weights.quasi_alt_bonus
    );
    if let Some(p) = &toml_config.constraints.preset {
        let _ = writeln!(out, " constraints.preset = {}", p);
    }
    if exclusive_pairs.is_empty() {
        let _ = writeln!(out, " exclusive_pairs = (なし)");
    } else {
        for pair in exclusive_pairs {
            let a: String = pair
                .group_a
                .iter()
                .map(|&c| chars::CHAR_LIST[c as usize])
                .collect();
            let b: String = pair
                .group_b
                .iter()
                .map(|&c| chars::CHAR_LIST[c as usize])
                .collect();
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
}

/// 初期解を出力する（CLI / GUI 共通）
pub fn write_initial_layout(
    out: &mut impl Write,
    layout: &layout::Layout,
    corpus: &corpus::Corpus,
    weights: &cost::Weights,
) {
    let _ = writeln!(out, "【初期解】");
    layout.display(out);
    cost::score_breakdown(layout, corpus, weights, out);
}

/// 最終結果を出力する（CLI / GUI 共通）
pub fn write_final_result(
    out: &mut impl Write,
    best_layout: &layout::Layout,
    corpus: &corpus::Corpus,
    weights: &cost::Weights,
    initial_score: f64,
) {
    let _ = writeln!(out, "\n【最適化結果】");
    best_layout.display(out);
    cost::score_breakdown(best_layout, corpus, weights, out);
    let best_score = cost::score(best_layout, corpus, weights);
    let _ = writeln!(out, "\n初期スコア : {:.4}", initial_score);
    let _ = writeln!(out, "最良スコア : {:.4}", best_score);
    let _ = writeln!(
        out,
        "改善幅     : {:.4}  ({:.2}%)",
        initial_score - best_score,
        (initial_score - best_score) / initial_score.abs() * 100.0
    );
}

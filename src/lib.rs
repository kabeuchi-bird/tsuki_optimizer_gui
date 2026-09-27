// lib.rs — tsuki_optimize ライブラリクレート
//
// CLI (main.rs) と GUI (bin/gui.rs) の両方から利用される。

pub mod chars;
pub mod config;
pub mod corpus;
pub mod cost;
pub mod layout;
pub mod search;
pub mod user_layout;
pub mod yoon;

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
    let _ = writeln!(out, " tsuki_optimize v{} 実行設定", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let _ = writeln!(out, " keyboard_size = {}", config::keyboard_size_str(kp));
    let _ = writeln!(
        out,
        " yoon_mode     = {}",
        if kp.yoon { "hybrid" } else { "none" }
    );
    if kp.yoon {
        let tokens: Vec<&str> = (0..kp.num_consonants)
            .filter_map(|i| yoon::consonant_label(kp.consonant_mask, chars::CONSONANT_FIRST + i))
            .collect();
        let _ = writeln!(
            out,
            " consonants    = {} 種 [{}]",
            kp.num_consonants,
            tokens.join(" ")
        );
    }
    let _ = writeln!(out, " corpus        = {}", corpus_path);
    let _ = writeln!(out, " seed          = {}", seed);
    let _ = writeln!(out, " max_iter      = {}", search_config.max_iter);
    let _ = writeln!(out, " restart_after = {}", search_config.restart_after);
    let _ = writeln!(out, " max_restarts  = {}", search_config.max_restarts);
    let _ = writeln!(
        out,
        " tabu(近傍比)   {}",
        search::summarize_by_kind(&search_config.tabu_ratio, search::active_op_kinds(kp))
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
    let _ = writeln!(
        out,
        " diversification= {:.2}{}",
        search_config.diversification,
        if search_config.diversification > 0.0 { "" } else { "（無効）" }
    );
    let _ = writeln!(out, " initial_layout = {}",
        search_config.initial_layout_mode.config_label()
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
        " bonuses        alt={:.2}  outroll_2g={:.2}  inroll_2g={:.2}  quasi_alt={:.2}  outroll_3g={:.2}  inroll_3g={:.2}",
        weights.alternation_bonus,
        weights.outroll_bonus_2gram,
        weights.inroll_bonus_2gram,
        weights.quasi_alt_bonus,
        weights.outroll_bonus_3gram,
        weights.inroll_bonus_3gram,
    );
    let _ = writeln!(out, " allow_index_roll = {}", weights.allow_index_roll);
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

/// コーパス統計を出力する（CLI / GUI 共通）
pub fn write_corpus_stats(out: &mut impl Write, stats: &corpus::CorpusStats) {
    let _ = writeln!(
        out,
        "コーパス統計: 有効文字数={}, スキップ文字数={}, セグメント数={}, \
         ユニグラム種={}, バイグラム種={}, トライグラム種={}",
        stats.total_chars,
        stats.skipped_chars,
        stats.num_segments,
        stats.num_unigrams,
        stats.num_bigrams,
        stats.num_trigrams,
    );
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
    cost::write_top_bigrams(layout, corpus, weights, out);
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
    cost::write_top_bigrams(best_layout, corpus, weights, out);
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

/// コーパスを読み込む。存在しない・読めない・認識可能な文字がない場合は Err（CLI / GUI 共通）
pub fn load_corpus(path: &str, table: Option<&yoon::YoonTable>) -> Result<corpus::Corpus, String> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return Err(format!("コーパスファイルが見つかりません: {path}"));
    }
    let c = corpus::Corpus::from_file_with_yoon(p, table)
        .map_err(|e| format!("コーパスファイルを読み込めません ({path}): {e}"))?;
    if c.is_empty() {
        return Err(format!("コーパスに認識可能な文字が含まれていません: {path}"));
    }
    Ok(c)
}

/// ログファイルを親ディレクトリごと作成する（CLI / GUI 共通）
pub fn create_log_file(path: &str) -> Result<std::fs::File, String> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!("ログディレクトリを作成できません ({}): {e}", parent.display())
        })?;
    }
    std::fs::File::create(path).map_err(|e| format!("ログファイルを作成できません ({path}): {e}"))
}

/// 探索1回分の入力（CLI / GUI 共通）
pub struct Run {
    pub toml_config: config::Config,
    pub yoon: yoon::YoonSetup,
    pub search_config: search::SearchConfig,
    pub weights: cost::Weights,
    pub exclusive_pairs: Vec<layout::ExclusivePair>,
    pub corpus: corpus::Corpus,
    pub corpus_path: String,
    pub seed: u64,
}

impl Run {
    /// 設定検証 → 初期解生成 → タブーサーチ → 結果出力を、すべて `out` に書きながら実行する
    pub fn execute(
        &self,
        out: &mut impl Write,
        stop_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
        report_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
        on_update: &mut impl FnMut(&search::SearchUpdate),
    ) {
        use rand::SeedableRng;

        let kp = self.yoon.kp;
        write_corpus_stats(out, &self.corpus.stats);
        self.toml_config.validate(out);
        self.search_config.validate(out);
        write_config_summary(
            out,
            &kp,
            &self.corpus_path,
            self.seed,
            &self.search_config,
            &self.weights,
            &self.toml_config,
            &self.exclusive_pairs,
        );

        let mut rng = rand::rngs::SmallRng::seed_from_u64(self.seed);
        let mut l1_only = self.toml_config.build_l1_only_set();
        // hybrid では拗音シフト ゃゅょ を L1 固定にする（1打でなければ方式が成立しない）
        self.yoon.extend_l1_only(&mut l1_only);
        let ctx = search::SearchContext {
            corpus: &self.corpus,
            weights: &self.weights,
            pairs: &self.exclusive_pairs,
            l1_only: &l1_only,
        };
        let initial = search::build_initial_layout(
            &ctx, kp, self.search_config.initial_layout_mode, &mut rng, out,
        );
        let initial_score = cost::score(&initial, &self.corpus, &self.weights);
        write_initial_layout(out, &initial, &self.corpus, &self.weights);

        let best = search::run(
            initial, &ctx, &self.search_config, &mut rng, stop_flag, report_flag, on_update, out,
        );
        write_final_result(out, &best, &self.corpus, &self.weights, initial_score);
        let _ = out.flush();
    }
}

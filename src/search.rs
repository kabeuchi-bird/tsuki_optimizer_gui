// search.rs — タブーサーチ本体

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use rand::prelude::*;

use crate::chars::{CharId, NUM_CHARS};
use crate::corpus::Corpus;
use crate::cost::{delta_score, score, Weights};
use crate::layout::{Layout, is_fixed, is_inter_layer_movable};

/// ——————————————————————————————
/// タブーリスト（циrcular buffer）
/// ——————————————————————————————
struct TabuList {
    entries: Vec<(CharId, CharId)>,
    capacity: usize,
    head: usize,
}

impl TabuList {
    fn new(capacity: usize) -> Self {
        TabuList {
            entries: Vec::with_capacity(capacity),
            capacity,
            head: 0,
        }
    }

    fn contains(&self, c1: CharId, c2: CharId) -> bool {
        let key = normalize_pair(c1, c2);
        self.entries.iter().any(|&e| e == key)
    }

    fn add(&mut self, c1: CharId, c2: CharId) {
        let key = normalize_pair(c1, c2);
        if self.entries.len() < self.capacity {
            self.entries.push(key);
        } else {
            self.entries[self.head] = key;
            self.head = (self.head + 1) % self.capacity;
        }
    }
}

#[inline]
fn normalize_pair(a: CharId, b: CharId) -> (CharId, CharId) {
    if a <= b { (a, b) } else { (b, a) }
}

/// ——————————————————————————————
/// 操作の種類
/// ——————————————————————————————
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    /// Layer 1 内スワップ
    SwapL1,
    /// Layer 2 内スワップ
    SwapL2,
    /// 層間スワップ（Layer 1文字 ⟺ Layer 2文字）
    InterLayer,
}

/// 候補操作
#[derive(Clone, Copy, Debug)]
struct Candidate {
    kind: OpKind,
    c1: CharId,
    c2: CharId,
    delta: f64,
}

/// ——————————————————————————————
/// タブーサーチの設定
/// ——————————————————————————————
pub struct SearchConfig {
    /// 最大イテレーション数
    pub max_iter: usize,
    /// 改善なしで再起動するイテレーション数
    pub restart_after: usize,
    /// 最大再起動回数
    pub max_restarts: usize,
    /// タブーリストのテニュア（操作種ごと）
    pub tabu_l1: usize,
    pub tabu_l2: usize,
    pub tabu_inter: usize,
    /// 操作Cのサンプリング数（層間スワップ候補数）
    pub inter_sample: usize,
    /// 操作A/Bは全候補を評価する上限（この数を超えたらランダムサンプリング）
    pub ab_sample_limit: usize,
    /// 進捗ログの間隔
    pub log_interval: usize,
    /// 再起動時のランダム層間スワップ回数
    pub perturbation_swaps: usize,
    /// no_improve がこの割合（0.0〜1.0）× restart_after を超えたらテニュア増加を開始
    pub tenure_grow_threshold: f64,
    /// テニュア増加の間隔（イテレーション数）
    pub tenure_grow_interval: usize,
    /// テニュアの上限倍率（初期値 × この値まで）
    pub tenure_max_scale: f64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            max_iter: 50_000,
            restart_after: 3_000,
            max_restarts: 10,
            tabu_l1: 15,
            tabu_l2: 15,
            tabu_inter: 25,
            inter_sample: 80,
            ab_sample_limit: 200,
            log_interval: 1_000,
            perturbation_swaps: 8,
            tenure_grow_threshold: 0.5,
            tenure_grow_interval:  200,
            tenure_max_scale:      3.0,
        }
    }
}

/// ——————————————————————————————
/// タブーサーチ本体
/// ——————————————————————————————
pub fn run(
    initial_layout: Layout,
    corpus: &Corpus,
    weights: &Weights,
    config: &SearchConfig,
    rng: &mut impl Rng,
    stop_flag: &Arc<AtomicBool>,
    report_flag: &Arc<AtomicBool>,
) -> Layout {
    let mut current = initial_layout.clone();
    let mut current_score = score(&current, corpus, weights);

    let mut best = current.clone();
    let mut best_score = current_score;

    let mut no_improve = 0usize;
    let mut restarts   = 0usize;
    let mut iter       = 0usize;

    // ── 可動テニュア管理 ──────────────────────────
    // 現在の実効テニュア（no_improveに応じて伸長、改善・再起動で初期値に戻す）
    let mut cur_tabu_l1    = config.tabu_l1;
    let mut cur_tabu_l2    = config.tabu_l2;
    let mut cur_tabu_inter = config.tabu_inter;
    // テニュア増加を開始する no_improve の閾値
    let tenure_grow_start =
        (config.restart_after as f64 * config.tenure_grow_threshold) as usize;
    // tenure_step を他パラメータから導出:
    //   再起動までの増加期間でちょうど上限（initial × max_scale）に届く量
    //   tenure_step = initial × (max_scale - 1.0) × interval / grow_period
    //   ゼロ除算・ゼロステップを避けるため最小1を保証する
    let grow_period = config.restart_after.saturating_sub(tenure_grow_start).max(1);
    let tenure_step_l1 = (
        config.tabu_l1 as f64
        * (config.tenure_max_scale - 1.0)
        * config.tenure_grow_interval as f64
        / grow_period as f64
    ).ceil().max(1.0) as usize;
    let tenure_step_l2 = (
        config.tabu_l2 as f64
        * (config.tenure_max_scale - 1.0)
        * config.tenure_grow_interval as f64
        / grow_period as f64
    ).ceil().max(1.0) as usize;
    let tenure_step_inter = (
        config.tabu_inter as f64
        * (config.tenure_max_scale - 1.0)
        * config.tenure_grow_interval as f64
        / grow_period as f64
    ).ceil().max(1.0) as usize;

    let mut tabu_l1    = TabuList::new(cur_tabu_l1);
    let mut tabu_l2    = TabuList::new(cur_tabu_l2);
    let mut tabu_inter = TabuList::new(cur_tabu_inter);

    while iter < config.max_iter {
        iter += 1;

        // ——————————————————
        // 候補生成
        // ——————————————————
        let mut candidates: Vec<Candidate> = Vec::new();

        // 操作A: Layer 1 内スワップ
        let l1_free = collect_l1_free_chars(&current);
        generate_swap_candidates(
            &current, corpus, weights,
            &l1_free, OpKind::SwapL1,
            config.ab_sample_limit, rng,
            &mut candidates,
        );

        // 操作B: Layer 2 内スワップ
        let l2_free = collect_l2_chars(&current);
        generate_swap_candidates(
            &current, corpus, weights,
            &l2_free, OpKind::SwapL2,
            config.ab_sample_limit, rng,
            &mut candidates,
        );

        // 操作C: 層間スワップ（サンプリング）
        generate_inter_layer_candidates(
            &current, corpus, weights,
            config.inter_sample, rng,
            &mut candidates,
        );

        if candidates.is_empty() {
            break;
        }

        // デルタ昇順ソート
        candidates.sort_unstable_by(|a, b| a.delta.total_cmp(&b.delta));

        // ——————————————————
        // 最良の非タブー操作（またはアスピレーション）を選択
        // ——————————————————
        let chosen = candidates.iter().find(|cand| {
            let tabu = match cand.kind {
                OpKind::SwapL1    => tabu_l1.contains(cand.c1, cand.c2),
                OpKind::SwapL2    => tabu_l2.contains(cand.c1, cand.c2),
                OpKind::InterLayer => tabu_inter.contains(cand.c1, cand.c2),
            };
            // タブーでない、またはアスピレーション（グローバルベスト更新）
            !tabu || (current_score + cand.delta < best_score)
        });

        let Some(chosen) = chosen else { continue };
        let chosen = *chosen;

        // ——————————————————
        // 操作を適用
        // ——————————————————
        current.swap_chars(chosen.c1, chosen.c2);
        current_score += chosen.delta;

        // タブーリスト更新
        match chosen.kind {
            OpKind::SwapL1     => tabu_l1.add(chosen.c1, chosen.c2),
            OpKind::SwapL2     => tabu_l2.add(chosen.c1, chosen.c2),
            OpKind::InterLayer => tabu_inter.add(chosen.c1, chosen.c2),
        }

        // グローバルベスト更新
        if current_score < best_score {
            best_score  = current_score;
            best        = current.clone();
            no_improve  = 0;
            // 改善発生 → テニュアを初期値に戻す
            if cur_tabu_l1 != config.tabu_l1
                || cur_tabu_l2 != config.tabu_l2
                || cur_tabu_inter != config.tabu_inter
            {
                cur_tabu_l1    = config.tabu_l1;
                cur_tabu_l2    = config.tabu_l2;
                cur_tabu_inter = config.tabu_inter;
                tabu_l1    = TabuList::new(cur_tabu_l1);
                tabu_l2    = TabuList::new(cur_tabu_l2);
                tabu_inter = TabuList::new(cur_tabu_inter);
            }
        } else {
            no_improve += 1;
            // ── 可動テニュア増加 ──────────────────────
            // 閾値を超えており、かつ増加インターバルに達したら伸長する
            if no_improve > tenure_grow_start
                && (no_improve - tenure_grow_start) % config.tenure_grow_interval == 0
            {
                let max_l1    = (config.tabu_l1    as f64 * config.tenure_max_scale) as usize;
                let max_l2    = (config.tabu_l2    as f64 * config.tenure_max_scale) as usize;
                let max_inter = (config.tabu_inter as f64 * config.tenure_max_scale) as usize;
                let grew =
                       cur_tabu_l1    < max_l1
                    || cur_tabu_l2    < max_l2
                    || cur_tabu_inter < max_inter;
                cur_tabu_l1    = (cur_tabu_l1    + tenure_step_l1).min(max_l1);
                cur_tabu_l2    = (cur_tabu_l2    + tenure_step_l2).min(max_l2);
                cur_tabu_inter = (cur_tabu_inter + tenure_step_inter).min(max_inter);
                if grew {
                    // タブーリストを新しいテニュアで再生成（既存エントリは保持しない）
                    tabu_l1    = TabuList::new(cur_tabu_l1);
                    tabu_l2    = TabuList::new(cur_tabu_l2);
                    tabu_inter = TabuList::new(cur_tabu_inter);
                }
            }
        }

        // ——————————————————
        // ログ
        // ——————————————————
        if iter % config.log_interval == 0 {
            eprintln!(
                "iter {:>6} | current {:.4} | best {:.4} | no_improve {:>5} | tenure l1={} l2={} inter={}{}",
                iter, current_score, best_score, no_improve,
                cur_tabu_l1, cur_tabu_l2, cur_tabu_inter,
                if restarts > 0 { format!(" (restart {})", restarts) } else { String::new() }
            );
        }

        // ——————————————————
        // 再起動
        // ——————————————————
        if no_improve >= config.restart_after {
            if restarts >= config.max_restarts {
                eprintln!("最大再起動回数到達。探索終了。");
                break;
            }
            restarts  += 1;
            no_improve = 0;

            // グローバルベストから再開し、ランダム摂動を加える
            current = best.clone();
            random_perturbation(&mut current, corpus, config.perturbation_swaps, rng);
            current_score = score(&current, corpus, weights);

            // タブーリストをクリア＆テニュアを初期値にリセット
            cur_tabu_l1    = config.tabu_l1;
            cur_tabu_l2    = config.tabu_l2;
            cur_tabu_inter = config.tabu_inter;
            tabu_l1    = TabuList::new(cur_tabu_l1);
            tabu_l2    = TabuList::new(cur_tabu_l2);
            tabu_inter = TabuList::new(cur_tabu_inter);

            eprintln!("  → 再起動 #{}: 摂動後スコア={:.4}", restarts, current_score);
        }

        // ——————————————————
        // シグナルチェック
        // ——————————————————
        // SIGUSR1: 現在のベストをログに出力して探索継続
        if report_flag.swap(false, Ordering::Relaxed) {
            eprintln!("\n[SIGUSR1] 現在のベストスコア={:.4} (iter {})", best_score, iter);
        }
        // SIGINT: 探索を中断してベストを返す
        if stop_flag.load(Ordering::Relaxed) {
            eprintln!("\n[SIGINT] 割り込みシグナルを受信。探索を中断します。");
            break;
        }
    }

    eprintln!(
        "探索完了: {} iter, {} restarts | 最良スコア={:.4}",
        iter, restarts, best_score
    );
    best
}

/// ——————————————————————————————
/// ヘルパー: Layer 1 の可動文字リストを収集
/// ——————————————————————————————
fn collect_l1_free_chars(layout: &Layout) -> Vec<CharId> {
    (0..NUM_CHARS as CharId)
        .filter(|&c| layout.is_l1(c) && !is_fixed(c))
        .collect()
}

/// Layer 2 文字リスト
fn collect_l2_chars(layout: &Layout) -> Vec<CharId> {
    (0..NUM_CHARS as CharId)
        .filter(|&c| !layout.is_l1(c))
        .collect()
}

/// ——————————————————————————————
/// 操作A/B: 同レイヤー内スワップの候補を生成
/// 候補数が多い場合はランダムサンプリング
/// ——————————————————————————————
fn generate_swap_candidates(
    layout: &Layout,
    corpus: &Corpus,
    weights: &Weights,
    chars: &[CharId],
    kind: OpKind,
    sample_limit: usize,
    rng: &mut impl Rng,
    out: &mut Vec<Candidate>,
) {
    let n = chars.len();
    if n < 2 { return; }

    let max_pairs = n * (n - 1) / 2;
    if max_pairs <= sample_limit {
        // 全ペアを評価
        for i in 0..n {
            for j in i + 1..n {
                let (c1, c2) = (chars[i], chars[j]);
                let delta = delta_score(layout, corpus, weights, c1, c2);
                out.push(Candidate { kind, c1, c2, delta });
            }
        }
    } else {
        // ランダムサンプリング
        let mut sampled = 0;
        let mut tries = 0;
        while sampled < sample_limit && tries < sample_limit * 4 {
            tries += 1;
            let i = rng.gen_range(0..n);
            let j = rng.gen_range(0..n);
            if i == j { continue; }
            let (c1, c2) = (chars[i], chars[j]);
            let delta = delta_score(layout, corpus, weights, c1, c2);
            out.push(Candidate { kind, c1, c2, delta });
            sampled += 1;
        }
    }
}

/// ——————————————————————————————
/// 操作C: 層間スワップ候補を頻度差ベースサンプリングで生成
///
/// 戦略: L1の低頻度文字 × L2の高頻度文字 のペアを優先
/// ——————————————————————————————
fn generate_inter_layer_candidates(
    layout: &Layout,
    corpus: &Corpus,
    weights: &Weights,
    n_samples: usize,
    rng: &mut impl Rng,
    out: &mut Vec<Candidate>,
) {
    // 層間移動可能なL1文字を頻度昇順で収集
    let mut l1_chars: Vec<(CharId, f64)> = (0..NUM_CHARS as CharId)
        .filter(|&c| layout.is_l1(c) && is_inter_layer_movable(c))
        .map(|c| (c, corpus.unigrams[c as usize]))
        .collect();
    l1_chars.sort_unstable_by(|a, b| a.1.total_cmp(&b.1)); // 低頻度先

    // L2文字を頻度降順で収集
    let mut l2_chars: Vec<(CharId, f64)> = (0..NUM_CHARS as CharId)
        .filter(|&c| !layout.is_l1(c))
        .map(|c| (c, corpus.unigrams[c as usize]))
        .collect();
    l2_chars.sort_unstable_by(|a, b| b.1.total_cmp(&a.1)); // 高頻度先

    if l1_chars.is_empty() || l2_chars.is_empty() { return; }

    // 重みベクトル生成（低頻度/高頻度側ほど高確率でサンプリング）
    // L1側: 1/(rank+1) の確率分布（低頻度 = 低rank = 高確率）
    let l1_weights: Vec<f64> = (0..l1_chars.len())
        .map(|r| 1.0 / (r + 1) as f64)
        .collect();
    let l2_weights: Vec<f64> = (0..l2_chars.len())
        .map(|r| 1.0 / (r + 1) as f64)
        .collect();

    let l1_w_sum: f64 = l1_weights.iter().sum();
    let l2_w_sum: f64 = l2_weights.iter().sum();

    let mut sampled = 0;
    let mut tries = 0;
    while sampled < n_samples && tries < n_samples * 5 {
        tries += 1;

        // 重みつきランダムサンプリング
        let c1 = weighted_choice(&l1_chars, &l1_weights, l1_w_sum, rng).0;
        let c2 = weighted_choice(&l2_chars, &l2_weights, l2_w_sum, rng).0;

        let delta = delta_score(layout, corpus, weights, c1, c2);
        out.push(Candidate {
            kind: OpKind::InterLayer,
            c1,
            c2,
            delta,
        });
        sampled += 1;
    }
}

fn weighted_choice<T: Copy>(
    items: &[(T, f64)],
    weights: &[f64],
    w_sum: f64,
    rng: &mut impl Rng,
) -> (T, f64) {
    let mut r = rng.gen::<f64>() * w_sum;
    for (i, &w) in weights.iter().enumerate() {
        r -= w;
        if r <= 0.0 {
            return items[i];
        }
    }
    *items.last().unwrap()
}

/// ——————————————————————————————
/// ランダム摂動（再起動時）
/// L1/L2間の層間スワップを n_swaps 回ランダムに実行
/// ——————————————————————————————
pub fn random_perturbation(
    layout: &mut Layout,
    _corpus: &Corpus,
    n_swaps: usize,
    rng: &mut impl Rng,
) {
    let l1_chars: Vec<CharId> = (0..NUM_CHARS as CharId)
        .filter(|&c| layout.is_l1(c) && is_inter_layer_movable(c))
        .collect();
    let l2_chars: Vec<CharId> = (0..NUM_CHARS as CharId)
        .filter(|&c| !layout.is_l1(c))
        .collect();

    if l1_chars.is_empty() || l2_chars.is_empty() { return; }

    for _ in 0..n_swaps {
        let c1 = *l1_chars.choose(rng).unwrap();
        let c2 = *l2_chars.choose(rng).unwrap();
        layout.swap_chars(c1, c2);
    }
}

/// ——————————————————————————————
/// 初期解生成：頻度上位の文字をLayer 1へ配置
/// ——————————————————————————————
pub fn build_initial_layout(corpus: &Corpus) -> Layout {
    use crate::chars::{DAKUTEN_ID, HANDAKUTEN_ID, TOUTEN_ID, KUTEN_ID};
    use crate::layout::Layout;

    let mut layout = Layout::initial();

    // Layer 1の固定文字（動かさない）
    // TOUTEN_ID(12=、), KUTEN_ID(17=。) → 既に正しいスロットにある

    // Layer 1に置く文字を頻度で決める
    // 固定文字: 、。゛゜ の4文字はL1確定、残り26スロットを頻度で埋める
    const L1_FREE_SLOTS: usize = 26; // 30 - 4(固定)

    // 動かせる全文字を頻度降順にソート
    let mut movable: Vec<(CharId, f64)> = (0..NUM_CHARS as CharId)
        .filter(|&c| c != TOUTEN_ID && c != KUTEN_ID && c != DAKUTEN_ID && c != HANDAKUTEN_ID)
        .map(|c| (c, corpus.unigrams[c as usize]))
        .collect();
    movable.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));

    // 現在の配置を確認: 既にL1にいる文字を L2 に移動しないといけない
    // 初期値では CharId i == SlotId i なので:
    //   CharId 0-29 → L1（ただし固定4文字は確定）
    //   CharId 30-59 → L2

    // 頻度上位26文字をL1に、残りをL2に割り当てる
    // 現在L2にいる文字（CharId 30+）がL1候補なら層間スワップ
    let l1_targets: Vec<CharId> = movable.iter()
        .take(L1_FREE_SLOTS)
        .map(|&(c, _)| c)
        .collect();

    // l1_targetsに含まれない文字でL1にいるものをL2に降格
    // →L1に上げる文字と対で交換する
    let l1_target_set: std::collections::HashSet<CharId> =
        l1_targets.iter().copied().collect();

    // 現在L1にいる（動かせる）文字のうち、topに入っていないものをキュー化
    let mut to_demote: std::collections::VecDeque<CharId> = (0..NUM_CHARS as CharId)
        .filter(|&c| {
            layout.is_l1(c)
                && c != TOUTEN_ID && c != KUTEN_ID
                && c != DAKUTEN_ID && c != HANDAKUTEN_ID
                && !l1_target_set.contains(&c)
        })
        .collect();

    // L2にいてL1に昇格すべき文字のキュー
    let mut to_promote: std::collections::VecDeque<CharId> = l1_targets.iter()
        .copied()
        .filter(|&c| !layout.is_l1(c))
        .collect();

    // ペアで層間スワップ
    while let (Some(demote), Some(promote)) = (to_demote.pop_front(), to_promote.pop_front()) {
        layout.swap_chars(demote, promote);
    }

    eprintln!("初期解生成完了。L1に配置: {:?}", {
        use crate::chars::CHAR_LIST;
        (0..NUM_CHARS as CharId)
            .filter(|&c| layout.is_l1(c))
            .map(|c| CHAR_LIST[c as usize])
            .collect::<String>()
    });

    layout
}

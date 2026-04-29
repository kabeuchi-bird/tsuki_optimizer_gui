// search.rs — タブーサーチ本体

use rand::prelude::*;
use std::collections::HashSet;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::chars::{CharId, MAX_CHARS, VOID_CHAR_FIRST};
use crate::corpus::Corpus;
use crate::cost::{delta_score, score, DeltaScoreBuffer, Weights};
use crate::layout::{
    is_fixed, is_inter_layer_movable, swap_would_violate, ExclusivePair, KeyboardParams, Layout,
    SHIFT_SLOT_SENTINEL,
};

/// ——————————————————————————————
/// タブーリスト（circular buffer）
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
        self.entries.contains(&key)
    }

    fn add(&mut self, c1: CharId, c2: CharId) {
        if self.capacity == 0 {
            return;
        }
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
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// ——————————————————————————————
/// 探索コンテキスト（静的な入力データをまとめる）
/// ——————————————————————————————
pub struct SearchContext<'a> {
    pub corpus: &'a Corpus,
    pub weights: &'a Weights,
    pub pairs: &'a [ExclusivePair],
    pub l1_only: &'a HashSet<CharId>,
}

/// ——————————————————————————————
/// 探索フェーズ（GUI 通信用）
/// ——————————————————————————————
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchPhase {
    Running,
    Restarting,
    Finished,
}

/// ——————————————————————————————
/// 探索状態の更新通知（GUI 通信用）
/// ——————————————————————————————
#[derive(Clone)]
pub struct SearchUpdate {
    pub iter: usize,
    pub restarts: usize,
    pub current_score: f64,
    pub best_score: f64,
    pub best_layout: Layout,
    pub phase: SearchPhase,
    /// ユニグラム頻度（GUI の色分け・指負荷計算用）
    pub unigrams: [f64; MAX_CHARS],
}

/// ——————————————————————————————
/// 操作の種類
/// ——————————————————————————————
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    SwapL1,
    SwapL2,
    InterLayer,
}

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
    pub max_iter: usize,
    pub restart_after: usize,
    pub max_restarts: usize,
    pub tabu_l1: usize,
    pub tabu_l2: usize,
    pub tabu_inter: usize,
    pub inter_sample: usize,
    pub ab_sample_limit: usize,
    pub log_interval: usize,
    pub perturbation_swaps: usize,
    pub tenure_grow_threshold: f64,
    pub tenure_grow_interval: usize,
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
            tenure_grow_interval: 200,
            tenure_max_scale: 3.0,
        }
    }
}

impl SearchConfig {
    /// 設定値を検証し、問題があれば警告メッセージを返す
    pub fn validate(&self, out: &mut impl Write) {
        if self.max_iter == 0 {
            let _ = writeln!(out, "警告: max_iter=0 → 探索は即座に終了します");
        }
        if self.log_interval == 0 {
            let _ = writeln!(out, "警告: log_interval=0 → ログ出力を無効化します");
        }
        if self.tenure_grow_interval == 0 {
            let _ = writeln!(out, "警告: tenure_grow_interval=0 → テニュア拡大を無効化します");
        }
        if self.restart_after == 0 {
            let _ = writeln!(out, "情報: restart_after=0 → 再起動なしで探索します");
        }
    }
}

/// ——————————————————————————————
/// タブーサーチ本体
/// ——————————————————————————————
#[allow(clippy::too_many_arguments)]
pub fn run(
    initial_layout: Layout,
    ctx: &SearchContext,
    config: &SearchConfig,
    rng: &mut impl Rng,
    stop_flag: &Arc<AtomicBool>,
    report_flag: &Arc<AtomicBool>,
    on_update: &mut impl FnMut(&SearchUpdate),
    out: &mut impl Write,
) -> Layout {
    let mut current = initial_layout;
    let mut current_score = score(&current, ctx.corpus, ctx.weights);

    let mut best = current.clone();
    let mut best_score = current_score;

    let mut no_improve = 0usize;
    let mut restarts = 0usize;
    let mut iter = 0usize;

    let mut cur_tabu_l1 = config.tabu_l1;
    let mut cur_tabu_l2 = config.tabu_l2;
    let mut cur_tabu_inter = config.tabu_inter;
    let tenure_grow_start = (config.restart_after as f64 * config.tenure_grow_threshold) as usize;
    let grow_period = config
        .restart_after
        .saturating_sub(tenure_grow_start)
        .max(1);
    let tenure_step_l1 = (config.tabu_l1 as f64
        * (config.tenure_max_scale - 1.0)
        * config.tenure_grow_interval as f64
        / grow_period as f64)
        .ceil()
        .max(1.0) as usize;
    let tenure_step_l2 = (config.tabu_l2 as f64
        * (config.tenure_max_scale - 1.0)
        * config.tenure_grow_interval as f64
        / grow_period as f64)
        .ceil()
        .max(1.0) as usize;
    let tenure_step_inter = (config.tabu_inter as f64
        * (config.tenure_max_scale - 1.0)
        * config.tenure_grow_interval as f64
        / grow_period as f64)
        .ceil()
        .max(1.0) as usize;

    let mut tabu_l1 = TabuList::new(cur_tabu_l1);
    let mut tabu_l2 = TabuList::new(cur_tabu_l2);
    let mut tabu_inter = TabuList::new(cur_tabu_inter);

    // 再利用バッファ（ループ外で確保してループ内で clear() して使い回す）
    let mut candidates: Vec<Candidate> =
        Vec::with_capacity(config.ab_sample_limit * 2 + config.inter_sample);
    let mut l1_free: Vec<CharId> = Vec::with_capacity(current.kp.num_chars);
    let mut l2_free: Vec<CharId> = Vec::with_capacity(current.kp.num_chars);
    let mut delta_buf = DeltaScoreBuffer::new(ctx.corpus.bigrams.len(), ctx.corpus.trigrams.len());

    while iter < config.max_iter {
        iter += 1;

        candidates.clear();

        collect_l1_free_chars_into(&current, &mut l1_free);
        generate_swap_candidates(
            &current,
            ctx,
            &l1_free,
            OpKind::SwapL1,
            config.ab_sample_limit,
            rng,
            &mut candidates,
            &mut delta_buf,
        );

        collect_l2_chars_into(&current, &mut l2_free);
        generate_swap_candidates(
            &current,
            ctx,
            &l2_free,
            OpKind::SwapL2,
            config.ab_sample_limit,
            rng,
            &mut candidates,
            &mut delta_buf,
        );

        generate_inter_layer_candidates(
            &current,
            ctx,
            config.inter_sample,
            rng,
            &mut candidates,
            &mut delta_buf,
        );

        if candidates.is_empty() {
            break;
        }

        // O(n) で最良候補を選択（ソート不要）
        // best_free: タブーでない最良候補
        // best_aspiration: タブーだがベストスコアを更新する最良候補
        let mut best_free: Option<Candidate> = None;
        let mut best_aspiration: Option<Candidate> = None;
        let aspiration_threshold = best_score - current_score;

        for &cand in &candidates {
            let is_tabu = match cand.kind {
                OpKind::SwapL1 => tabu_l1.contains(cand.c1, cand.c2),
                OpKind::SwapL2 => tabu_l2.contains(cand.c1, cand.c2),
                OpKind::InterLayer => tabu_inter.contains(cand.c1, cand.c2),
            };
            if !is_tabu {
                if best_free.is_none() || cand.delta < best_free.unwrap().delta {
                    best_free = Some(cand);
                }
            } else if cand.delta < aspiration_threshold
                && (best_aspiration.is_none() || cand.delta < best_aspiration.unwrap().delta)
            {
                best_aspiration = Some(cand);
            }
        }

        let chosen = match (best_free, best_aspiration) {
            (Some(f), Some(a)) => {
                if a.delta < f.delta { a } else { f }
            }
            (Some(f), None) => f,
            (None, Some(a)) => a,
            (None, None) => continue,
        };

        current.swap_chars(chosen.c1, chosen.c2);
        current_score += chosen.delta;

        match chosen.kind {
            OpKind::SwapL1 => tabu_l1.add(chosen.c1, chosen.c2),
            OpKind::SwapL2 => tabu_l2.add(chosen.c1, chosen.c2),
            OpKind::InterLayer => tabu_inter.add(chosen.c1, chosen.c2),
        }

        if current_score < best_score {
            best_score = current_score;
            best = current.clone();
            no_improve = 0;
            on_update(&SearchUpdate {
                iter,
                restarts,
                current_score,
                best_score,
                best_layout: best.clone(),
                unigrams: ctx.corpus.unigrams,
                phase: SearchPhase::Running,
            });
            if cur_tabu_l1 != config.tabu_l1
                || cur_tabu_l2 != config.tabu_l2
                || cur_tabu_inter != config.tabu_inter
            {
                cur_tabu_l1 = config.tabu_l1;
                cur_tabu_l2 = config.tabu_l2;
                cur_tabu_inter = config.tabu_inter;
                tabu_l1 = TabuList::new(cur_tabu_l1);
                tabu_l2 = TabuList::new(cur_tabu_l2);
                tabu_inter = TabuList::new(cur_tabu_inter);
            }
        } else {
            no_improve += 1;
            if config.tenure_grow_interval > 0
                && no_improve > tenure_grow_start
                && (no_improve - tenure_grow_start).is_multiple_of(config.tenure_grow_interval)
            {
                let max_l1 = (config.tabu_l1 as f64 * config.tenure_max_scale) as usize;
                let max_l2 = (config.tabu_l2 as f64 * config.tenure_max_scale) as usize;
                let max_inter = (config.tabu_inter as f64 * config.tenure_max_scale) as usize;
                let grew =
                    cur_tabu_l1 < max_l1 || cur_tabu_l2 < max_l2 || cur_tabu_inter < max_inter;
                cur_tabu_l1 = (cur_tabu_l1 + tenure_step_l1).min(max_l1);
                cur_tabu_l2 = (cur_tabu_l2 + tenure_step_l2).min(max_l2);
                cur_tabu_inter = (cur_tabu_inter + tenure_step_inter).min(max_inter);
                if grew {
                    tabu_l1 = TabuList::new(cur_tabu_l1);
                    tabu_l2 = TabuList::new(cur_tabu_l2);
                    tabu_inter = TabuList::new(cur_tabu_inter);
                }
            }
        }

        if config.log_interval > 0 && iter.is_multiple_of(config.log_interval) {
            let _ = writeln!(out,
                "iter {:>6} | current {:.4} | best {:.4} | no_improve {:>5} | tenure l1={} l2={} inter={}{}",
                iter, current_score, best_score, no_improve,
                cur_tabu_l1, cur_tabu_l2, cur_tabu_inter,
                if restarts > 0 { format!(" (restart {})", restarts) } else { String::new() }
            );
            on_update(&SearchUpdate {
                iter,
                restarts,
                current_score,
                best_score,
                best_layout: best.clone(),
                unigrams: ctx.corpus.unigrams,
                phase: SearchPhase::Running,
            });
        }

        if config.restart_after > 0 && no_improve >= config.restart_after {
            if restarts >= config.max_restarts {
                let _ = writeln!(out, "最大再起動回数到達。探索終了。");
                break;
            }
            restarts += 1;
            no_improve = 0;

            current = best.clone();
            random_perturbation(
                &mut current,
                config.perturbation_swaps,
                rng,
                ctx.pairs,
                ctx.l1_only,
            );
            current_score = score(&current, ctx.corpus, ctx.weights);

            cur_tabu_l1 = config.tabu_l1;
            cur_tabu_l2 = config.tabu_l2;
            cur_tabu_inter = config.tabu_inter;
            tabu_l1 = TabuList::new(cur_tabu_l1);
            tabu_l2 = TabuList::new(cur_tabu_l2);
            tabu_inter = TabuList::new(cur_tabu_inter);

            let _ = writeln!(
                out,
                "  → 再起動 #{}: 摂動後スコア={:.4}",
                restarts, current_score
            );
            on_update(&SearchUpdate {
                iter,
                restarts,
                current_score,
                best_score,
                best_layout: best.clone(),
                unigrams: ctx.corpus.unigrams,
                phase: SearchPhase::Restarting,
            });
        }

        if report_flag.swap(false, Ordering::Relaxed) {
            let _ = writeln!(
                out,
                "\n[SIGUSR1] 現在のベスト配列 (スコア={:.4}, iter {})",
                best_score, iter
            );
            best.display(out);
        }
        if stop_flag.load(Ordering::Relaxed) {
            let _ = writeln!(out, "\n[SIGINT] 割り込みシグナルを受信。探索を中断します。");
            break;
        }
    }

    let _ = writeln!(
        out,
        "探索完了: {} iter, {} restarts | 最良スコア={:.4}",
        iter, restarts, best_score
    );
    let final_update = SearchUpdate {
        iter,
        restarts,
        current_score,
        best_score,
        best_layout: best,
        unigrams: ctx.corpus.unigrams,
        phase: SearchPhase::Finished,
    };
    on_update(&final_update);
    final_update.best_layout
}

// ──────────────────────────────────────────────────────────────
// ヘルパー関数
// ──────────────────────────────────────────────────────────────

/// Layer 1 の可動文字（固定文字を除く）を既存 Vec に収集（再利用版）
fn collect_l1_free_chars_into(layout: &Layout, out: &mut Vec<CharId>) {
    out.clear();
    let kp = layout.kp;
    for c in 0..kp.num_chars as CharId {
        if layout.is_l1(c) && !is_fixed(c, kp) && !is_void(c) {
            out.push(c);
        }
    }
}

/// Layer 2 の文字（void 除く）を既存 Vec に収集（再利用版）
fn collect_l2_chars_into(layout: &Layout, out: &mut Vec<CharId>) {
    out.clear();
    let kp = layout.kp;
    for c in 0..kp.num_chars as CharId {
        if !layout.is_l1(c) && !is_void(c) {
            out.push(c);
        }
    }
}

/// void文字（空きスロット代替）かどうか
#[inline]
fn is_void(c: CharId) -> bool {
    c >= VOID_CHAR_FIRST
}

/// 操作A/B: 同レイヤー内スワップの候補を生成
#[allow(clippy::too_many_arguments)]
fn generate_swap_candidates(
    layout: &Layout,
    ctx: &SearchContext,
    chars: &[CharId],
    kind: OpKind,
    sample_limit: usize,
    rng: &mut impl Rng,
    out: &mut Vec<Candidate>,
    buf: &mut DeltaScoreBuffer,
) {
    let n = chars.len();
    if n < 2 {
        return;
    }

    let max_pairs = n * (n - 1) / 2;
    if max_pairs <= sample_limit {
        for i in 0..n {
            for j in i + 1..n {
                let (c1, c2) = (chars[i], chars[j]);
                if swap_would_violate(layout, c1, c2, ctx.pairs) {
                    continue;
                }
                let delta = delta_score(layout, ctx.corpus, ctx.weights, c1, c2, buf);
                out.push(Candidate {
                    kind,
                    c1,
                    c2,
                    delta,
                });
            }
        }
    } else {
        let mut sampled = 0;
        let mut tries = 0;
        while sampled < sample_limit && tries < sample_limit * 4 {
            tries += 1;
            let i = rng.gen_range(0..n);
            let j = rng.gen_range(0..n);
            if i == j {
                continue;
            }
            let (c1, c2) = (chars[i], chars[j]);
            if swap_would_violate(layout, c1, c2, ctx.pairs) {
                continue;
            }
            let delta = delta_score(layout, ctx.corpus, ctx.weights, c1, c2, buf);
            out.push(Candidate {
                kind,
                c1,
                c2,
                delta,
            });
            sampled += 1;
        }
    }
}

/// 操作C: 層間スワップ候補を頻度差ベースサンプリングで生成
fn generate_inter_layer_candidates(
    layout: &Layout,
    ctx: &SearchContext,
    n_samples: usize,
    rng: &mut impl Rng,
    out: &mut Vec<Candidate>,
    buf: &mut DeltaScoreBuffer,
) {
    let kp = layout.kp;

    let mut l1_chars: Vec<(CharId, f64)> = (0..kp.num_chars as CharId)
        .filter(|&c| layout.is_l1(c) && is_inter_layer_movable(c, kp, ctx.l1_only) && !is_void(c))
        .map(|c| (c, ctx.corpus.unigrams[c as usize]))
        .collect();
    l1_chars.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));

    let mut l2_chars: Vec<(CharId, f64)> = (0..kp.num_chars as CharId)
        .filter(|&c| !layout.is_l1(c) && !is_void(c))
        .map(|c| (c, ctx.corpus.unigrams[c as usize]))
        .collect();
    l2_chars.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));

    if l1_chars.is_empty() || l2_chars.is_empty() {
        return;
    }

    let l1_weights: Vec<f64> = (0..l1_chars.len()).map(|r| 1.0 / (r + 1) as f64).collect();
    let l2_weights: Vec<f64> = (0..l2_chars.len()).map(|r| 1.0 / (r + 1) as f64).collect();
    let l1_w_sum: f64 = l1_weights.iter().sum();
    let l2_w_sum: f64 = l2_weights.iter().sum();

    let mut sampled = 0;
    let mut tries = 0;
    while sampled < n_samples && tries < n_samples * 5 {
        tries += 1;
        let c1 = weighted_choice(&l1_chars, &l1_weights, l1_w_sum, rng).0;
        let c2 = weighted_choice(&l2_chars, &l2_weights, l2_w_sum, rng).0;
        if swap_would_violate(layout, c1, c2, ctx.pairs) {
            continue;
        }
        let delta = delta_score(layout, ctx.corpus, ctx.weights, c1, c2, buf);
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

/// ランダム摂動（再起動時）
fn random_perturbation(
    layout: &mut Layout,
    n_swaps: usize,
    rng: &mut impl Rng,
    pairs: &[ExclusivePair],
    l1_only: &HashSet<CharId>,
) {
    let kp = layout.kp;
    let l1_chars: Vec<CharId> = (0..kp.num_chars as CharId)
        .filter(|&c| layout.is_l1(c) && is_inter_layer_movable(c, kp, l1_only) && !is_void(c))
        .collect();
    let l2_chars: Vec<CharId> = (0..kp.num_chars as CharId)
        .filter(|&c| !layout.is_l1(c) && !is_void(c))
        .collect();

    if l1_chars.is_empty() || l2_chars.is_empty() {
        return;
    }

    for _ in 0..n_swaps {
        let c1 = *l1_chars.choose(rng).unwrap();
        let c2 = *l2_chars.choose(rng).unwrap();
        if swap_would_violate(layout, c1, c2, pairs) {
            continue;
        }
        layout.swap_chars(c1, c2);
    }
}

/// ——————————————————————————————
/// 初期解生成：頻度上位の文字をLayer 1へ配置
/// ——————————————————————————————
pub fn build_initial_layout(
    ctx: &SearchContext,
    kp: KeyboardParams,
    out: &mut impl Write,
) -> Layout {
    let mut layout = Layout::initial(kp);

    // L1に確定固定される文字：
    //   3x10: 。(KUTEN)、、(TOUTEN)、゛(DAKUTEN)、゜(HANDAKUTEN) → 4文字
    //   3x11: ゛(DAKUTEN)、゜(HANDAKUTEN) → 2文字（。と、は自由移動可）
    //
    // L1キャラクタースロット数：
    //   3x10: 30 - 0（シフトスロットなし）= 30
    //   3x11: 33 - 2（☆★スロット）= 31
    //
    // L1の自由スロット数（頻度上位でうめる枠）:
    //   3x10: 30 - 4（固定）= 26
    //   3x11: 31 - 2（l1_only）= 29

    let l1_char_slots = kp.num_slots_per_layer as usize
        - if kp.size == crate::layout::KeyboardSize::K3x11 {
            2
        } else {
            0
        };

    let l1_fixed_count = match kp.size {
        crate::layout::KeyboardSize::K3x10 => 4, // 。、゛゜
        crate::layout::KeyboardSize::K3x11 => 2, // ゛゜のみ（。、は自由）
    };
    let l1_free_slots = l1_char_slots - l1_fixed_count;

    // 動かせる全文字を頻度降順にソート
    let mut movable: Vec<(CharId, f64)> = (0..kp.num_chars as CharId)
        .filter(|&c| !is_fixed(c, kp) && !ctx.l1_only.contains(&c) && !is_void(c))
        .map(|c| (c, ctx.corpus.unigrams[c as usize]))
        .collect();
    movable.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));

    // 頻度上位 l1_free_slots 文字をL1ターゲットとする
    let l1_targets: Vec<CharId> = movable
        .iter()
        .take(l1_free_slots)
        .map(|&(c, _)| c)
        .collect();

    let l1_target_set: std::collections::HashSet<CharId> = l1_targets.iter().copied().collect();

    // 現在L1にいる（動かせる）文字のうち、targetに入っていないものをL2に降格
    let mut to_demote: std::collections::VecDeque<CharId> = (0..kp.num_chars as CharId)
        .filter(|&c| {
            layout.is_l1(c)
                && !is_fixed(c, kp)
                && !ctx.l1_only.contains(&c)
                && !is_void(c)
                && !l1_target_set.contains(&c)
        })
        .collect();

    // L2にいてL1に昇格すべき文字のキュー
    let mut to_promote: std::collections::VecDeque<CharId> = l1_targets
        .iter()
        .copied()
        .filter(|&c| !layout.is_l1(c))
        .collect();

    // ペアで層間スワップ
    while let (Some(demote), Some(promote)) = (to_demote.pop_front(), to_promote.pop_front()) {
        layout.swap_chars(demote, promote);
    }

    // 排他ペア制約の初期違反を greedy 修正（L2 同士をスワップして解消）
    if !ctx.pairs.is_empty() {
        let npl = kp.num_slots_per_layer as usize;
        for _pass in 0..20 {
            let mut any_violation = false;
            for l1_slot in 0..npl {
                let l2_slot = l1_slot + npl;
                let l1_c = layout.slot_to_char[l1_slot];
                let l2_c = layout.slot_to_char[l2_slot];
                // SHIFT_SLOT_SENTINEL(255) と void(>=62) を除外
                if l1_c >= VOID_CHAR_FIRST || l2_c >= VOID_CHAR_FIRST {
                    continue;
                }
                if !ctx.pairs.iter().any(|p| p.violates(l1_c, l2_c)) {
                    continue;
                }

                any_violation = true;
                let mut fixed = false;
                'fix: for alt_l1_slot in 0..npl {
                    let alt_l2_slot = alt_l1_slot + npl;
                    let alt_l2_c = layout.slot_to_char[alt_l2_slot];
                    if alt_l2_c >= VOID_CHAR_FIRST || alt_l2_c == l2_c {
                        continue;
                    }
                    // スワップ後: l1_slot側は (l1_c, alt_l2_c)、alt_l1_slot側は (alt_l1_c, l2_c)
                    if ctx.pairs.iter().any(|p| p.violates(l1_c, alt_l2_c)) {
                        continue;
                    }
                    let alt_l1_c = layout.slot_to_char[alt_l1_slot];
                    if alt_l1_c != SHIFT_SLOT_SENTINEL
                        && alt_l1_c < VOID_CHAR_FIRST
                        && ctx.pairs.iter().any(|p| p.violates(alt_l1_c, l2_c))
                    {
                        continue;
                    }
                    layout.swap_chars(l2_c, alt_l2_c);
                    fixed = true;
                    break 'fix;
                }
                if !fixed {
                    let _ = writeln!(
                        out,
                        "警告: 排他ペア制約の初期違反を修正できませんでした (L1スロット{})",
                        l1_slot
                    );
                }
            }
            if !any_violation {
                break;
            }
        }
    }

    let _ = writeln!(out, "初期解生成完了。L1に配置: {:?}", {
        use crate::chars::CHAR_LIST;
        (0..kp.num_chars as CharId)
            .filter(|&c| layout.is_l1(c) && !is_void(c))
            .map(|c| CHAR_LIST[c as usize])
            .collect::<String>()
    });

    layout
}

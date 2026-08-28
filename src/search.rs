// search.rs — タブーサーチ本体

use rand::prelude::*;
use std::collections::HashSet;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::chars::{is_base_kana, is_l1l2_void, CharId, CONSONANT_FIRST, MAX_CHARS};
use crate::corpus::Corpus;
use crate::cost::{delta_score, score, unigram_cost_for_slot, DeltaScoreBuffer, Weights};
use crate::layout::{
    is_fixed, is_inter_layer_movable, swap_would_violate, yoon_physical_forbidden, ExclusivePair,
    KeyboardParams, Layout, SlotId, SHIFT_SLOT_SENTINEL,
};

/// ——————————————————————————————
/// タブーリスト（circular buffer）
/// ——————————————————————————————
struct TabuList {
    entries: Vec<(CharId, CharId)>,
    capacity: usize,
    head: usize,
    bitset: [u64; VALID_WORDS],
}

impl TabuList {
    fn new(capacity: usize) -> Self {
        TabuList {
            entries: Vec::with_capacity(capacity),
            capacity,
            head: 0,
            bitset: [0u64; VALID_WORDS],
        }
    }

    #[inline]
    fn contains(&self, c1: CharId, c2: CharId) -> bool {
        let (a, b) = normalize_pair(c1, c2);
        bitset_test(&self.bitset, pair_index(a as usize, b as usize))
    }

    fn add(&mut self, c1: CharId, c2: CharId) {
        if self.capacity == 0 {
            return;
        }
        let key = normalize_pair(c1, c2);
        let idx = pair_index(key.0 as usize, key.1 as usize);
        if bitset_test(&self.bitset, idx) {
            return;
        }
        if self.entries.len() < self.capacity {
            self.entries.push(key);
        } else {
            let old = self.entries[self.head];
            bitset_clear(&mut self.bitset, pair_index(old.0 as usize, old.1 as usize));
            self.entries[self.head] = key;
            self.head = (self.head + 1) % self.capacity;
        }
        bitset_set(&mut self.bitset, idx);
    }
}

/// ——————————————————————————————
/// 操作種別ごとのタブーリスト一式（テニュアの拡大・リセットを含む）
///
/// L1内 / L2内 / 層間 / 拗音面内 の4種を `OpKind` で添字づけして扱い、
/// 操作種別を増やしても run() 側の分岐が増えないようにする。
/// ——————————————————————————————
struct TabuSet {
    lists: [TabuList; NUM_OP_KINDS],
    /// 設定値（リセット時に戻す基準）
    base: [usize; NUM_OP_KINDS],
    /// 現在のテニュア
    cur: [usize; NUM_OP_KINDS],
    /// 1回の拡大ステップ
    step: [usize; NUM_OP_KINDS],
    /// テニュア上限
    max: [usize; NUM_OP_KINDS],
}

impl TabuSet {
    /// 設定と拡大パラメータからタブーリスト一式を構築する
    fn new(base: [usize; NUM_OP_KINDS], grow_period: usize, config: &SearchConfig) -> Self {
        let step = base.map(|b| {
            (b as f64 * (config.tenure_max_scale - 1.0) * config.tenure_grow_interval as f64
                / grow_period as f64)
                .ceil()
                .max(1.0) as usize
        });
        let max = base.map(|b| (b as f64 * config.tenure_max_scale) as usize);
        TabuSet {
            lists: std::array::from_fn(|i| TabuList::new(base[i])),
            base,
            cur: base,
            step,
            max,
        }
    }

    #[inline]
    fn contains(&self, kind: OpKind, c1: CharId, c2: CharId) -> bool {
        self.lists[kind as usize].contains(c1, c2)
    }

    #[inline]
    fn add(&mut self, kind: OpKind, c1: CharId, c2: CharId) {
        self.lists[kind as usize].add(c1, c2);
    }

    /// 現在のテニュアでリストを作り直す（内容は破棄される）
    fn rebuild(&mut self) {
        for (list, &cap) in self.lists.iter_mut().zip(self.cur.iter()) {
            *list = TabuList::new(cap);
        }
    }

    /// テニュアを設定値へ戻す（改善時）。
    ///
    /// テニュアが拡大されていた場合のみ作り直す。`rebuild()` は内容を破棄するので、
    /// 「拡大されていたら中身ごとリセット、拡大されていなければ何もしない」という挙動になる。
    fn reset(&mut self) {
        if self.cur != self.base {
            self.cur = self.base;
            self.rebuild();
        }
    }

    /// テニュアを設定値へ戻し、タブー内容を必ず破棄する（再起動時）。
    ///
    /// `reset()` と違い、テニュアが設定値のままでも作り直す。
    fn reset_and_clear(&mut self) {
        self.cur = self.base;
        self.rebuild();
    }

    /// テニュアを1ステップ拡大する。上限未満のものがあれば作り直して true を返す。
    fn grow(&mut self) -> bool {
        let grew = self.cur.iter().zip(self.max.iter()).any(|(c, m)| c < m);
        for i in 0..NUM_OP_KINDS {
            self.cur[i] = (self.cur[i] + self.step[i]).min(self.max[i]);
        }
        if grew {
            self.rebuild();
        }
        grew
    }

    /// ログ表示用の現在テニュア（"l1=15 l2=15 inter=25 [yoon=15]"）
    fn tenure_summary(&self, yoon: bool) -> String {
        let mut s = format!(
            "l1={} l2={} inter={}",
            self.cur[OpKind::SwapL1 as usize],
            self.cur[OpKind::SwapL2 as usize],
            self.cur[OpKind::InterLayer as usize]
        );
        if yoon {
            s.push_str(&format!(" yoon={}", self.cur[OpKind::SwapYoon as usize]));
        }
        s
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

/// ペアの三角行列インデックス（TabuList / DeltaPairCache 共用）
const NUM_PAIRS: usize = MAX_CHARS * (MAX_CHARS - 1) / 2;
const BITS_PER_WORD: usize = u64::BITS as usize;
const VALID_WORDS: usize = NUM_PAIRS.div_ceil(BITS_PER_WORD);

// dirty mask が u128 に収まることの静的検証
const _: () = assert!(MAX_CHARS <= 128, "MAX_CHARS must be <= 128 for u128 dirty mask");

#[inline]
fn pair_index(a: usize, b: usize) -> usize {
    debug_assert!(a < b && b < MAX_CHARS);
    a * (2 * MAX_CHARS - a - 1) / 2 + (b - a - 1)
}

#[inline]
fn bitset_test(bitset: &[u64; VALID_WORDS], idx: usize) -> bool {
    bitset[idx / BITS_PER_WORD] & (1u64 << (idx % BITS_PER_WORD)) != 0
}

#[inline]
fn bitset_set(bitset: &mut [u64; VALID_WORDS], idx: usize) {
    bitset[idx / BITS_PER_WORD] |= 1u64 << (idx % BITS_PER_WORD);
}

#[inline]
fn bitset_clear(bitset: &mut [u64; VALID_WORDS], idx: usize) {
    bitset[idx / BITS_PER_WORD] &= !(1u64 << (idx % BITS_PER_WORD));
}

/// ——————————————————————————————
/// デルタスコアのペアキャッシュ（三角行列）
///
/// ペア (a, b) の delta_score を保持し、レイアウト変更時に
/// 影響を受けるペアだけを無効化して再計算コストを抑える。
/// ——————————————————————————————
struct DeltaPairCache {
    values: Vec<f64>,
    valid: [u64; VALID_WORDS],
}

impl DeltaPairCache {
    fn new() -> Self {
        DeltaPairCache {
            values: vec![0.0; NUM_PAIRS],
            valid: [0u64; VALID_WORDS],
        }
    }

    #[inline]
    fn get(&self, a: usize, b: usize) -> Option<f64> {
        let idx = pair_index(a, b);
        if bitset_test(&self.valid, idx) {
            Some(self.values[idx])
        } else {
            None
        }
    }

    #[inline]
    fn set(&mut self, a: usize, b: usize, value: f64) {
        let idx = pair_index(a, b);
        self.values[idx] = value;
        bitset_set(&mut self.valid, idx);
    }

    #[inline]
    fn get_or_compute(
        &mut self,
        c1: CharId,
        c2: CharId,
        layout: &Layout,
        corpus: &Corpus,
        weights: &Weights,
        buf: &mut DeltaScoreBuffer,
    ) -> f64 {
        let (a, b) = (c1.min(c2) as usize, c1.max(c2) as usize);
        if let Some(cached) = self.get(a, b) {
            return cached;
        }
        let d = delta_score(layout, corpus, weights, c1, c2, buf);
        self.set(a, b, d);
        d
    }

    /// dirty に含まれる文字が絡むペアのキャッシュを無効化する。
    ///
    /// `n_active` は実際に使われる CharId の上限（mode=none なら 64、hybrid なら
    /// 64+npl）。MAX_CHARS ではなくこれで打ち切ることで、存在しない文字IDの
    /// ペアを毎回クリアする無駄を省く。
    fn invalidate_dirty(&mut self, dirty: u128, n_active: usize) {
        let mut bits = dirty;
        while bits != 0 {
            let c = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            for other in 0..c {
                bitset_clear(&mut self.valid, pair_index(other, c));
            }
            for other in (c + 1)..n_active {
                bitset_clear(&mut self.valid, pair_index(c, other));
            }
        }
    }

    fn invalidate_all(&mut self) {
        self.valid = [0u64; VALID_WORDS];
    }
}

/// スワップ (c1, c2) でコストが変化しうる文字のビットマスク。
///
/// マスクはコーパスから決まりレイアウトに依存しないため、`Corpus::dirty_mask`
/// に前計算済み。ここは2語の OR を取るだけ。
#[inline]
fn compute_dirty_mask(corpus: &Corpus, c1: CharId, c2: CharId) -> u128 {
    corpus.dirty_mask[c1 as usize] | corpus.dirty_mask[c2 as usize]
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
    /// ユニグラム頻度（GUI の色分け・指負荷計算用）。
    /// 探索中は不変なので Arc で共有し、更新ごとの配列コピーを避ける。
    pub unigrams: Arc<[f64; MAX_CHARS]>,
}

/// ——————————————————————————————
/// 操作の種類
/// ——————————————————————————————
/// 操作の種類。`TabuSet` の添字に使うため、判別子は 0 から連番であること。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    SwapL1 = 0,
    SwapL2 = 1,
    InterLayer = 2,
    /// 拗音面内スワップ（子音↔子音、子音↔void）。hybrid のみ。
    SwapYoon = 3,
}

/// `OpKind` の種類数（`TabuSet` の配列長）
const NUM_OP_KINDS: usize = 4;

#[derive(Clone, Copy, Debug)]
struct Candidate {
    kind: OpKind,
    c1: CharId,
    c2: CharId,
    delta: f64,
}

/// 初期配列の生成方式
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InitialLayoutMode {
    /// 月配列2-263の初期配列＋頻度ソートで L1/L2 を振り分け
    #[default]
    Tsuki2_263,
    /// 制約を守りつつランダムに配字
    Random,
    /// initial_layout.toml で定義されたユーザー定義配列
    UserDefined,
}

impl InitialLayoutMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tsuki2_263 => "2-263",
            Self::Random => "ランダム",
            Self::UserDefined => "ユーザー定義",
        }
    }

    pub fn config_label(self) -> &'static str {
        match self {
            Self::Tsuki2_263 => "2-263（月配列2-263ベース）",
            Self::Random => "random（ランダム配字）",
            Self::UserDefined => "user-defined（initial_layout.toml）",
        }
    }

    pub fn from_config_str(s: &str) -> Self {
        match s {
            "random" => Self::Random,
            "2-263" => Self::Tsuki2_263,
            "user-defined" => Self::UserDefined,
            _ => {
                eprintln!("警告: 不明な initial_layout '{}' → 2-263 を使用します", s);
                Self::Tsuki2_263
            }
        }
    }
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
    /// 拗音面内スワップのタブーテニュア（hybrid のみ使用）
    pub tabu_yoon: usize,
    pub inter_sample: usize,
    pub ab_sample_limit: usize,
    pub log_interval: usize,
    pub perturbation_swaps: usize,
    pub tenure_grow_threshold: f64,
    pub tenure_grow_interval: usize,
    pub tenure_max_scale: f64,
    pub initial_layout_mode: InitialLayoutMode,
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
            tabu_yoon: 15,
            inter_sample: 80,
            ab_sample_limit: 200,
            log_interval: 1_000,
            perturbation_swaps: 8,
            tenure_grow_threshold: 0.5,
            tenure_grow_interval: 200,
            tenure_max_scale: 3.0,
            initial_layout_mode: InitialLayoutMode::default(),
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

    let tenure_grow_start = (config.restart_after as f64 * config.tenure_grow_threshold) as usize;
    let grow_period = config
        .restart_after
        .saturating_sub(tenure_grow_start)
        .max(1);
    let mut tabu = TabuSet::new(
        [
            config.tabu_l1,
            config.tabu_l2,
            config.tabu_inter,
            config.tabu_yoon,
        ],
        grow_period,
        config,
    );

    // 再利用バッファ（ループ外で確保してループ内で clear() して使い回す）
    let mut candidates: Vec<Candidate> =
        Vec::with_capacity(config.ab_sample_limit * 2 + config.inter_sample);
    let mut l1_free: Vec<CharId> = Vec::with_capacity(current.kp.num_chars);
    let mut l2_free: Vec<CharId> = Vec::with_capacity(current.kp.num_chars);
    // 拗音面の文字集合は kp から決まり探索中に変化しないので、一度だけ構築する
    let yoon_chars: Vec<CharId> = current.kp.yoon_char_range().map(|c| c as CharId).collect();
    // 実際に使われる CharId の上限（キャッシュ無効化の走査範囲）
    let n_active_chars = current.kp.yoon_char_range().end.max(current.kp.num_chars);
    // 更新通知で共有するユニグラム頻度（探索中は不変）
    let unigrams_shared = Arc::new(ctx.corpus.unigrams);
    let mut inter_bufs = InterLayerBufs::new(current.kp.num_chars);
    let mut delta_buf = DeltaScoreBuffer::new(ctx.corpus.bigrams.len(), ctx.corpus.trigrams.len());
    let mut pair_cache = DeltaPairCache::new();

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
            &mut pair_cache,
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
            &mut pair_cache,
        );

        generate_inter_layer_candidates(
            &current,
            ctx,
            config.inter_sample,
            rng,
            &mut candidates,
            &mut inter_bufs,
            &mut delta_buf,
            &mut pair_cache,
        );

        // 拗音面内スワップ（hybrid のみ）
        if current.kp.yoon {
            generate_yoon_candidates(
                &current,
                ctx,
                &yoon_chars,
                config.ab_sample_limit,
                rng,
                &mut candidates,
                &mut delta_buf,
                &mut pair_cache,
            );
        }

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
            let is_tabu = tabu.contains(cand.kind, cand.c1, cand.c2);
            if !is_tabu {
                if best_free.is_none_or(|f| cand.delta < f.delta) {
                    best_free = Some(cand);
                }
            } else if cand.delta < aspiration_threshold
                && best_aspiration.is_none_or(|a| cand.delta < a.delta)
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

        let dirty = compute_dirty_mask(ctx.corpus, chosen.c1, chosen.c2);
        pair_cache.invalidate_dirty(dirty, n_active_chars);

        tabu.add(chosen.kind, chosen.c1, chosen.c2);

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
                unigrams: Arc::clone(&unigrams_shared),
                phase: SearchPhase::Running,
            });
            tabu.reset();
        } else {
            no_improve += 1;
            if config.tenure_grow_interval > 0
                && no_improve > tenure_grow_start
                && (no_improve - tenure_grow_start).is_multiple_of(config.tenure_grow_interval)
            {
                tabu.grow();
            }
        }

        if config.log_interval > 0 && iter.is_multiple_of(config.log_interval) {
            let _ = writeln!(out,
                "iter {:>6} | current {:.4} | best {:.4} | no_improve {:>5} | tenure {}{}",
                iter, current_score, best_score, no_improve,
                tabu.tenure_summary(current.kp.yoon),
                if restarts > 0 { format!(" (restart {})", restarts) } else { String::new() }
            );
            on_update(&SearchUpdate {
                iter,
                restarts,
                current_score,
                best_score,
                best_layout: best.clone(),
                unigrams: Arc::clone(&unigrams_shared),
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
            pair_cache.invalidate_all();

            tabu.reset_and_clear();

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
                unigrams: Arc::clone(&unigrams_shared),
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
        unigrams: Arc::clone(&unigrams_shared),
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

/// 操作D: 拗音面内スワップ候補を生成（hybrid のみ）
///
/// 少なくとも一方の端点を子音に限定する。void↔void のスワップは
/// デルタが常に 0 の無操作で、候補枠とタブー枠を浪費するため除外する。
#[allow(clippy::too_many_arguments)]
fn generate_yoon_candidates(
    layout: &Layout,
    ctx: &SearchContext,
    yoon_chars: &[CharId],
    sample_limit: usize,
    rng: &mut impl Rng,
    out: &mut Vec<Candidate>,
    buf: &mut DeltaScoreBuffer,
    cache: &mut DeltaPairCache,
) {
    let k = layout.kp.num_consonants as usize;
    let n = yoon_chars.len();
    if k == 0 || n < 2 {
        return;
    }
    // 子音を含むペア数 = 子音同士 + 子音×void
    let max_pairs = k * (k - 1) / 2 + k * (n - k);

    // 除外（無操作ペア・制約違反）のときは false を返す。呼び出し側はこれを見て
    // sample_limit の消費対象から外す（除外分もカウントすると、無操作ペアが多い
    // ときに実候補が sample_limit より大幅に少ないまま探索が打ち切られてしまう）。
    let push = |c1: CharId,
                c2: CharId,
                out: &mut Vec<Candidate>,
                buf: &mut DeltaScoreBuffer,
                cache: &mut DeltaPairCache|
     -> bool {
        if skip_inert_pair(layout, ctx.corpus, c1, c2) || swap_would_violate(layout, c1, c2, ctx.pairs) {
            return false;
        }
        let delta = cache.get_or_compute(c1, c2, layout, ctx.corpus, ctx.weights, buf);
        out.push(Candidate { kind: OpKind::SwapYoon, c1, c2, delta });
        true
    };

    if max_pairs <= sample_limit {
        // 全列挙: i は子音のみ、j は i より後ろの全拗音面文字
        for i in 0..k {
            for j in (i + 1)..n {
                push(yoon_chars[i], yoon_chars[j], out, buf, cache);
            }
        }
    } else {
        let mut sampled = 0;
        let mut tries = 0;
        while sampled < sample_limit && tries < sample_limit * 4 {
            tries += 1;
            let i = rng.gen_range(0..k); // 必ず子音
            let j = rng.gen_range(0..n);
            if i == j {
                continue;
            }
            if push(yoon_chars[i], yoon_chars[j], out, buf, cache) {
                sampled += 1;
            }
        }
    }
}

/// L1/L2 の void文字（空きスロット代替）かどうか
///
/// 拗音面の文字は含まない（`chars::is_l1l2_void` 参照）。
#[inline]
fn is_void(c: CharId) -> bool {
    is_l1l2_void(c)
}

/// 候補から除外すべき「スコアが必ず変化しない」ペアか（hybrid のみ）。
///
/// 両端ともコーパス出現頻度0の文字なら、どの n-gram にも現れない（n-gram はユニグラムと
/// 同じセグメントから集計されるため）ので、入れ替えてもスコアは数学的に必ず変化しない。
///
/// この「必ず delta == 0」の候補は、収束後には他の候補（すべて悪化＝正の delta）に
/// 常に勝つため、探索が無限のプラトーに捕まる。実測（同梱コーパス・5万反復）では
/// 採択の98%がこれに費やされ、拗音面の実改善は63回しか出せていなかった。除外すると
/// 2095回まで増え、8シード中6シードでスコアが改善する。
///
/// ただし none では逆に悪化する（8シード中3シードしか勝てない）。デルタ0の移動は
/// レイアウトを変えないままタブーリストだけを経過させる「待ち」として働いており、
/// none ではその多様化効果の方が勝るため。よって拗音面という競合する操作種別を
/// 持つ hybrid に限定して適用する。
#[inline]
fn skip_inert_pair(layout: &Layout, corpus: &Corpus, c1: CharId, c2: CharId) -> bool {
    layout.kp.yoon
        && corpus.unigrams[c1 as usize] == 0.0
        && corpus.unigrams[c2 as usize] == 0.0
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
    cache: &mut DeltaPairCache,
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
                if skip_inert_pair(layout, ctx.corpus, c1, c2)
                    || swap_would_violate(layout, c1, c2, ctx.pairs)
                {
                    continue;
                }
                let delta =
                    cache.get_or_compute(c1, c2, layout, ctx.corpus, ctx.weights, buf);
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
            if skip_inert_pair(layout, ctx.corpus, c1, c2)
                || swap_would_violate(layout, c1, c2, ctx.pairs)
            {
                continue;
            }
            let delta = cache.get_or_compute(c1, c2, layout, ctx.corpus, ctx.weights, buf);
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
struct InterLayerBufs {
    l1_chars: Vec<(CharId, f64)>,
    l2_chars: Vec<(CharId, f64)>,
    l1_weights: Vec<f64>,
    l2_weights: Vec<f64>,
}

impl InterLayerBufs {
    fn new(num_chars: usize) -> Self {
        Self {
            l1_chars: Vec::with_capacity(num_chars),
            l2_chars: Vec::with_capacity(num_chars),
            l1_weights: Vec::with_capacity(num_chars),
            l2_weights: Vec::with_capacity(num_chars),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_inter_layer_candidates(
    layout: &Layout,
    ctx: &SearchContext,
    n_samples: usize,
    rng: &mut impl Rng,
    out: &mut Vec<Candidate>,
    ibufs: &mut InterLayerBufs,
    buf: &mut DeltaScoreBuffer,
    cache: &mut DeltaPairCache,
) {
    let kp = layout.kp;

    ibufs.l1_chars.clear();
    ibufs.l1_chars.extend(
        (0..kp.num_chars as CharId)
            .filter(|&c| layout.is_l1(c) && is_inter_layer_movable(c, kp, ctx.l1_only) && !is_void(c))
            .map(|c| (c, ctx.corpus.unigrams[c as usize])),
    );
    ibufs.l1_chars.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));

    ibufs.l2_chars.clear();
    ibufs.l2_chars.extend(
        (0..kp.num_chars as CharId)
            .filter(|&c| !layout.is_l1(c) && !is_void(c))
            .map(|c| (c, ctx.corpus.unigrams[c as usize])),
    );
    ibufs.l2_chars.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));

    if ibufs.l1_chars.is_empty() || ibufs.l2_chars.is_empty() {
        return;
    }

    ibufs.l1_weights.clear();
    ibufs.l1_weights.extend((0..ibufs.l1_chars.len()).map(|r| 1.0 / (r + 1) as f64));
    ibufs.l2_weights.clear();
    ibufs.l2_weights.extend((0..ibufs.l2_chars.len()).map(|r| 1.0 / (r + 1) as f64));
    let l1_w_sum: f64 = ibufs.l1_weights.iter().sum();
    let l2_w_sum: f64 = ibufs.l2_weights.iter().sum();

    let mut sampled = 0;
    let mut tries = 0;
    while sampled < n_samples && tries < n_samples * 5 {
        tries += 1;
        let c1 = weighted_choice(&ibufs.l1_chars, &ibufs.l1_weights, l1_w_sum, rng).0;
        let c2 = weighted_choice(&ibufs.l2_chars, &ibufs.l2_weights, l2_w_sum, rng).0;
        if swap_would_violate(layout, c1, c2, ctx.pairs) {
            continue;
        }
        let delta = cache.get_or_compute(c1, c2, layout, ctx.corpus, ctx.weights, buf);
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

    // hybrid: 拗音面も撹乱する（少なくとも一方は子音。void↔void は無操作）
    let k = kp.num_consonants as usize;
    if kp.yoon && k > 0 {
        let yoon_chars: Vec<CharId> = kp.yoon_char_range().map(|c| c as CharId).collect();
        for _ in 0..n_swaps {
            let c1 = yoon_chars[rng.gen_range(0..k)];
            let c2 = *yoon_chars.choose(rng).unwrap();
            if c1 == c2 || swap_would_violate(layout, c1, c2, pairs) {
                continue;
            }
            layout.swap_chars(c1, c2);
        }
    }
}

/// ——————————————————————————————
/// 初期解生成
/// ——————————————————————————————
pub fn build_initial_layout(
    ctx: &SearchContext,
    kp: KeyboardParams,
    mode: InitialLayoutMode,
    rng: &mut impl Rng,
    out: &mut impl Write,
) -> Layout {
    // hybrid 未対応の初期配列モードはここで差し替える。
    // （どのモードが拗音面に対応しているかを dispatch の直前に集約し、
    //   下のログが「実際に使われたモード」を表示するようにする）
    let mode = if kp.yoon && mode == InitialLayoutMode::UserDefined {
        let _ = writeln!(
            out,
            "注意: user-defined 初期配列は hybrid 拗音面に未対応です → 2-263 を使用します"
        );
        InitialLayoutMode::Tsuki2_263
    } else {
        mode
    };

    let layout = match mode {
        InitialLayoutMode::Tsuki2_263 => build_initial_2_263(ctx, kp, out),
        InitialLayoutMode::Random => build_initial_random(ctx, kp, rng, out),
        InitialLayoutMode::UserDefined => build_initial_user_defined(ctx, kp, rng, out),
    };

    let _ = writeln!(out, "初期解生成完了（{}）。L1に配置: {:?}",
        mode.label(),
        {
            use crate::chars::CHAR_LIST;
            (0..kp.num_chars as CharId)
                .filter(|&c| layout.is_l1(c) && !is_void(c))
                .map(|c| CHAR_LIST[c as usize])
                .collect::<String>()
        },
    );

    layout
}

/// `l1_only` 指定の文字を Layer 1 へ引き上げる。
///
/// `l1_only` は「L1 から出さない」制約としてのみ実装されており（`is_inter_layer_movable`
/// が false を返す）、初期配置で L2 にいる文字を L1 へ入れる処理はどこにもなかった。
/// そのため初期レイヤーが L2 の文字（CharId 30..60）を `l1_only` に指定すると、
/// 昇格候補にも層間スワップ候補にもならず、L2 に固定されたまま制約が黙って破られていた。
///
/// ここで L1 の最低頻度の可動文字と交換して引き上げ、以降の
/// `is_inter_layer_movable` による凍結が「L1 に居続ける」意味になるようにする。
///
/// 交換相手が尽きた場合（L1 が固定文字で埋まっている等）は、引き上げられなかった
/// 文字を返す。呼び出し側で警告を出すために使う。
fn promote_l1_only_chars(
    layout: &mut Layout,
    ctx: &SearchContext,
    kp: KeyboardParams,
) -> Vec<CharId> {
    let unigrams = &ctx.corpus.unigrams;
    // 頻度の低い文字から引き上げると、交換相手（L1 の最低頻度文字）を先に消費して
    // しまうため、頻度の高い文字から順に処理する
    let mut pending: Vec<CharId> = (0..kp.num_chars as CharId)
        .filter(|&c| ctx.l1_only.contains(&c) && !is_void(c) && !layout.is_l1(c))
        .collect();
    pending.sort_unstable_by(|&a, &b| unigrams[b as usize].total_cmp(&unigrams[a as usize]));

    let mut failed = Vec::new();
    for c in pending {
        // 交換相手: L1 にいる可動文字のうち最低頻度のもの
        let target = (0..kp.num_chars as CharId)
            .filter(|&t| {
                layout.is_l1(t) && is_inter_layer_movable(t, kp, ctx.l1_only) && !is_void(t)
            })
            .min_by(|&a, &b| unigrams[a as usize].total_cmp(&unigrams[b as usize]));
        match target {
            Some(t) => layout.swap_chars(c, t),
            None => failed.push(c),
        }
    }
    failed
}

/// `promote_l1_only_chars` が引き上げられなかった文字を警告する。
fn warn_unpromoted(failed: Vec<CharId>, out: &mut impl Write) {
    if failed.is_empty() {
        return;
    }
    use crate::chars::CHAR_LIST;
    let names: String = failed.iter().map(|&c| CHAR_LIST[c as usize]).collect();
    let _ = writeln!(
        out,
        "警告: l1_only の文字 '{}' を Layer 1 へ引き上げられませんでした（L1に交換可能な文字がありません）。",
        names
    );
}

/// hybrid: 拗音面（第3層）を構築する。
///
/// 1. ゃゅょ を L1 へ移動する（L2 にある場合、最低頻度の可動L1基底文字と交換）。
/// 2. 拗音面の各物理位置に子音/void を配置する。子音は「シフトキー位置」「ゃゅょ物理位置」
///    以外の使用可能スロットへ、頻度降順 × 難易度昇順で決定的に割り当てる。残りは void。
///
/// mode=none では呼ばれない（呼び出し側で kp.yoon を確認する）。
fn setup_yoon_face(layout: &mut Layout, ctx: &SearchContext, kp: KeyboardParams) {
    let npl = kp.num_slots_per_layer as usize;
    let unigrams = &ctx.corpus.unigrams;

    // 1. ゃゅょ の L1 への引き上げは promote_l1_only_chars が担う
    //    （hybrid では YoonSetup::extend_l1_only が ゃゅょ を l1_only に入れている）。
    //    ここに来た時点で L1 にいることを前提に、以降で禁止スロットを判定する。

    // 2. 子音を配置できる拗音面 physical（禁止位置を除く）
    let mut avail: Vec<usize> = (0..npl)
        .filter(|&p| !yoon_physical_forbidden(layout, p as SlotId))
        .collect();

    // 子音を頻度降順に
    let k = kp.num_consonants as usize;
    let mut cons: Vec<CharId> = (0..k as CharId).map(|i| CONSONANT_FIRST + i).collect();
    cons.sort_by(|&a, &b| unigrams[b as usize].total_cmp(&unigrams[a as usize]));

    // 子音は方式の定義上ちょうど100%が ゃ/ゅ/ょ に続かれるため、スロット難易度だけで
    // 決めるとバイグラム（子音→拗音シフト）を取りこぼす。拗音面ではこの遷移コストの
    // 方が難易度より支配的なので、両方を頻度で重み付けした合計コストで貪欲に割り当てる。
    let shift_slots: Vec<(CharId, SlotId)> = crate::chars::YOON_SHIFT_IDS
        .iter()
        .map(|&sid| (sid, layout.char_to_slot[sid as usize]))
        .collect();
    let placement_cost = |c: CharId, p: usize| -> f64 {
        let freq = unigrams[c as usize];
        let mut cost = freq * unigram_cost_for_slot(p as SlotId, ctx.weights);
        for &(sid, s_slot) in &shift_slots {
            let bf = crate::cost::lookup_bigram_freq(ctx.corpus, c, sid);
            if bf > 0.0 {
                cost += bf * crate::cost::key_pair_cost(p as SlotId, s_slot, ctx.weights);
            }
        }
        cost
    };

    // 頻度の高い子音から、残っているスロットのうち合計コスト最小の位置へ
    for &c in &cons {
        let Some((idx, _)) = avail
            .iter()
            .enumerate()
            .map(|(i, &p)| (i, placement_cost(c, p)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
        else {
            break;
        };
        let p = avail.swap_remove(idx);
        let slot = (2 * npl + p) as SlotId;
        layout.char_to_slot[c as usize] = slot;
        layout.slot_to_char[slot as usize] = c;
    }

    // 残りの拗音面スロットに void を割り当てる
    let mut void_id = CONSONANT_FIRST + kp.num_consonants;
    for p in 0..npl {
        let slot = 2 * npl + p;
        if !kp.is_consonant(layout.slot_to_char[slot]) {
            layout.char_to_slot[void_id as usize] = slot as SlotId;
            layout.slot_to_char[slot] = void_id;
            void_id += 1;
        }
    }
}

/// 頻度上位の文字をLayer 1へ配置（従来方式）
fn build_initial_2_263(
    ctx: &SearchContext,
    kp: KeyboardParams,
    out: &mut impl Write,
) -> Layout {
    let mut layout = Layout::initial(kp);
    // l1_only の文字を先に L1 へ引き上げる。拗音面の禁止スロット判定は
    // ゃゅょ が L1 にいることを前提にするため、setup_yoon_face より前に行う。
    warn_unpromoted(promote_l1_only_chars(&mut layout, ctx, kp), out);
    if kp.yoon {
        setup_yoon_face(&mut layout, ctx, kp);
    }

    let l1_char_slots = kp.num_slots_per_layer as usize
        - if kp.size == crate::layout::KeyboardSize::K3x11 {
            2
        } else {
            0
        };

    // 引き上げ後の実際の配置を数える。引き上げに失敗した文字を「L1を占める」と
    // 数えると L1 の空き枠を過少に見積もり、昇格すべき高頻度文字を弾いてしまう。
    let l1_fixed_count = (0..kp.num_chars as CharId)
        .filter(|&c| {
            !is_void(c) && (is_fixed(c, kp) || ctx.l1_only.contains(&c)) && layout.is_l1(c)
        })
        .count();
    if l1_fixed_count > l1_char_slots {
        let _ = writeln!(
            out,
            "警告: L1固定文字数({})がL1スロット数({})を超えています。可動L1枠は0として扱います。",
            l1_fixed_count, l1_char_slots
        );
    }
    let l1_free_slots = l1_char_slots.saturating_sub(l1_fixed_count);

    let mut movable: Vec<(CharId, f64)> = (0..kp.num_chars as CharId)
        .filter(|&c| !is_fixed(c, kp) && !ctx.l1_only.contains(&c) && !is_void(c))
        .map(|c| (c, ctx.corpus.unigrams[c as usize]))
        .collect();
    movable.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));

    let l1_targets: Vec<CharId> = movable
        .iter()
        .take(l1_free_slots)
        .map(|&(c, _)| c)
        .collect();

    let l1_target_set: std::collections::HashSet<CharId> = l1_targets.iter().copied().collect();

    let mut to_demote: std::collections::VecDeque<CharId> = (0..kp.num_chars as CharId)
        .filter(|&c| {
            layout.is_l1(c)
                && !is_fixed(c, kp)
                && !ctx.l1_only.contains(&c)
                && !is_void(c)
                && !l1_target_set.contains(&c)
        })
        .collect();

    let mut to_promote: std::collections::VecDeque<CharId> = l1_targets
        .iter()
        .copied()
        .filter(|&c| !layout.is_l1(c))
        .collect();

    while let (Some(demote), Some(promote)) = (to_demote.pop_front(), to_promote.pop_front()) {
        layout.swap_chars(demote, promote);
    }

    fix_exclusive_pair_violations(&mut layout, ctx, kp, out);
    layout
}

/// initial_layout.toml からユーザー定義配列を読み込む。
/// ファイルが存在しない・パース失敗・定義不正の場合はログに警告を記録し
/// ランダム配字にフォールバックする。
fn build_initial_user_defined(
    ctx: &SearchContext,
    kp: KeyboardParams,
    rng: &mut impl Rng,
    out: &mut impl Write,
) -> Layout {
    use crate::user_layout::{parse_user_layout, UserLayoutFile, USER_LAYOUT_PATH};
    use std::path::Path;


    let path = Path::new(USER_LAYOUT_PATH);

    let user_file = match UserLayoutFile::from_file(path) {
        Ok(f) => f,
        Err(e) => {
            let _ = writeln!(
                out,
                "警告: ユーザー定義配列を使用できません（{}）→ ランダム配字にフォールバック",
                e
            );
            return build_initial_random(ctx, kp, rng, out);
        }
    };

    let def = match user_file.get_def(kp) {
        Some(d) => d,
        None => {
            let _ = writeln!(
                out,
                "警告: initial_layout.toml に [layout_{}] セクションがありません → ランダム配字にフォールバック",
                crate::config::keyboard_size_str(&kp)
            );
            return build_initial_random(ctx, kp, rng, out);
        }
    };

    match parse_user_layout(kp, def) {
        Ok(mut layout) => {
            let violates_layer_constraints = (0..kp.num_chars as CharId).any(|c| {
                (is_fixed(c, kp) || ctx.l1_only.contains(&c)) && !layout.is_l1(c)
            });
            if violates_layer_constraints {
                let _ = writeln!(
                    out,
                    "警告: ユーザー定義配列が固定/L1固定制約に違反しています → ランダム配字にフォールバック"
                );
                return build_initial_random(ctx, kp, rng, out);
            }
            fix_exclusive_pair_violations(&mut layout, ctx, kp, out);
            layout
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "警告: ユーザー定義配列の解析に失敗しました（{}）→ ランダム配字にフォールバック",
                e
            );
            build_initial_random(ctx, kp, rng, out)
        }
    }
}

/// ランダム配字：制約を守りつつ全文字をランダムにシャッフル
fn build_initial_random(
    ctx: &SearchContext,
    kp: KeyboardParams,
    rng: &mut impl Rng,
    out: &mut impl Write,
) -> Layout {
    let mut layout = Layout::initial(kp);
    warn_unpromoted(promote_l1_only_chars(&mut layout, ctx, kp), out);
    if kp.yoon {
        setup_yoon_face(&mut layout, ctx, kp);
    }

    let movable: Vec<CharId> = (0..kp.num_chars as CharId)
        .filter(|&c| !is_fixed(c, kp) && !ctx.l1_only.contains(&c) && !is_void(c))
        .collect();

    // シャッフル前のスロットを記録
    let mut slots: Vec<crate::layout::SlotId> = movable
        .iter()
        .map(|&c| layout.char_to_slot[c as usize])
        .collect();

    // スロット列をシャッフルし、文字に再割り当て
    slots.shuffle(rng);

    for &s in &slots {
        layout.slot_to_char[s as usize] = SHIFT_SLOT_SENTINEL;
    }
    for (&c, &s) in movable.iter().zip(slots.iter()) {
        layout.char_to_slot[c as usize] = s;
        layout.slot_to_char[s as usize] = c;
    }

    fix_exclusive_pair_violations(&mut layout, ctx, kp, out);
    layout
}

/// 排他ペア制約の初期違反を greedy 修正（L2 同士をスワップして解消）
fn fix_exclusive_pair_violations(
    layout: &mut Layout,
    ctx: &SearchContext,
    kp: KeyboardParams,
    out: &mut impl Write,
) {
    if ctx.pairs.is_empty() {
        return;
    }
    let npl = kp.num_slots_per_layer as usize;
    for _pass in 0..20 {
        let mut any_violation = false;
        for l1_slot in 0..npl {
            let l2_slot = l1_slot + npl;
            let l1_c = layout.slot_to_char[l1_slot];
            let l2_c = layout.slot_to_char[l2_slot];
            if !is_base_kana(l1_c) || !is_base_kana(l2_c) {
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
                if !is_base_kana(alt_l2_c) || alt_l2_c == l2_c {
                    continue;
                }
                if ctx.pairs.iter().any(|p| p.violates(l1_c, alt_l2_c)) {
                    continue;
                }
                let alt_l1_c = layout.slot_to_char[alt_l1_slot];
                if is_base_kana(alt_l1_c)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chars::{YA_ID, YO_ID, YU_ID};
    use crate::yoon::{YoonTable, DEFAULT_CONSONANTS};

    const YOON_CORPUS: &str = "\
        きゃきゅきょしゃしゅしょちゃちゅちょにゃにゅにょひゃひゅひょ\
        みゃみゅみょりゃりゅりょぎゃぎゅぎょじゃじゅじょびゃびゅびょ\
        ぴゃぴゅぴょきょうしょうじょうりょうびょうぎょうにゅうす\
        しているのはたかいてにをとなっくれるさきこそうんおもちよけ";

    fn hybrid_ctx_fixtures(
        kp: KeyboardParams,
    ) -> (Corpus, Weights, HashSet<CharId>, Vec<ExclusivePair>) {
        let table = YoonTable::from_spec(DEFAULT_CONSONANTS).unwrap();
        let corpus = Corpus::from_str_with_yoon(YOON_CORPUS, Some(&table));
        let weights = Weights {
            kp,
            ..Default::default()
        };
        let mut l1_only: HashSet<CharId> = HashSet::new();
        for c in [
            crate::chars::DAKUTEN_ID,
            crate::chars::HANDAKUTEN_ID,
            YA_ID,
            YU_ID,
            YO_ID,
        ] {
            l1_only.insert(c);
        }
        (corpus, weights, l1_only, Vec::new())
    }

    /// レイアウトが有効か（全単射 + 拗音制約）を検証する
    fn assert_valid_hybrid(layout: &Layout) {
        let kp = layout.kp;
        let npl = kp.num_slots_per_layer as usize;

        // 全文字（基底非void + 拗音面）が全単射
        let charset: Vec<CharId> = (0..kp.num_chars as CharId)
            .filter(|&c| is_base_kana(c))
            .chain(kp.yoon_char_range().map(|c| c as CharId))
            .collect();
        for &c in &charset {
            let s = layout.char_to_slot[c as usize];
            assert_eq!(
                layout.slot_to_char[s as usize], c,
                "char {c} at slot {s} not bijective"
            );
        }

        // ゃゅょ は L1
        for yc in [YA_ID, YU_ID, YO_ID] {
            assert!(layout.is_l1(yc), "ゃゅょ ({yc}) must be in L1");
        }

        // 子音は禁止物理位置（シフト/ゃゅょ）に置かれない
        for c in 0..kp.num_consonants as CharId {
            let cons = CONSONANT_FIRST + c;
            let slot = layout.char_to_slot[cons as usize] as usize;
            assert!(slot >= 2 * npl, "consonant {cons} must be in yoon layer");
            let p = slot - 2 * npl;
            assert!(
                p != kp.shift_left as usize && p != kp.shift_right as usize,
                "consonant {cons} on shift physical {p}"
            );
            assert!(
                !crate::chars::is_yoon_shift_id(layout.slot_to_char[p]),
                "consonant {cons} shares physical {p} with ゃゅょ"
            );
        }
    }

    fn run_hybrid_checks(base: KeyboardParams) {
        let kp = base.with_yoon(((1u32 << 11) - 1) as u16).unwrap();
        let (corpus, weights, l1_only, pairs) = hybrid_ctx_fixtures(kp);
        let ctx = SearchContext {
            corpus: &corpus,
            weights: &weights,
            pairs: &pairs,
            l1_only: &l1_only,
        };
        let mut sink = Vec::new();
        let layout = build_initial_2_263(&ctx, kp, &mut sink);
        assert_valid_hybrid(&layout);
        crate::cost::tests::verify_all_pairs(&layout, &corpus, &weights);
    }

    #[test]
    fn test_hybrid_delta_all_pairs_3x10() {
        run_hybrid_checks(KeyboardParams::k3x10());
    }

    #[test]
    fn test_hybrid_delta_all_pairs_single_shift() {
        run_hybrid_checks(KeyboardParams::k3x10_single_shift());
    }

    #[test]
    fn test_hybrid_delta_all_pairs_3x11() {
        run_hybrid_checks(KeyboardParams::k3x11());
    }

    #[test]
    fn test_skip_inert_pair_gated_and_correct() {
        let table = YoonTable::from_spec(DEFAULT_CONSONANTS).unwrap();
        let corpus = Corpus::from_str_with_yoon(YOON_CORPUS, Some(&table));

        // 頻度0どうし（拗音面の void 同士）は hybrid で除外される
        let kp = KeyboardParams::k3x10()
            .with_yoon(table.registry_mask())
            .unwrap();
        let layout = Layout::initial(kp);
        let v1 = CONSONANT_FIRST + kp.num_consonants;
        let v2 = v1 + 1;
        assert_eq!(corpus.unigrams[v1 as usize], 0.0);
        assert!(skip_inert_pair(&layout, &corpus, v1, v2));

        // 片方でも頻度があれば除外しない
        let used = (0..kp.num_chars as CharId)
            .find(|&c| corpus.unigrams[c as usize] > 0.0)
            .expect("出現する文字が1つはある");
        assert!(!skip_inert_pair(&layout, &corpus, v1, used));

        // none では常に false（デフォルト経路の挙動を変えない）
        let plain = Layout::initial(KeyboardParams::k3x10());
        assert!(!skip_inert_pair(&plain, &corpus, v1, v2));
    }

    #[test]
    fn test_l1_only_chars_are_promoted_to_l1() {
        // l1_only は「L1 から出さない」制約でしかなく、初期配置が L2 の文字を
        // L1 へ入れる処理が無かったため、指定しても黙って L2 に残っていた。
        use crate::chars::build_char_to_id;
        let map = build_char_to_id();
        let corpus = Corpus::from_str(YOON_CORPUS);
        let kp = KeyboardParams::k3x10();
        let weights = Weights { kp, ..Default::default() };

        // 'ー'(CharId 58) と 'を'(41) は初期配置が L2
        let l2_start = [map[&'ー'], map[&'を']];
        let plain = Layout::initial(kp);
        for &c in &l2_start {
            assert!(!plain.is_l1(c), "前提: CharId {c} は初期 L2");
        }

        let mut l1_only: HashSet<CharId> = HashSet::new();
        l1_only.insert(crate::chars::DAKUTEN_ID);
        l1_only.extend(l2_start);
        let pairs: Vec<ExclusivePair> = Vec::new();
        let ctx = SearchContext {
            corpus: &corpus,
            weights: &weights,
            pairs: &pairs,
            l1_only: &l1_only,
        };

        for mode in [InitialLayoutMode::Tsuki2_263, InitialLayoutMode::Random] {
            let mut rng = SmallRng::seed_from_u64(1);
            let mut sink = Vec::new();
            let layout = build_initial_layout(&ctx, kp, mode, &mut rng, &mut sink);
            for &c in &l1_only {
                assert!(
                    layout.is_l1(c),
                    "{mode:?}: l1_only の CharId {c} が L1 にいない"
                );
            }
        }
    }

    #[test]
    fn test_hybrid_swap_constraint() {
        // 子音がある拗音物理位置へ ゃ を L1 スワップできない
        let kp = KeyboardParams::k3x10().with_yoon(((1u32 << 11) - 1) as u16).unwrap();
        let (corpus, weights, l1_only, pairs) = hybrid_ctx_fixtures(kp);
        let ctx = SearchContext {
            corpus: &corpus,
            weights: &weights,
            pairs: &pairs,
            l1_only: &l1_only,
        };
        let mut sink = Vec::new();
        let layout = build_initial_2_263(&ctx, kp, &mut sink);
        let npl = kp.num_slots_per_layer as usize;

        // 子音が乗っている物理位置を探し、その物理位置の L1 文字と ゃ の交換が違反すること
        let cons0 = CONSONANT_FIRST;
        let p = layout.char_to_slot[cons0 as usize] as usize - 2 * npl;
        let l1_char_at_p = layout.slot_to_char[p];
        // ゃ をその L1 位置へ動かす swap は違反する（子音がいるため）
        assert!(
            swap_would_violate(&layout, YA_ID, l1_char_at_p, &pairs),
            "ゃ moving onto a consonant's physical key must violate"
        );
    }
}

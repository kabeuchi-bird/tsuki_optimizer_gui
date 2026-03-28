// cost.rs — 評価関数とコスト定数

use std::io::Write;

use crate::chars::{CharId, DAKUTEN_ID, TOUTEN_ID, KUTEN_ID, MAX_CHARS};
use crate::corpus::Corpus;
use crate::layout::{
    Layout, SlotId, Hand,
    slot_col, slot_row, slot_hand, col_to_finger,
    keystrokes_for_slot, slot_after_swap,
    KeyboardParams,
};

/// ——————————————————————————————
/// スコアリングの重みパラメータ
/// ——————————————————————————————
#[derive(Clone)]
pub struct Weights {
    /// キーボード形状パラメータ（scoring 側が参照する）
    pub kp: KeyboardParams,

    /// 打鍵数スケール（優先度最大）
    pub stroke_scale: f64,

    /// スロット難易度テーブル [row 0..2][col 0..10]
    /// 3x10 は col 0-9 のみ使用、3x11 は col 0-10 まで使用
    pub slot_difficulty: [[f64; 11]; 3],

    /// 同指連打ペナルティ（同じ指・異なるキー）
    pub same_finger_penalty: f64,
    /// 同キー連打ペナルティ（同じ物理キーを連続。L1同士は同一文字、L2→L1は異なる文字でも発生）
    pub same_key_penalty: f64,
    /// 同手・上段⟺下段 段跨ぎペナルティ（row差2）
    pub upper_lower_jump: f64,
    /// 同手・異指の基礎コスト
    pub same_hand_base: f64,

    /// 左右交互打鍵ボーナス（差し引く値）
    pub alternation_bonus: f64,
    /// アウトロール（小指方向）ボーナス
    pub outroll_bonus: f64,
    /// インロール（人差し指方向）ボーナス
    pub inroll_bonus: f64,

    /// 準交互打鍵（LLR/RRL等）ボーナス（trigram単位）
    pub quasi_alt_bonus: f64,

    /// プリセット有効時: この文字がL2に配置されているとき、直後の゛コストを -stroke_scale 削減する
    /// （デフォルトはすべて false = 削減なし）
    pub daku_l2_trigger: [bool; MAX_CHARS],
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            kp: KeyboardParams::k3x10(),
            stroke_scale: 10.0,
            // 難易度テーブル（3行×11列）
            // col 10 は 3x11 専用（右小指の追加列）
            slot_difficulty: [
                // row 0（上段）
                [1.8, 1.4, 1.2, 1.1, 1.4,  1.4, 1.1, 1.2, 1.4, 1.8, 2.0],
                // row 1（中段ホーム）
                [1.3, 1.0, 0.9, 0.9, 1.1,  1.1, 0.9, 0.9, 1.0, 1.3, 1.6],
                // row 2（下段）
                [1.9, 1.5, 1.3, 1.2, 1.6,  1.6, 1.2, 1.3, 1.5, 1.9, 2.2],
            ],
            same_finger_penalty: 5.0,
            same_key_penalty:    8.0,
            upper_lower_jump:    1.5,
            same_hand_base:      0.2,
            alternation_bonus:   0.6,
            outroll_bonus:       0.4,
            inroll_bonus:        0.15,
            quasi_alt_bonus:     0.1,
            daku_l2_trigger:     [false; MAX_CHARS],
        }
    }
}

/// ——————————————————————————————
/// 2キー間のトランジションコスト
/// ——————————————————————————————
#[inline]
pub fn key_pair_cost(k1: SlotId, k2: SlotId, w: &Weights) -> f64 {
    let nc = w.kp.num_cols;
    if k1 == k2 {
        return w.same_key_penalty;
    }
    let f1 = col_to_finger(slot_col(k1, nc));
    let f2 = col_to_finger(slot_col(k2, nc));
    if f1 == f2 {
        return w.same_finger_penalty;
    }
    let h1 = slot_hand(k1, nc);
    let h2 = slot_hand(k2, nc);
    if h1 != h2 {
        return -w.alternation_bonus;
    }
    // 同手・異指
    let mut cost = w.same_hand_base;
    let r1 = slot_row(k1, nc);
    let r2 = slot_row(k2, nc);
    if (r1 as i8 - r2 as i8).abs() == 2 {
        cost += w.upper_lower_jump;
    }
    let c1 = slot_col(k1, nc);
    let c2 = slot_col(k2, nc);
    let is_outroll = match h1 {
        Hand::Left  => c2 < c1,
        Hand::Right => c2 > c1,
    };
    cost -= if is_outroll { w.outroll_bonus } else { w.inroll_bonus };
    cost
}

/// ——————————————————————————————
/// ユニグラムコスト（単独打鍵の難易度 + L2文字の文字内トランジション）
/// ——————————————————————————————
#[inline]
pub fn unigram_cost_for_slot(slot: SlotId, w: &Weights) -> f64 {
    let nc = w.kp.num_cols;
    let ks = keystrokes_for_slot(slot, w.kp);
    let keys = ks.as_slice();
    let diff: f64 = keys.iter()
        .map(|&s| w.slot_difficulty[slot_row(s, nc) as usize][slot_col(s, nc) as usize])
        .sum();
    let intra = if keys.len() == 2 {
        key_pair_cost(keys[0], keys[1], w)
    } else {
        0.0
    };
    diff + intra
}

/// ——————————————————————————————
/// バイグラムの文字間トランジションコスト
///
/// 。/、 が c1 のとき：Enterで文脈リセット → 0
/// （3x10/3x11 共通。。/、は文として区切りになるため）
/// ——————————————————————————————
#[inline]
pub fn bigram_inter_cost(c1: CharId, c2: CharId, slot1: SlotId, slot2: SlotId, w: &Weights) -> f64 {
    if c1 == KUTEN_ID || c1 == TOUTEN_ID {
        return 0.0;
    }
    let ks1 = keystrokes_for_slot(slot1, w.kp);
    let ks2 = keystrokes_for_slot(slot2, w.kp);
    let mut cost = key_pair_cost(ks1.last(), ks2.first(), w);
    // L2配置の濁音基音 → ゛ のとき、シフトキー1打鍵分（打鍵数コスト + シフトキー難度）を削減
    // （シフトキー→対象文字→゛ 打鍵列においてシフトキーを不要と見なす）
    // ks1 = [shift_slot, physical_slot]（L2の場合）なので ks1.first() = シフトキースロット
    // delta_score() は slot_after_swap() で仮スロットを渡すため追加実装不要
    if c2 == DAKUTEN_ID
        && w.daku_l2_trigger[c1 as usize]
        && (slot1 as usize) >= w.kp.num_slots_per_layer as usize
    {
        let nc = w.kp.num_cols;
        let shift_slot = ks1.first();
        let shift_diff = w.slot_difficulty[slot_row(shift_slot, nc) as usize]
                                          [slot_col(shift_slot, nc) as usize];
        cost -= w.stroke_scale + shift_diff;
    }
    cost
}

/// ——————————————————————————————
/// 準交互打鍵ボーナス（trigram単位）
/// ——————————————————————————————
#[inline]
pub fn quasi_alt_bonus(h1: Hand, h2: Hand, h3: Hand, w: &Weights) -> f64 {
    if (h1 == h2) != (h2 == h3) {
        -w.quasi_alt_bonus
    } else {
        0.0
    }
}

/// ——————————————————————————————
/// 総合スコア（全コーパスに対して計算、値が小さいほど良い）
/// ——————————————————————————————
pub fn score(layout: &Layout, corpus: &Corpus, w: &Weights) -> f64 {
    let nc = w.kp.num_chars;
    let mut total = 0.0;

    // 1. 打鍵数コスト（最優先）
    for c in 0..nc as CharId {
        let freq = corpus.unigrams[c as usize];
        if freq == 0.0 { continue; }
        total += freq * layout.char_stroke_count(c) as f64 * w.stroke_scale;
    }

    // 2. ユニグラム難易度（基礎コスト + 文字内トランジション）
    for c in 0..nc as CharId {
        let freq = corpus.unigrams[c as usize];
        if freq == 0.0 { continue; }
        let slot = layout.char_to_slot[c as usize];
        total += freq * unigram_cost_for_slot(slot, w);
    }

    // 3. バイグラム文字間トランジション
    for bg in &corpus.bigrams {
        if bg.freq == 0.0 { continue; }
        let s1 = layout.char_to_slot[bg.c1 as usize];
        let s2 = layout.char_to_slot[bg.c2 as usize];
        total += bg.freq * bigram_inter_cost(bg.c1, bg.c2, s1, s2, w);
    }

    // 4. トライグラム準交互ボーナス
    for tg in &corpus.trigrams {
        if tg.freq == 0.0 { continue; }
        let h1 = layout.primary_hand(tg.c1);
        let h2 = layout.primary_hand(tg.c2);
        let h3 = layout.primary_hand(tg.c3);
        total += tg.freq * quasi_alt_bonus(h1, h2, h3, w);
    }

    total
}

/// ——————————————————————————————
/// デルタスコア（スワップ swap_c1 ⟺ swap_c2 によるスコア変化量）
/// ——————————————————————————————
pub fn delta_score(
    layout: &Layout,
    corpus: &Corpus,
    w: &Weights,
    swap_c1: CharId,
    swap_c2: CharId,
) -> f64 {
    let mut delta = 0.0;

    let s1_old = layout.char_to_slot[swap_c1 as usize];
    let s2_old = layout.char_to_slot[swap_c2 as usize];
    let s1_new = s2_old;
    let s2_new = s1_old;

    // 打鍵数コスト差分
    let strokes_old_c1 = stroke_count_for_slot(swap_c1, s1_old, w.kp);
    let strokes_new_c1 = stroke_count_for_slot(swap_c1, s1_new, w.kp);
    let strokes_old_c2 = stroke_count_for_slot(swap_c2, s2_old, w.kp);
    let strokes_new_c2 = stroke_count_for_slot(swap_c2, s2_new, w.kp);
    delta += (strokes_new_c1 - strokes_old_c1) as f64
           * corpus.unigrams[swap_c1 as usize]
           * w.stroke_scale;
    delta += (strokes_new_c2 - strokes_old_c2) as f64
           * corpus.unigrams[swap_c2 as usize]
           * w.stroke_scale;

    // ユニグラム難易度差分
    delta += corpus.unigrams[swap_c1 as usize]
           * (unigram_cost_for_slot(s1_new, w) - unigram_cost_for_slot(s1_old, w));
    delta += corpus.unigrams[swap_c2 as usize]
           * (unigram_cost_for_slot(s2_new, w) - unigram_cost_for_slot(s2_old, w));

    // バイグラム差分
    let mut visited = vec![false; corpus.bigrams.len()];

    for &c in &[swap_c1, swap_c2] {
        for &idx in &corpus.bigram_adj[c as usize] {
            if visited[idx] { continue; }
            visited[idx] = true;

            let bg = &corpus.bigrams[idx];
            if bg.freq == 0.0 { continue; }

            let s_c1_old = layout.char_to_slot[bg.c1 as usize];
            let s_c2_old = layout.char_to_slot[bg.c2 as usize];
            let old_cost = bigram_inter_cost(bg.c1, bg.c2, s_c1_old, s_c2_old, w);

            let s_c1_new = slot_after_swap(layout, swap_c1, swap_c2, bg.c1);
            let s_c2_new = slot_after_swap(layout, swap_c1, swap_c2, bg.c2);
            let new_cost = bigram_inter_cost(bg.c1, bg.c2, s_c1_new, s_c2_new, w);

            delta += bg.freq * (new_cost - old_cost);
        }
    }

    // トライグラム準交互差分
    let mut tri_visited = vec![false; corpus.trigrams.len()];

    for &c in &[swap_c1, swap_c2] {
        for &idx in &corpus.trigram_adj[c as usize] {
            if tri_visited[idx] { continue; }
            tri_visited[idx] = true;

            let tg = &corpus.trigrams[idx];
            if tg.freq == 0.0 { continue; }

            let h1_old = slot_hand(layout.char_to_slot[tg.c1 as usize], w.kp.num_cols);
            let h2_old = slot_hand(layout.char_to_slot[tg.c2 as usize], w.kp.num_cols);
            let h3_old = slot_hand(layout.char_to_slot[tg.c3 as usize], w.kp.num_cols);
            let old_bonus = quasi_alt_bonus(h1_old, h2_old, h3_old, w);

            let h1_new = slot_hand(slot_after_swap(layout, swap_c1, swap_c2, tg.c1), w.kp.num_cols);
            let h2_new = slot_hand(slot_after_swap(layout, swap_c1, swap_c2, tg.c2), w.kp.num_cols);
            let h3_new = slot_hand(slot_after_swap(layout, swap_c1, swap_c2, tg.c3), w.kp.num_cols);
            let new_bonus = quasi_alt_bonus(h1_new, h2_new, h3_new, w);

            delta += tg.freq * (new_bonus - old_bonus);
        }
    }

    delta
}

/// 打鍵数計算（スロットと文字種から）
#[inline]
fn stroke_count_for_slot(c: CharId, slot: SlotId, kp: KeyboardParams) -> i32 {
    let is_3x10_punct = kp.size == crate::layout::KeyboardSize::K3x10
        && (c == KUTEN_ID || c == TOUTEN_ID);
    if is_3x10_punct {
        2  // K/D + Enter
    } else if (slot as usize) < kp.num_slots_per_layer as usize {
        1
    } else {
        2
    }
}

/// スコアの内訳を表示
pub fn score_breakdown(layout: &Layout, corpus: &Corpus, w: &Weights, out: &mut impl Write) {
    let nc = w.kp.num_chars;
    let mut stroke_cost = 0.0;
    let mut uni_cost = 0.0;
    let mut bi_cost = 0.0;
    let mut tri_cost = 0.0;
    let mut total_strokes = 0.0;
    let mut l1_coverage = 0.0;

    for c in 0..nc as CharId {
        let freq = corpus.unigrams[c as usize];
        if freq == 0.0 { continue; }
        let strokes = layout.char_stroke_count(c);
        stroke_cost += freq * strokes as f64 * w.stroke_scale;
        total_strokes += freq * strokes as f64;
        let slot = layout.char_to_slot[c as usize];
        uni_cost += freq * unigram_cost_for_slot(slot, w);
        if strokes == 1 { l1_coverage += freq; }
    }
    for bg in &corpus.bigrams {
        if bg.freq == 0.0 { continue; }
        let s1 = layout.char_to_slot[bg.c1 as usize];
        let s2 = layout.char_to_slot[bg.c2 as usize];
        bi_cost += bg.freq * bigram_inter_cost(bg.c1, bg.c2, s1, s2, w);
    }
    for tg in &corpus.trigrams {
        if tg.freq == 0.0 { continue; }
        let h1 = layout.primary_hand(tg.c1);
        let h2 = layout.primary_hand(tg.c2);
        let h3 = layout.primary_hand(tg.c3);
        tri_cost += tg.freq * quasi_alt_bonus(h1, h2, h3, w);
    }

    let total = stroke_cost + uni_cost + bi_cost + tri_cost;
    let _ = writeln!(out, "  打鍵数コスト  : {:.4}  （平均打鍵数 {:.4}, 1打鍵カバー率 {:.1}%）",
        stroke_cost, total_strokes, l1_coverage * 100.0);
    let _ = writeln!(out, "  難易度コスト  : {:.4}", uni_cost);
    let _ = writeln!(out, "  バイグラムコスト: {:.4}", bi_cost);
    let _ = writeln!(out, "  準交互ボーナス: {:.4}", tri_cost);
    let _ = writeln!(out, "  合計スコア    : {:.4}", total);
}

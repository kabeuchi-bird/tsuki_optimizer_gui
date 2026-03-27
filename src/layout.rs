// layout.rs — 月配列改変版のレイアウト定義

use std::collections::HashSet;
use std::io::Write;

use crate::chars::{
    CharId, MAX_CHARS, TOUTEN_ID, KUTEN_ID, DAKUTEN_ID, HANDAKUTEN_ID, VOID_CHAR_FIRST,
};

pub type SlotId = u8;

/// スロット配列の上限サイズ（3x11: 66スロット）
pub const MAX_SLOTS: usize = 66;

/// シフトキースロットのセンチネル値（slot_to_char でシフトキー位置に使用）
pub const SHIFT_SLOT_SENTINEL: CharId = u8::MAX;

/// Layer 1 上のDキースロット（3x10: row1, col2 → 、固定）
pub const D_SLOT: SlotId = 12;
/// Layer 1 上のKキースロット（3x10: row1, col7 → 。固定）
pub const K_SLOT: SlotId = 17;

// ──────────────────────────────────────────────────────────────
// キーボードサイズ設定
// ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardSize { K3x10, K3x11 }

/// レイアウト計算に必要なキーボード形状パラメータ
///
/// `Copy` なので値渡しで使用する。
#[derive(Clone, Copy, Debug)]
pub struct KeyboardParams {
    pub size: KeyboardSize,
    /// 列数（10 または 11）
    pub num_cols: u8,
    /// 1レイヤーのスロット数（30 または 33）
    pub num_slots_per_layer: u8,
    /// 全スロット数（60 または 66）
    pub num_slots: usize,
    /// 最適化対象の文字数（60 または 64）
    pub num_chars: usize,
    /// 左手シフトキーのスロット（L2右手文字を打つ際に押す）
    /// 3x10: D_SLOT=12（左中指）、3x11: ☆=13（row1,col2）
    pub shift_left: SlotId,
    /// 右手シフトキーのスロット（L2左手文字を打つ際に押す）
    /// 3x10: K_SLOT=17（右中指）、3x11: ★=18（row1,col7）
    pub shift_right: SlotId,
}

impl KeyboardParams {
    /// 3x10キーボード（デフォルト）
    pub fn k3x10() -> Self {
        KeyboardParams {
            size: KeyboardSize::K3x10,
            num_cols: 10,
            num_slots_per_layer: 30,
            num_slots: 60,
            num_chars: 60,
            shift_left:  D_SLOT,  // 12
            shift_right: K_SLOT,  // 17
        }
    }

    /// 3x11キーボード（右端に1列追加、☆★は同位置で固定専用シフトキー）
    ///
    /// ☆: row1, col2 → slot = 1*11+2 = 13
    /// ★: row1, col7 → slot = 1*11+7 = 18
    pub fn k3x11() -> Self {
        KeyboardParams {
            size: KeyboardSize::K3x11,
            num_cols: 11,
            num_slots_per_layer: 33,
            num_slots: 66,
            num_chars: 64,
            shift_left:  13,  // ☆ (row1, col2)
            shift_right: 18,  // ★ (row1, col7)
        }
    }
}

// ──────────────────────────────────────────────────────────────
// スロット計算ユーティリティ
// ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hand { Left, Right }

/// スロット番号からカラム（0 〜 num_cols-1）を得る
#[inline]
pub fn slot_col(s: SlotId, num_cols: u8) -> u8 { s % num_cols }

/// スロット番号からロウ（0=上段, 1=中段, 2=下段）を得る
#[inline]
pub fn slot_row(s: SlotId, num_cols: u8) -> u8 {
    (s % (num_cols * 3)) / num_cols
}

/// カラムから指番号を得る（0=左小指 … 7=右小指）
/// 3x10 の col 0-9 と 3x11 の col 0-10 の両方に対応
#[inline]
pub fn col_to_finger(col: u8) -> u8 {
    match col {
        0       => 0,  // 左小指
        1       => 1,  // 左薬指
        2       => 2,  // 左中指（☆/Dキー）
        3 | 4   => 3,  // 左人差し指
        5 | 6   => 4,  // 右人差し指
        7       => 5,  // 右中指（★/Kキー）
        8       => 6,  // 右薬指
        9 | 10  => 7,  // 右小指（col10 は3x11の追加列）
        _       => unreachable!(),
    }
}

/// スロットの手（左/右）
#[inline]
pub fn slot_hand(s: SlotId, num_cols: u8) -> Hand {
    if slot_col(s, num_cols) < 5 { Hand::Left } else { Hand::Right }
}

// ──────────────────────────────────────────────────────────────
// キーストローク（最大2打鍵）の軽量な表現
// ──────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug)]
pub struct Keystrokes {
    data: [SlotId; 2],
    len: u8,
}

impl Keystrokes {
    #[inline]
    pub fn one(a: SlotId) -> Self { Keystrokes { data: [a, 0], len: 1 } }
    #[inline]
    pub fn two(a: SlotId, b: SlotId) -> Self { Keystrokes { data: [a, b], len: 2 } }
    #[inline]
    pub fn as_slice(&self) -> &[SlotId] { &self.data[..self.len as usize] }
    #[inline]
    pub fn first(&self) -> SlotId { self.data[0] }
    #[inline]
    pub fn last(&self) -> SlotId { self.data[self.len as usize - 1] }
}

/// スロット番号からキーストロークを計算
#[inline]
pub fn keystrokes_for_slot(slot: SlotId, kp: KeyboardParams) -> Keystrokes {
    if (slot as usize) < kp.num_slots_per_layer as usize {
        // Layer 1: そのスロットを1打鍵するだけ
        Keystrokes::one(slot)
    } else {
        // Layer 2: 物理キー番号 = slot - num_slots_per_layer
        let physical = slot - kp.num_slots_per_layer;
        let col = slot_col(physical, kp.num_cols);
        // 左手キー → 右シフト（★）、右手キー → 左シフト（☆）
        let shift = if col < 5 { kp.shift_right } else { kp.shift_left };
        Keystrokes::two(shift, physical)
    }
}

// ──────────────────────────────────────────────────────────────
// レイアウト本体
// ──────────────────────────────────────────────────────────────
#[derive(Clone)]
pub struct Layout {
    pub kp: KeyboardParams,
    /// char_to_slot[c] = スロット番号
    pub char_to_slot: [SlotId; MAX_CHARS],
    /// slot_to_char[s] = その位置の文字ID
    /// シフトキースロット（3x11では13,18）は SHIFT_SLOT_SENTINEL
    pub slot_to_char: [CharId; MAX_SLOTS],
}

impl Layout {
    /// 初期配置を生成する
    ///
    /// 3x10: CharId i → SlotId i（既存の月配列2-263と一致）
    /// 3x11: CharId 0..63 を、シフトキースロット（13, 18）を除いた
    ///       スロット 0..65 に順番に割り当てる
    pub fn initial(kp: KeyboardParams) -> Self {
        let mut cts = [0u8; MAX_CHARS];
        let mut stc = [SHIFT_SLOT_SENTINEL; MAX_SLOTS];

        match kp.size {
            KeyboardSize::K3x10 => {
                for i in 0..60usize {
                    cts[i] = i as SlotId;
                    stc[i] = i as CharId;
                }
            }
            KeyboardSize::K3x11 => {
                // シフトキースロット（13, 18）をスキップして文字スロットを割り当てる
                let mut char_id = 0usize;
                for slot in 0u8..kp.num_slots as u8 {
                    if slot == kp.shift_left || slot == kp.shift_right {
                        // stc[slot] は SHIFT_SLOT_SENTINEL のまま
                        continue;
                    }
                    cts[char_id] = slot;
                    stc[slot as usize] = char_id as CharId;
                    char_id += 1;
                    if char_id >= kp.num_chars { break; }
                }
            }
        }

        Layout { kp, char_to_slot: cts, slot_to_char: stc }
    }

    /// 文字c1とc2のスロットを交換する（制約チェックなし、search層で行う）
    #[inline]
    pub fn swap_chars(&mut self, c1: CharId, c2: CharId) {
        let s1 = self.char_to_slot[c1 as usize];
        let s2 = self.char_to_slot[c2 as usize];
        self.char_to_slot[c1 as usize] = s2;
        self.char_to_slot[c2 as usize] = s1;
        self.slot_to_char[s1 as usize] = c2;
        self.slot_to_char[s2 as usize] = c1;
    }

    /// c が Layer 1 にいるか
    #[inline]
    pub fn is_l1(&self, c: CharId) -> bool {
        (self.char_to_slot[c as usize] as usize) < self.kp.num_slots_per_layer as usize
    }

    /// 文字の「主手」（Layer 2なら文字キー側の手）
    #[inline]
    pub fn primary_hand(&self, c: CharId) -> Hand {
        slot_hand(self.char_to_slot[c as usize], self.kp.num_cols)
    }

    /// 実打鍵数を返す
    /// 3x10: 。/、は K/D + Enter で 2打鍵
    /// 3x11: 。/、は通常文字扱い（L1なら1打鍵、L2なら2打鍵）
    #[inline]
    pub fn char_stroke_count(&self, c: CharId) -> u32 {
        if self.kp.size == KeyboardSize::K3x10 && (c == KUTEN_ID || c == TOUTEN_ID) {
            2  // K/D + Enter
        } else if self.is_l1(c) {
            1
        } else {
            2  // shift + key
        }
    }

    /// 現在のレイアウトを表示する
    pub fn display(&self, out: &mut impl Write) {
        use crate::chars::CHAR_LIST;
        let nc = self.kp.num_cols as usize;
        let npl = self.kp.num_slots_per_layer as usize;

        let _ = writeln!(out, "【Layer 1】");
        for row in 0u8..3 {
            let _ = write!(out, "  ");
            for col in 0..nc {
                let slot = (row as usize) * nc + col;
                // シフトキースロットは ☆/★ を表示
                if self.kp.size == KeyboardSize::K3x11
                    && (slot == self.kp.shift_left as usize
                        || slot == self.kp.shift_right as usize)
                {
                    let sym = if slot == self.kp.shift_left as usize { '☆' } else { '★' };
                    let _ = write!(out, "{} ", sym);
                } else {
                    let c = self.slot_to_char[slot];
                    let _ = write!(out, "{} ", if c == SHIFT_SLOT_SENTINEL { '?' } else { CHAR_LIST[c as usize] });
                }
            }
            let _ = writeln!(out);
        }
        let _ = writeln!(out, "【Layer 2】");
        for row in 0u8..3 {
            let _ = write!(out, "  ");
            for col in 0..nc {
                let slot = npl + (row as usize) * nc + col;
                let c = self.slot_to_char[slot];
                let _ = write!(out, "{} ", if c == SHIFT_SLOT_SENTINEL { '?' } else { CHAR_LIST[c as usize] });
            }
            let _ = writeln!(out);
        }
    }
}

// ──────────────────────────────────────────────────────────────
// スワップ後のスロットを仮計算（レイアウトを変更せずにデルタ評価用）
// ──────────────────────────────────────────────────────────────
#[inline]
pub fn slot_after_swap(layout: &Layout, swap_c1: CharId, swap_c2: CharId, c: CharId) -> SlotId {
    if c == swap_c1 {
        layout.char_to_slot[swap_c2 as usize]
    } else if c == swap_c2 {
        layout.char_to_slot[swap_c1 as usize]
    } else {
        layout.char_to_slot[c as usize]
    }
}

// ──────────────────────────────────────────────────────────────
// 移動制約チェック関数群（tabu search で使用）
// ──────────────────────────────────────────────────────────────

/// 文字cが固定（動かせない）かどうか
/// 3x10: 。と、は K/D スロット固定
/// 3x11: 固定文字なし（☆★はスロットとして管理され、CharIdを持たない）
#[inline]
pub fn is_fixed(c: CharId, kp: KeyboardParams) -> bool {
    match kp.size {
        KeyboardSize::K3x10 => c == TOUTEN_ID || c == KUTEN_ID,
        KeyboardSize::K3x11 => false,
    }
}

/// 文字cがLayer1専用（Layer2へ移動不可）かどうか
#[inline]
pub fn is_l1_only(c: CharId) -> bool {
    c == DAKUTEN_ID || c == HANDAKUTEN_ID
}

/// 文字cが層間移動可能かどうか
#[inline]
pub fn is_inter_layer_movable(c: CharId, kp: KeyboardParams) -> bool {
    !is_fixed(c, kp) && !is_l1_only(c)
}

// ──────────────────────────────────────────────────────────────
// 排他配置ペア制約
// ──────────────────────────────────────────────────────────────

/// 排他配置ペア：GroupAとGroupBのかなを同一物理キーのL1/L2に共存させない
pub struct ExclusivePair {
    pub group_a: HashSet<CharId>,
    pub group_b: HashSet<CharId>,
}

impl ExclusivePair {
    /// L1/L2のペアが制約に違反するか（どちらの向きも対称）
    #[inline]
    pub fn violates(&self, l1_c: CharId, l2_c: CharId) -> bool {
        (self.group_a.contains(&l1_c) && self.group_b.contains(&l2_c))
            || (self.group_b.contains(&l1_c) && self.group_a.contains(&l2_c))
    }
}

/// スワップ (c1↔c2) 後に特定スロットに配置される文字IDを返す（レイアウト変更なし）
#[inline]
fn char_at_slot_after_swap(layout: &Layout, c1: CharId, c2: CharId, slot: usize) -> CharId {
    let s1 = layout.char_to_slot[c1 as usize] as usize;
    let s2 = layout.char_to_slot[c2 as usize] as usize;
    if slot == s1 { c2 } else if slot == s2 { c1 } else { layout.slot_to_char[slot] }
}

/// L1スロット l1_slot とその対応 L2スロット (l1_slot + npl) のペアが、
/// スワップ (c1↔c2) 後に排他ペア制約を違反するか
fn pair_violates_after_swap(
    layout: &Layout,
    c1: CharId, c2: CharId,
    l1_slot: usize,
    pairs: &[ExclusivePair],
) -> bool {
    let npl = layout.kp.num_slots_per_layer as usize;
    let l2_slot = l1_slot + npl;
    let l1_c = char_at_slot_after_swap(layout, c1, c2, l1_slot);
    let l2_c = char_at_slot_after_swap(layout, c1, c2, l2_slot);
    // SHIFT_SLOT_SENTINEL(255) も void chars(>=62) もここで除外される
    if l1_c >= VOID_CHAR_FIRST || l2_c >= VOID_CHAR_FIRST { return false; }
    pairs.iter().any(|p| p.violates(l1_c, l2_c))
}

/// スワップ (c1↔c2) が排他ペア制約に違反するか（影響する最大2スロット列を確認）
pub fn swap_would_violate(
    layout: &Layout,
    c1: CharId, c2: CharId,
    pairs: &[ExclusivePair],
) -> bool {
    if pairs.is_empty() { return false; }
    let npl = layout.kp.num_slots_per_layer as usize;
    let s1 = layout.char_to_slot[c1 as usize] as usize;
    let s2 = layout.char_to_slot[c2 as usize] as usize;
    let l1_s1 = if s1 < npl { s1 } else { s1 - npl };
    let l1_s2 = if s2 < npl { s2 } else { s2 - npl };
    pair_violates_after_swap(layout, c1, c2, l1_s1, pairs)
        || (l1_s2 != l1_s1 && pair_violates_after_swap(layout, c1, c2, l1_s2, pairs))
}

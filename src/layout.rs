// layout.rs — 月配列改変版のレイアウト定義

use std::collections::HashSet;
use std::io::Write;

use crate::chars::{CharId, KUTEN_ID, MAX_CHARS, TOUTEN_ID, VOID_CHAR_FIRST};

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
pub enum KeyboardSize {
    K3x10,
    K3x11,
}

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
            shift_left: D_SLOT,  // 12
            shift_right: K_SLOT, // 17
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
            shift_left: 13,  // ☆ (row1, col2)
            shift_right: 18, // ★ (row1, col7)
        }
    }
}

// ──────────────────────────────────────────────────────────────
// スロット計算ユーティリティ
// ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hand {
    Left,
    Right,
}

/// スロット番号からカラム（0 〜 num_cols-1）を得る
#[inline]
pub fn slot_col(s: SlotId, num_cols: u8) -> u8 {
    s % num_cols
}

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
        0 => 0,      // 左小指
        1 => 1,      // 左薬指
        2 => 2,      // 左中指（☆/Dキー）
        3 | 4 => 3,  // 左人差し指
        5 | 6 => 4,  // 右人差し指
        7 => 5,      // 右中指（★/Kキー）
        8 => 6,      // 右薬指
        9 | 10 => 7, // 右小指（col10 は3x11の追加列）
        _ => unreachable!(),
    }
}

/// スロットの手（左/右）
#[inline]
pub fn slot_hand(s: SlotId, num_cols: u8) -> Hand {
    if slot_col(s, num_cols) < 5 {
        Hand::Left
    } else {
        Hand::Right
    }
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
    pub fn one(a: SlotId) -> Self {
        Keystrokes {
            data: [a, 0],
            len: 1,
        }
    }
    #[inline]
    pub fn two(a: SlotId, b: SlotId) -> Self {
        Keystrokes {
            data: [a, b],
            len: 2,
        }
    }
    #[inline]
    pub fn as_slice(&self) -> &[SlotId] {
        &self.data[..self.len as usize]
    }
    #[inline]
    pub fn first(&self) -> SlotId {
        self.data[0]
    }
    #[inline]
    pub fn last(&self) -> SlotId {
        self.data[self.len as usize - 1]
    }
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
        let shift = if col < 5 {
            kp.shift_right
        } else {
            kp.shift_left
        };
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
    /// 3x11: 月配列2-263を3x11に拡張した配置
    ///       - L1 Row 0 col10 に「ち」、Row 1 col10 に「れ」を移動
    ///       - 「、」「。」をL1 Row 2 col7,8 に配置
    ///       - L2 col10 に「」と void を配置
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
                // 月配列2-263の3x11初期配置
                //
                // 3x11 スロット番号:
                //   L1 Row 0: slot  0-10    L2 Row 0: slot 33-43
                //   L1 Row 1: slot 11-21    L2 Row 1: slot 44-54
                //   L1 Row 2: slot 22-32    L2 Row 2: slot 55-65
                //   ☆ = slot 13 (L1のみ)  ★ = slot 18 (L1のみ)
                //   L2 の slot 46,51 は文字スロット（★→☆キー / ☆→★キーでアクセス）
                //
                // Layer 1 (31文字スロット):
                //   Row 0: そ こ し て ょ つ ん い の り ち
                //   Row 1: は か ☆ と た く う ★ ゛ き れ
                //   Row 2: す け に な さ っ る 、 。 ゜ □
                // Layer 2 (33文字スロット):
                //   Row 0: ぁ ひ ほ ふ め ぬ え み や ぇ 「
                //   Row 1: ぃ を ら あ よ ま お も わ ゆ 」
                //   Row 2: ぅ へ せ ゅ ゃ む ろ ね ー ぉ □
                //
                // 3x10 との差分:
                //   ち(27): L1 Row2 col7 → L1 Row0 col10
                //   れ(28): L1 Row2 col8 → L1 Row1 col10
                //   、(12): L1 Row1 col2 → L1 Row2 col7（☆がcol2を占有）
                //   。(17): L1 Row1 col7 → L1 Row2 col8（★がcol7を占有）
                //   「(60): L2 Row0 col10（新規）
                //   」(61): L2 Row1 col10（新規）
                //   □(63): L1 Row2 col10、□(62): L2 Row2 col10（void）
                #[rustfmt::skip]
                let c2s: [SlotId; 64] = [
                //  CharId:  0   1   2   3   4   5   6   7   8   9
                /*  0- 9 */  0,  1,  2,  3,  4,  5,  6,  7,  8,  9,
                /* 10-19 */ 11, 12, 29, 14, 15, 16, 17, 30, 19, 20,
                /* 20-29 */ 22, 23, 24, 25, 26, 27, 28, 10, 21, 31,
                /* 30-39 */ 33, 34, 35, 36, 37, 38, 39, 40, 41, 42,
                /* 40-49 */ 44, 45, 46, 47, 48, 49, 50, 51, 52, 53,
                /* 50-59 */ 55, 56, 57, 58, 59, 60, 61, 62, 63, 64,
                /* 60-63 */ 43, 54, 65, 32,
                ];
                for (c, &s) in c2s.iter().enumerate() {
                    cts[c] = s;
                    stc[s as usize] = c as CharId;
                }
            }
        }

        Layout {
            kp,
            char_to_slot: cts,
            slot_to_char: stc,
        }
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
            2 // K/D + Enter
        } else if self.is_l1(c) {
            1
        } else {
            2 // shift + key
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
                    && (slot == self.kp.shift_left as usize || slot == self.kp.shift_right as usize)
                {
                    let sym = if slot == self.kp.shift_left as usize {
                        '☆'
                    } else {
                        '★'
                    };
                    let _ = write!(out, "{} ", sym);
                } else {
                    let c = self.slot_to_char[slot];
                    let _ = write!(
                        out,
                        "{} ",
                        if c == SHIFT_SLOT_SENTINEL {
                            '?'
                        } else {
                            CHAR_LIST[c as usize]
                        }
                    );
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
                let _ = write!(
                    out,
                    "{} ",
                    if c == SHIFT_SLOT_SENTINEL {
                        '?'
                    } else {
                        CHAR_LIST[c as usize]
                    }
                );
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

/// 文字cが層間移動可能かどうか
#[inline]
pub fn is_inter_layer_movable(c: CharId, kp: KeyboardParams, l1_only: &HashSet<CharId>) -> bool {
    !is_fixed(c, kp) && !l1_only.contains(&c)
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
    if slot == s1 {
        c2
    } else if slot == s2 {
        c1
    } else {
        layout.slot_to_char[slot]
    }
}

/// L1スロット l1_slot とその対応 L2スロット (l1_slot + npl) のペアが、
/// スワップ (c1↔c2) 後に排他ペア制約を違反するか
fn pair_violates_after_swap(
    layout: &Layout,
    c1: CharId,
    c2: CharId,
    l1_slot: usize,
    pairs: &[ExclusivePair],
) -> bool {
    let npl = layout.kp.num_slots_per_layer as usize;
    let l2_slot = l1_slot + npl;
    let l1_c = char_at_slot_after_swap(layout, c1, c2, l1_slot);
    let l2_c = char_at_slot_after_swap(layout, c1, c2, l2_slot);
    // SHIFT_SLOT_SENTINEL(255) も void chars(>=62) もここで除外される
    if l1_c >= VOID_CHAR_FIRST || l2_c >= VOID_CHAR_FIRST {
        return false;
    }
    pairs.iter().any(|p| p.violates(l1_c, l2_c))
}

/// スワップ (c1↔c2) が排他ペア制約に違反するか（影響する最大2スロット列を確認）
pub fn swap_would_violate(
    layout: &Layout,
    c1: CharId,
    c2: CharId,
    pairs: &[ExclusivePair],
) -> bool {
    if pairs.is_empty() {
        return false;
    }
    let npl = layout.kp.num_slots_per_layer as usize;
    let s1 = layout.char_to_slot[c1 as usize] as usize;
    let s2 = layout.char_to_slot[c2 as usize] as usize;
    let l1_s1 = if s1 < npl { s1 } else { s1 - npl };
    let l1_s2 = if s2 < npl { s2 } else { s2 - npl };
    pair_violates_after_swap(layout, c1, c2, l1_s1, pairs)
        || (l1_s2 != l1_s1 && pair_violates_after_swap(layout, c1, c2, l1_s2, pairs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chars::{KUTEN_ID, TOUTEN_ID};

    #[test]
    fn test_slot_col_row() {
        // 3x10: slot 0 = row0, col0
        assert_eq!(slot_col(0, 10), 0);
        assert_eq!(slot_row(0, 10), 0);
        // slot 15 = row1, col5
        assert_eq!(slot_col(15, 10), 5);
        assert_eq!(slot_row(15, 10), 1);
    }

    #[test]
    fn test_col_to_finger() {
        assert_eq!(col_to_finger(0), 0); // 左小指
        assert_eq!(col_to_finger(4), 3); // 左人差指
        assert_eq!(col_to_finger(5), 4); // 右人差指
        assert_eq!(col_to_finger(9), 7); // 右小指
    }

    #[test]
    fn test_is_fixed_3x10() {
        let kp = KeyboardParams::k3x10();
        assert!(is_fixed(KUTEN_ID, kp));
        assert!(is_fixed(TOUTEN_ID, kp));
        assert!(!is_fixed(0, kp)); // 'そ' は固定ではない
    }

    #[test]
    fn test_swap_chars_integrity() {
        let kp = KeyboardParams::k3x10();
        let mut layout = Layout::initial(kp);
        let c1: CharId = 0;
        let c2: CharId = 1;
        let s1_before = layout.char_to_slot[c1 as usize];
        let s2_before = layout.char_to_slot[c2 as usize];

        layout.swap_chars(c1, c2);

        // char_to_slot が入れ替わっている
        assert_eq!(layout.char_to_slot[c1 as usize], s2_before);
        assert_eq!(layout.char_to_slot[c2 as usize], s1_before);
        // slot_to_char も整合
        assert_eq!(layout.slot_to_char[s1_before as usize], c2);
        assert_eq!(layout.slot_to_char[s2_before as usize], c1);
    }

    #[test]
    fn test_initial_layout_3x10() {
        let kp = KeyboardParams::k3x10();
        let layout = Layout::initial(kp);
        // 3x10: CharId i → SlotId i
        assert_eq!(layout.char_to_slot[0], 0);
        assert_eq!(layout.char_to_slot[59], 59);
        assert_eq!(layout.slot_to_char[0], 0);
    }

    #[test]
    fn test_initial_layout_3x11() {
        let kp = KeyboardParams::k3x11();
        let layout = Layout::initial(kp);
        // char_to_slot と slot_to_char の双方向整合性
        for c in 0..64u8 {
            let s = layout.char_to_slot[c as usize];
            assert_eq!(layout.slot_to_char[s as usize], c, "slot_to_char mismatch for char {c}");
        }
        // シフトキースロットは SHIFT_SLOT_SENTINEL
        assert_eq!(layout.slot_to_char[13], SHIFT_SLOT_SENTINEL); // ☆
        assert_eq!(layout.slot_to_char[18], SHIFT_SLOT_SENTINEL); // ★
        // 全64文字が一意なスロットに配置されている
        let mut slots_used: std::collections::HashSet<u8> = std::collections::HashSet::new();
        for c in 0..64u8 {
            assert!(slots_used.insert(layout.char_to_slot[c as usize]), "duplicate slot for char {c}");
        }
    }
}

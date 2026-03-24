// layout.rs — 月配列改変版のレイアウト定義

use crate::chars::{CharId, NUM_CHARS, TOUTEN_ID, KUTEN_ID, DAKUTEN_ID, HANDAKUTEN_ID};

pub type SlotId = u8;
pub const NUM_SLOTS: usize = 60;

/// Layer 1 上のDキースロット（row1, col2 → 、固定）
pub const D_SLOT: SlotId = 12;
/// Layer 1 上のKキースロット（row1, col7 → 。固定）
pub const K_SLOT: SlotId = 17;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hand { Left, Right }

/// スロット番号からカラム（0-9）を得る
/// L1: slot % 10, L2: (slot-30) % 10 = slot % 10 でも同じ
#[inline]
pub fn slot_col(s: SlotId) -> u8 { s % 10 }

/// スロット番号からロウ（0=上段, 1=中段, 2=下段）を得る
#[inline]
pub fn slot_row(s: SlotId) -> u8 { (s % 30) / 10 }

/// カラムから指番号を得る（0=左小指 … 7=右小指）
#[inline]
pub fn col_to_finger(col: u8) -> u8 {
    match col {
        0 => 0,        // 左小指
        1 => 1,        // 左薬指
        2 => 2,        // 左中指 (Dキー)
        3 | 4 => 3,    // 左人差し指
        5 | 6 => 4,    // 右人差し指
        7 => 5,        // 右中指 (Kキー)
        8 => 6,        // 右薬指
        9 => 7,        // 右小指
        _ => unreachable!(),
    }
}

/// スロットの手（左/右）
#[inline]
pub fn slot_hand(s: SlotId) -> Hand {
    if slot_col(s) < 5 { Hand::Left } else { Hand::Right }
}

/// ——————————————————————————————
/// キーストローク（最大2打鍵）の軽量な表現
/// ——————————————————————————————
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

/// ——————————————————————————————
/// レイアウト本体
/// ——————————————————————————————
#[derive(Clone)]
pub struct Layout {
    /// char_to_slot[c] = スロット番号（0..59）
    pub char_to_slot: [SlotId; NUM_CHARS],
    /// slot_to_char[s] = その位置の文字ID
    pub slot_to_char: [CharId; NUM_SLOTS],
}

impl Layout {
    /// 初期配置：CharId i → SlotId i（画像の月配列2-263と一致）
    pub fn initial() -> Self {
        let mut cts = [0u8; NUM_CHARS];
        let mut stc = [0u8; NUM_SLOTS];
        for i in 0..NUM_CHARS {
            cts[i] = i as SlotId;
            stc[i] = i as CharId;
        }
        Layout { char_to_slot: cts, slot_to_char: stc }
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

    /// c がLayer 1にいるか
    #[inline]
    pub fn is_l1(&self, c: CharId) -> bool {
        self.char_to_slot[c as usize] < 30
    }

    /// 文字の「主手」：Layer 2なら文字キー側の手（シフトキー側ではない）
    #[inline]
    pub fn primary_hand(&self, c: CharId) -> Hand {
        slot_hand(self.char_to_slot[c as usize])
    }

    /// 実打鍵数（Enterを含む）
    #[inline]
    pub fn char_stroke_count(&self, c: CharId) -> u32 {
        if c == KUTEN_ID || c == TOUTEN_ID {
            2  // K/D + Enter
        } else if self.is_l1(c) {
            1
        } else {
            2  // shift + key
        }
    }

    /// 現在のレイアウトを表示する
    pub fn display(&self) {
        use crate::chars::CHAR_LIST;
        println!("【Layer 1】");
        for row in 0u8..3 {
            print!("  ");
            for col in 0u8..10 {
                let slot = row * 10 + col;
                let c = self.slot_to_char[slot as usize];
                let ch = CHAR_LIST[c as usize];
                print!("{} ", ch);
            }
            println!();
        }
        println!("【Layer 2】");
        for row in 0u8..3 {
            print!("  ");
            for col in 0u8..10 {
                let slot = 30 + row * 10 + col;
                let c = self.slot_to_char[slot as usize];
                let ch = CHAR_LIST[c as usize];
                print!("{} ", ch);
            }
            println!();
        }
    }
}

/// スロット番号からキーストロークを計算（c不要で計算できる汎用版）
#[inline]
pub fn keystrokes_for_slot(slot: SlotId) -> Keystrokes {
    if slot < 30 {
        // Layer 1: そのスロットを1打鍵するだけ
        Keystrokes::one(slot)
    } else {
        // Layer 2: 物理キー番号 = slot - 30
        let physical = slot - 30;
        let col = physical % 10;
        let shift = if col < 5 { K_SLOT } else { D_SLOT };
        Keystrokes::two(shift, physical)
    }
}

/// ——————————————————————————————
/// スワップ後のスロットを仮計算（レイアウトを変更せずにデルタ評価用）
/// ——————————————————————————————
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

/// ——————————————————————————————
/// 移動制約チェック関数群（tabu search で使用）
/// ——————————————————————————————

/// 文字cが固定（動かせない）かどうか
#[inline]
pub fn is_fixed(c: CharId) -> bool {
    c == TOUTEN_ID || c == KUTEN_ID
}

/// 文字cがLayer1専用（Layer2へ移動不可）かどうか
#[inline]
pub fn is_l1_only(c: CharId) -> bool {
    c == DAKUTEN_ID || c == HANDAKUTEN_ID
}

/// 文字cが層間移動可能かどうか
#[inline]
pub fn is_inter_layer_movable(c: CharId) -> bool {
    !is_fixed(c) && !is_l1_only(c)
}

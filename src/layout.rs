// layout.rs — 月配列改変版のレイアウト定義

use std::collections::HashSet;
use std::io::Write;

use crate::chars::{CharId, KUTEN_ID, MAX_CHARS, TOUTEN_ID, VOID_CHAR_FIRST};

pub type SlotId = u8;

/// スロット配列の上限サイズ。
/// 3層構成（L1 / L2 / 拗音面）の 3x11: 33スロット × 3層 = 99スロット。
/// `mode = "none"` では 2層分（60 or 66）のみ使用する。
pub const MAX_SLOTS: usize = 99;

/// シフトキースロットのセンチネル値（slot_to_char でシフトキー位置に使用）
pub const SHIFT_SLOT_SENTINEL: CharId = u8::MAX;

/// Layer 1 上のDキースロット（3x10: row1, col2 → 、固定）
pub const D_SLOT: SlotId = 12;
/// Layer 1 上のKキースロット（3x10: row1, col7 → 。固定）
pub const K_SLOT: SlotId = 17;
/// 3x10_single_shift の単一シフトキースロット（row0, col2 → E位置、、固定）
pub const E_SHIFT_SLOT: SlotId = 2;

// ──────────────────────────────────────────────────────────────
// キーボードサイズ設定
// ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardSize {
    K3x10,
    /// 3x10と同じグリッドだが、シフトキーをE位置（slot2）の1個のみとし、、を固定配置する
    K3x10SingleShift,
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

    /// ハイブリッド拗音方式（第3層=拗音面）が有効か。
    pub yoon: bool,
    /// 拗音面の子音数（有効時のみ非0）。CharId は [CONSONANT_FIRST, +num_consonants)。
    pub num_consonants: u8,
    /// 拗音面の void 数（子音で埋まらない使用可能スロットの数）。
    /// CharId は [CONSONANT_FIRST+num_consonants, +num_yoon_void)。
    pub num_yoon_void: u8,
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
            yoon: false,
            num_consonants: 0,
            num_yoon_void: 0,
        }
    }

    /// 3x10キーボード（単一シフト版）
    ///
    /// グリッド・スロット数・文字数は 3x10 と同一。
    /// シフトキーは E位置（row0, col2 = slot 2）の1個のみで、、をそこに固定する。
    /// 左右どちらのL2文字もこの単一シフトキーで打鍵するため、shift_left = shift_right = 2。
    pub fn k3x10_single_shift() -> Self {
        KeyboardParams {
            size: KeyboardSize::K3x10SingleShift,
            num_cols: 10,
            num_slots_per_layer: 30,
            num_slots: 60,
            num_chars: 60,
            shift_left: E_SHIFT_SLOT,  // 2（単一シフト）
            shift_right: E_SHIFT_SLOT, // 2（単一シフト）
            yoon: false,
            num_consonants: 0,
            num_yoon_void: 0,
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
            yoon: false,
            num_consonants: 0,
            num_yoon_void: 0,
        }
    }

    /// シフトキーの物理キー数（単一シフトは1、それ以外は2）。
    pub fn num_shift_keys(&self) -> usize {
        if self.shift_left == self.shift_right {
            1
        } else {
            2
        }
    }

    /// 拗音面を有効化した KeyboardParams を返す。
    ///
    /// 拗音面（第3層）のスロットは全て文字（子音 or void）を持ち、センチネルは無い。
    /// 子音を置けない物理位置は「同一物理キー排他」制約で void に固定される。
    /// 内訳: シフトキー位置（前置シフトと衝突）が num_shift_keys 個、ゃゅょ の物理位置
    /// （自己参照 [TT] を回避）が 3 個。
    /// よって子音の最大数 = num_slots_per_layer − num_shift_keys − 3、
    /// void 数 = num_slots_per_layer − 子音数。
    ///
    /// 子音数が上限を超える場合は Err を返す。
    pub fn with_yoon(mut self, num_consonants: usize) -> Result<Self, String> {
        let npl = self.num_slots_per_layer as usize;
        let reserved = self.num_shift_keys() + 3;
        let max_consonants = npl.checked_sub(reserved).ok_or_else(|| {
            format!("拗音面のスロットが不足しています（層スロット {npl} < 予約 {reserved}）")
        })?;
        if num_consonants > max_consonants {
            return Err(format!(
                "子音数({})が拗音面の上限({})を超えています",
                num_consonants, max_consonants
            ));
        }
        self.yoon = true;
        self.num_consonants = num_consonants as u8;
        self.num_yoon_void = (npl - num_consonants) as u8;
        self.num_slots = npl * 3;
        Ok(self)
    }

    /// 拗音面の文字数（子音 + void）。
    pub fn yoon_char_count(&self) -> usize {
        self.num_consonants as usize + self.num_yoon_void as usize
    }

    /// 拗音面 CharId 区間 [first, end)。無効時は空区間。
    pub fn yoon_char_range(&self) -> std::ops::Range<usize> {
        let first = crate::chars::CONSONANT_FIRST as usize;
        first..(first + self.yoon_char_count())
    }

    /// c が有効な子音 CharId か（このパラメータ下で）。
    pub fn is_consonant(&self, c: CharId) -> bool {
        let first = crate::chars::CONSONANT_FIRST;
        self.yoon && c >= first && (c as usize) < first as usize + self.num_consonants as usize
    }

    /// c が拗音面の void CharId か。
    pub fn is_yoon_void(&self, c: CharId) -> bool {
        let first = crate::chars::CONSONANT_FIRST as usize + self.num_consonants as usize;
        self.yoon && (c as usize) >= first && (c as usize) < first + self.num_yoon_void as usize
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
    let npl = kp.num_slots_per_layer;
    if slot < npl {
        // Layer 1: そのスロットを1打鍵するだけ
        Keystrokes::one(slot)
    } else if slot < 2 * npl {
        // Layer 2: 物理キー番号 = slot - num_slots_per_layer
        let physical = slot - npl;
        let col = slot_col(physical, kp.num_cols);
        // 左手キー → 右シフト（★）、右手キー → 左シフト（☆）
        let shift = if col < 5 {
            kp.shift_right
        } else {
            kp.shift_left
        };
        Keystrokes::two(shift, physical)
    } else {
        // 拗音面（子音）: 物理キー = slot - 2*num_slots_per_layer を1打。
        // 直前のL1文字はキャンセルされるためコスト外（ゃゅょシフトは別文字として集計）。
        Keystrokes::one(slot - 2 * npl)
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
            KeyboardSize::K3x10 | KeyboardSize::K3x10SingleShift => {
                for i in 0..60usize {
                    cts[i] = i as SlotId;
                    stc[i] = i as CharId;
                }
                if kp.size == KeyboardSize::K3x10SingleShift {
                    // 、(TOUTEN_ID) を単一シフト位置 E_SHIFT_SLOT へ固定配置する。
                    // 元々その位置にあった文字は 、の旧スロットへ退避（純粋なスワップ）。
                    let dst = E_SHIFT_SLOT as usize;
                    let src = TOUTEN_ID as usize;
                    let cd = stc[dst];
                    let cs = stc[src];
                    stc[dst] = cs;
                    stc[src] = cd;
                    cts[cd as usize] = src as SlotId;
                    cts[cs as usize] = dst as SlotId;
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

    /// c が拗音面（第3層）にいるか（＝子音。hybrid のみ）
    #[inline]
    pub fn is_yoon_char(&self, c: CharId) -> bool {
        self.kp.yoon
            && (self.char_to_slot[c as usize] as usize) >= 2 * self.kp.num_slots_per_layer as usize
    }

    /// 文字の「主手」（Layer 2なら文字キー側の手）
    #[inline]
    pub fn primary_hand(&self, c: CharId) -> Hand {
        slot_hand(self.char_to_slot[c as usize], self.kp.num_cols)
    }

    /// 実打鍵数を返す
    /// 3x10: 。/、は K/D + Enter で 2打鍵
    /// 3x10_single_shift: 、は E（シフトキー）+ Enter で 2打鍵（。は通常文字扱い）
    /// 3x11: 。/、は通常文字扱い（L1なら1打鍵、L2なら2打鍵）
    #[inline]
    pub fn char_stroke_count(&self, c: CharId) -> u32 {
        if punct_needs_enter(c, self.kp.size) {
            2 // シフトキー + Enter
        } else if self.is_l1(c) || self.is_yoon_char(c) {
            // L1 文字 / 拗音面の子音はともに1打
            // （拗音シフト ゃゅょ は L1 の別文字として集計される）
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
/// 3x10_single_shift: 、のみ単一シフトキーに固定（。は自由）
/// 3x11: 固定文字なし（☆★はスロットとして管理され、CharIdを持たない）
#[inline]
pub fn is_fixed(c: CharId, kp: KeyboardParams) -> bool {
    match kp.size {
        KeyboardSize::K3x10 => c == TOUTEN_ID || c == KUTEN_ID,
        KeyboardSize::K3x10SingleShift => c == TOUTEN_ID,
        KeyboardSize::K3x11 => false,
    }
}

/// 文字cがシフトキー兼用のため Enter 確定で打鍵数+1（合計2打鍵）になるか
/// 3x10: 、。（D/K がシフトキー兼用）
/// 3x10_single_shift: 、（E がシフトキー兼用。。は通常文字）
/// 3x11: なし
#[inline]
pub fn punct_needs_enter(c: CharId, size: KeyboardSize) -> bool {
    match size {
        KeyboardSize::K3x10 => c == KUTEN_ID || c == TOUTEN_ID,
        KeyboardSize::K3x10SingleShift => c == TOUTEN_ID,
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

/// c が拗音シフトかな（ゃゅょ）の CharId か。
#[inline]
pub fn is_yoon_shift_id(c: CharId) -> bool {
    c == crate::chars::YA_ID || c == crate::chars::YU_ID || c == crate::chars::YO_ID
}

/// 物理位置 p が前置シフトキー位置か。
#[inline]
fn is_shift_physical(kp: KeyboardParams, p: usize) -> bool {
    p == kp.shift_left as usize || p == kp.shift_right as usize
}

/// 拗音方式の「同一物理キー排他」制約（ExclusivePair の層違い版）。
///
/// 拗音面 physical p に子音を置けるのは、L1 physical p が
/// 「前置シフトキー」でも「ゃゅょ」でもない場合に限る（自己参照 [TT] や
/// シフト衝突を回避）。スワップ (c1↔c2) 後にこの不変条件が破れるか判定する。
///
/// L1内スワップ（ゃゅょ移動）と拗音面内スワップ（子音移動）は層をまたがないため、
/// 反対側の層は不変。よって現在の slot_to_char をそのまま参照できる。
fn yoon_swap_violates(layout: &Layout, c1: CharId, c2: CharId) -> bool {
    if !layout.kp.yoon {
        return false;
    }
    let npl = layout.kp.num_slots_per_layer as usize;
    for &c in &[c1, c2] {
        let new_slot = slot_after_swap(layout, c1, c2, c) as usize;
        if is_yoon_shift_id(c) {
            // ゃゅょ が L1 physical p へ → 拗音面 p が子音なら違反
            if new_slot < npl {
                let yoon_c = layout.slot_to_char[2 * npl + new_slot];
                if layout.kp.is_consonant(yoon_c) {
                    return true;
                }
            }
        } else if layout.kp.is_consonant(c) {
            // 子音が拗音面 physical p へ → L1 p が shift/ゃゅょ なら違反
            if new_slot >= 2 * npl {
                let p = new_slot - 2 * npl;
                if is_shift_physical(layout.kp, p) || is_yoon_shift_id(layout.slot_to_char[p]) {
                    return true;
                }
            }
        }
    }
    false
}

/// スワップ (c1↔c2) が制約（排他ペア + 拗音同一物理キー排他）に違反するか
pub fn swap_would_violate(
    layout: &Layout,
    c1: CharId,
    c2: CharId,
    pairs: &[ExclusivePair],
) -> bool {
    if yoon_swap_violates(layout, c1, c2) {
        return true;
    }
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
    fn test_with_yoon_params() {
        // 拗音面は全スロットが文字。void 数 = npl − 子音数。
        // 3x10: 上限 = 30 − 2(D/K) − 3(ゃゅょ) = 25
        let kp = KeyboardParams::k3x10().with_yoon(11).unwrap();
        assert!(kp.yoon);
        assert_eq!(kp.num_consonants, 11);
        assert_eq!(kp.num_yoon_void, 19); // 30 - 11
        assert_eq!(kp.yoon_char_count(), 30); // 全スロットが文字
        assert_eq!(kp.num_slots, 90); // 3 * 30
                                      // 単一シフト: 上限 = 30 − 1 − 3 = 26
        let ss = KeyboardParams::k3x10_single_shift().with_yoon(11).unwrap();
        assert_eq!(ss.num_yoon_void, 19);
        // 3x11: 上限 = 33 − 2 − 3 = 28
        let k11 = KeyboardParams::k3x11().with_yoon(11).unwrap();
        assert_eq!(k11.num_yoon_void, 22);
        assert_eq!(k11.num_slots, 99);
        // 子音上限
        assert!(KeyboardParams::k3x10().with_yoon(25).is_ok());
        assert!(KeyboardParams::k3x10().with_yoon(26).is_err());
    }

    #[test]
    fn test_yoon_char_predicates() {
        let kp = KeyboardParams::k3x10().with_yoon(2).unwrap();
        let first = crate::chars::CONSONANT_FIRST;
        assert!(kp.is_consonant(first));
        assert!(kp.is_consonant(first + 1));
        assert!(!kp.is_consonant(first + 2)); // 3個目は無効
        assert!(kp.is_yoon_void(first + 2)); // void 区間
        assert!(!kp.is_consonant(0)); // 基底文字は子音でない
                                      // yoon 無効時は常に false
        let plain = KeyboardParams::k3x10();
        assert!(!plain.is_consonant(first));
        assert!(!plain.is_yoon_void(first));
    }

    #[test]
    fn test_yoon_stroke_counts() {
        // 拗音ユニットの打鍵数: きゃ=2, きょう=3, ぎょう=3
        let map = crate::chars::build_char_to_id();
        let kp = KeyboardParams::k3x10().with_yoon(2).unwrap();
        let ky = crate::chars::CONSONANT_FIRST; // 64
        let gy = crate::chars::CONSONANT_FIRST + 1; // 65
        let npl = kp.num_slots_per_layer;

        let mut layout = Layout::initial(kp);
        // ゃ を L1 へ移動（slot0 の文字と交換）
        let ya = map[&'ゃ'];
        let displaced = layout.slot_to_char[0];
        layout.swap_chars(ya, displaced);
        // 子音 Ky, Gy を拗音面スロットへ配置
        for (cons, phys) in [(ky, 3u8), (gy, 5u8)] {
            let slot = 2 * npl + phys;
            layout.char_to_slot[cons as usize] = slot;
            layout.slot_to_char[slot as usize] = cons;
        }

        let yo = map[&'ょ']; // slot 4 (L1)
        let u = map[&'う']; // slot 16 (L1)

        // 個別打鍵数
        assert_eq!(layout.char_stroke_count(ya), 1); // ゃ は L1
        assert_eq!(layout.char_stroke_count(yo), 1); // ょ は L1
        assert_eq!(layout.char_stroke_count(u), 1); // う は L1
        assert_eq!(layout.char_stroke_count(ky), 1); // 子音は1打
        assert_eq!(layout.char_stroke_count(gy), 1);
        assert!(layout.is_yoon_char(ky));
        assert!(!layout.is_yoon_char(ya));

        // ユニット合計
        let kya = layout.char_stroke_count(ky) + layout.char_stroke_count(ya);
        let kyou =
            layout.char_stroke_count(ky) + layout.char_stroke_count(yo) + layout.char_stroke_count(u);
        let gyou =
            layout.char_stroke_count(gy) + layout.char_stroke_count(yo) + layout.char_stroke_count(u);
        assert_eq!(kya, 2, "きゃ");
        assert_eq!(kyou, 3, "きょう");
        assert_eq!(gyou, 3, "ぎょう");

        // 拗音面スロットは1打（物理キー）
        let ks = keystrokes_for_slot(2 * npl + 3, kp);
        assert_eq!(ks.as_slice(), &[3]);
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
    fn test_single_shift_params_and_fixed() {
        let kp = KeyboardParams::k3x10_single_shift();
        // 単一シフト: shift_left == shift_right == E_SHIFT_SLOT
        assert_eq!(kp.shift_left, E_SHIFT_SLOT);
        assert_eq!(kp.shift_right, E_SHIFT_SLOT);
        assert_eq!(kp.num_chars, 60);
        assert_eq!(kp.num_slots, 60);
        // 、のみ固定、。は自由
        assert!(is_fixed(TOUTEN_ID, kp));
        assert!(!is_fixed(KUTEN_ID, kp));
        // 、は2打鍵（E+Enter）、。は通常文字
        assert!(punct_needs_enter(TOUTEN_ID, kp.size));
        assert!(!punct_needs_enter(KUTEN_ID, kp.size));
    }

    #[test]
    fn test_initial_layout_single_shift() {
        let kp = KeyboardParams::k3x10_single_shift();
        let layout = Layout::initial(kp);
        // 、が単一シフト位置 slot2 に配置されている
        assert_eq!(layout.char_to_slot[TOUTEN_ID as usize], E_SHIFT_SLOT);
        assert_eq!(layout.slot_to_char[E_SHIFT_SLOT as usize], TOUTEN_ID);
        // 60文字すべてが一意のスロットに配置されている
        let mut slots_used: std::collections::HashSet<u8> = std::collections::HashSet::new();
        for c in 0..60u8 {
            let s = layout.char_to_slot[c as usize];
            assert!(slots_used.insert(s), "duplicate slot for char {c}");
            assert_eq!(layout.slot_to_char[s as usize], c);
        }
        // 、は2打鍵
        assert_eq!(layout.char_stroke_count(TOUTEN_ID), 2);
    }

    #[test]
    fn test_initial_layout_3x11() {
        let kp = KeyboardParams::k3x11();
        let layout = Layout::initial(kp);
        for c in 0..64u8 {
            let s = layout.char_to_slot[c as usize];
            assert_eq!(
                layout.slot_to_char[s as usize], c,
                "slot_to_char mismatch for char {c}"
            );
        }
        assert_eq!(layout.slot_to_char[13], SHIFT_SLOT_SENTINEL);
        assert_eq!(layout.slot_to_char[18], SHIFT_SLOT_SENTINEL);
        let mut slots_used: std::collections::HashSet<u8> = std::collections::HashSet::new();
        for c in 0..64u8 {
            assert!(
                slots_used.insert(layout.char_to_slot[c as usize]),
                "duplicate slot for char {c}"
            );
        }
    }
}

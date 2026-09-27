// chars.rs — 月配列改変版の文字定義

use std::collections::HashMap;

pub type CharId = u8;

/// 配列サイズの上限。
///
/// CharId空間はハイブリッド拗音方式のために領域分割される:
///   [0..62)          基底かな（実文字。3x11では「」を含む）
///   [62..64)         L1/L2 の void（空きスロット代替、表示用 '□'）
///   [64..64+npl)     拗音面（第3層）。前半が子音（Ky, Gy, Sh, ...）、
///                    残りが拗音面の void。npl = num_slots_per_layer（30 or 33）。
///
/// 拗音面は「子音 → void」の順に詰めて採番されるため境界は動的で、
/// `KeyboardParams::is_consonant` / `is_yoon_void` が唯一の判定元となる。
/// 到達しうる最大IDは 3x11 の 64+33-1 = 96、よって MAX_CHARS = 97。
///
/// `mode = "none"`（既存動作）では 0..64 のみ使用し、64以上は生成されない。
/// u128 dirty mask に収めるため MAX_CHARS <= 128 を要求する（search.rs で静的検証）。
pub const MAX_CHARS: usize = 97;

/// 基底かなの定義（インデックス = 初期スロット番号）
///
/// [0..60]  3x10 / 3x11 共通文字
/// [60..62] 3x11 追加文字（「」）
/// [62..64] 3x11 空きスロット代替（void、表示用 '□'）
///
/// Layer 1 (0-29):
///   Row 0: そ こ し て ょ つ ん い の り
///   Row 1: は か 、 と た く う 。 ゛ き
///   Row 2: す け に な さ っ る ち れ ゜
///
/// Layer 2 (30-59):
///   Row 0: ぁ ひ ほ ふ め ぬ え み や ぇ
///   Row 1: ぃ を ら あ よ ま お も わ ゆ
///   Row 2: ぅ へ せ ゅ ゃ む ろ ね ー ぉ
const CHAR_LIST_BASE: [char; 64] = [
    // Layer 1
    'そ', 'こ', 'し', 'て', 'ょ', 'つ', 'ん', 'い', 'の', 'り', //  0- 9  row0
    'は', 'か', '、', 'と', 'た', 'く', 'う', '。', '゛', 'き', // 10-19  row1
    'す', 'け', 'に', 'な', 'さ', 'っ', 'る', 'ち', 'れ', '゜', // 20-29  row2
    // Layer 2
    'ぁ', 'ひ', 'ほ', 'ふ', 'め', 'ぬ', 'え', 'み', 'や', 'ぇ', // 30-39  row0
    'ぃ', 'を', 'ら', 'あ', 'よ', 'ま', 'お', 'も', 'わ', 'ゆ', // 40-49  row1
    'ぅ', 'へ', 'せ', 'ゅ', 'ゃ', 'む', 'ろ', 'ね', 'ー', 'ぉ', // 50-59  row2
    // 3x11 追加
    '「', '」', // 60-61  カギ括弧
    '□', '□', // 62-63  void（空きスロット代替）
];

/// 全 CharId → 表示文字。基底かな以外（子音・拗音void）は '□' プレースホルダ。
/// 子音の表示ラベルは別途 `consonant_label` を用いる（'□' は仮）。
pub const CHAR_LIST: [char; MAX_CHARS] = build_char_list();

const fn build_char_list() -> [char; MAX_CHARS] {
    let mut arr = ['□'; MAX_CHARS];
    let mut i = 0;
    while i < CHAR_LIST_BASE.len() {
        arr[i] = CHAR_LIST_BASE[i];
        i += 1;
    }
    arr
}

/// 読点「、」のCharId（3x10ではDスロット固定）
pub const TOUTEN_ID: CharId = 12;
/// 句点「。」のCharId（3x10ではKスロット固定）
pub const KUTEN_ID: CharId = 17;
/// 濁点「゛」のCharId（L1固定・L1内移動可）
pub const DAKUTEN_ID: CharId = 18;
/// 半濁点「゜」のCharId（L1固定・L1内移動可）
pub const HANDAKUTEN_ID: CharId = 29;
/// 拗音シフト「ゃ」のCharId（hybrid では L1固定・L1内移動可）
pub const YA_ID: CharId = 54;
/// 拗音シフト「ゅ」のCharId（hybrid では L1固定・L1内移動可）
pub const YU_ID: CharId = 53;
/// 拗音シフト「ょ」のCharId（hybrid では L1固定・L1内移動可）
pub const YO_ID: CharId = 4;
/// 拗音シフト ゃゅょ の CharId（`yoon::YOON_SHIFT_CHARS` と同順）
pub const YOON_SHIFT_IDS: [CharId; 3] = [YA_ID, YU_ID, YO_ID];
/// L1/L2 void文字の最初のID（62, 63 は空きスロット代替）
pub const VOID_CHAR_FIRST: CharId = 62;
/// 拗音面（子音 + 拗音void）の最初のID。
pub const CONSONANT_FIRST: CharId = 64;

/// c が L1/L2 の void（空きスロット代替 '□'）か。
///
/// 拗音面の文字（>= CONSONANT_FIRST）は含まない。「基底かな以外」を判定したい場合は
/// `!is_base_kana(c)` を使うこと。
#[inline]
pub fn is_l1l2_void(c: CharId) -> bool {
    (VOID_CHAR_FIRST..CONSONANT_FIRST).contains(&c)
}

/// c が実在する基底かな（L1/L2 の void も拗音面も含まない）か。
#[inline]
pub fn is_base_kana(c: CharId) -> bool {
    c < VOID_CHAR_FIRST
}

/// c が拗音シフトかな（ゃゅょ）か。
#[inline]
pub fn is_yoon_shift_id(c: CharId) -> bool {
    c == YA_ID || c == YU_ID || c == YO_ID
}

/// char → CharId のルックアップテーブルを構築
/// void文字（'□'、インデックス62-63）はマップに含めない
pub fn build_char_to_id() -> HashMap<char, CharId> {
    CHAR_LIST
        .iter()
        .enumerate()
        .take(62) // 実文字のみ（void除外）
        .map(|(i, &c)| (c, i as CharId))
        .collect()
}

/// コーパスの1文字を CharId のシーケンスにデコンポーズする。
/// 有声音（が→か+゛）、半濁音（ぱ→は+゜）を展開する。
/// 未知文字は空スライスを返す。
pub fn decompose(c: char, map: &HashMap<char, CharId>) -> ArrayVec2 {
    if let Some(&id) = map.get(&c) {
        return ArrayVec2::one(id);
    }
    // 有声・半濁音テーブル
    static VOICED: &[(char, char, char)] = &[
        ('が', 'か', '゛'),
        ('ぎ', 'き', '゛'),
        ('ぐ', 'く', '゛'),
        ('げ', 'け', '゛'),
        ('ご', 'こ', '゛'),
        ('ざ', 'さ', '゛'),
        ('じ', 'し', '゛'),
        ('ず', 'す', '゛'),
        ('ぜ', 'せ', '゛'),
        ('ぞ', 'そ', '゛'),
        ('だ', 'た', '゛'),
        ('ぢ', 'ち', '゛'),
        ('づ', 'つ', '゛'),
        ('で', 'て', '゛'),
        ('ど', 'と', '゛'),
        ('ば', 'は', '゛'),
        ('び', 'ひ', '゛'),
        ('ぶ', 'ふ', '゛'),
        ('べ', 'へ', '゛'),
        ('ぼ', 'ほ', '゛'),
        ('ぱ', 'は', '゜'),
        ('ぴ', 'ひ', '゜'),
        ('ぷ', 'ふ', '゜'),
        ('ぺ', 'へ', '゜'),
        ('ぽ', 'ほ', '゜'),
        ('ゔ', 'う', '゛'),
    ];
    if let Some(&(_, base, diac)) = VOICED.iter().find(|&&(v, _, _)| v == c) {
        if let (Some(&bid), Some(&did)) = (map.get(&base), map.get(&diac)) {
            return ArrayVec2::two(bid, did);
        }
    }
    ArrayVec2::empty()
}

/// ヒープアロケーションなしの小容量Vec（最大2要素）
#[derive(Clone, Copy, Default)]
pub struct ArrayVec2 {
    data: [CharId; 2],
    len: u8,
}

impl ArrayVec2 {
    pub fn empty() -> Self {
        Self {
            data: [0; 2],
            len: 0,
        }
    }
    pub fn one(a: CharId) -> Self {
        Self {
            data: [a, 0],
            len: 1,
        }
    }
    pub fn two(a: CharId, b: CharId) -> Self {
        Self {
            data: [a, b],
            len: 2,
        }
    }
    pub fn as_slice(&self) -> &[CharId] {
        &self.data[..self.len as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_char_to_id() {
        let map = build_char_to_id();
        assert_eq!(map.len(), 62); // 実文字のみ (void除外)
        assert_eq!(map[&'そ'], 0);
        assert_eq!(map[&'゛'], DAKUTEN_ID);
        assert_eq!(map[&'゜'], HANDAKUTEN_ID);
    }

    #[test]
    fn test_decompose_plain() {
        let map = build_char_to_id();
        let result = decompose('あ', &map);
        assert_eq!(result.as_slice().len(), 1);
        assert_eq!(result.as_slice()[0], map[&'あ']);
    }

    #[test]
    fn test_decompose_voiced() {
        let map = build_char_to_id();
        let result = decompose('が', &map);
        assert_eq!(result.as_slice().len(), 2);
        assert_eq!(result.as_slice()[0], map[&'か']);
        assert_eq!(result.as_slice()[1], DAKUTEN_ID);
    }

    #[test]
    fn test_decompose_unknown() {
        let map = build_char_to_id();
        let result = decompose('A', &map);
        assert_eq!(result.as_slice().len(), 0);
    }
}

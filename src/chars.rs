// chars.rs — 月配列改変版の文字定義

use std::collections::HashMap;

pub type CharId = u8;

/// 3x10モードの文字数（参照用定数）
#[allow(dead_code)]
pub const NUM_CHARS: usize = 60;
/// 配列サイズの上限（3x11モード: 62実文字 + 2 void = 64）
pub const MAX_CHARS: usize = 64;

/// 全文字の定義（インデックス = 初期スロット番号）
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
pub const CHAR_LIST: [char; MAX_CHARS] = [
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

/// 読点「、」のCharId（3x10ではDスロット固定）
pub const TOUTEN_ID: CharId = 12;
/// 句点「。」のCharId（3x10ではKスロット固定）
pub const KUTEN_ID: CharId = 17;
/// 濁点「゛」のCharId（L1固定・L1内移動可）
pub const DAKUTEN_ID: CharId = 18;
/// 半濁点「゜」のCharId（L1固定・L1内移動可）
pub const HANDAKUTEN_ID: CharId = 29;
/// 開きカギ括弧「「」のCharId（3x11のみ）
#[allow(dead_code)]
pub const KAGIKAKO_OPEN_ID: CharId = 60;
/// 閉じカギ括弧「」」のCharId（3x11のみ）
#[allow(dead_code)]
pub const KAGIKAKO_CLOSE_ID: CharId = 61;
/// void文字の最初のID（62, 63 は空きスロット代替）
pub const VOID_CHAR_FIRST: CharId = 62;

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

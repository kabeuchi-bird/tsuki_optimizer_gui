// yoon.rs — ハイブリッド拗音方式の定義
//
// 拗音面（子音面）は L1/L2 に続く第3層。後置シフト（ゃゅょ）で到達する。
// 拗音ユニット = 子音キー1打 + 拗音シフト（ゃゅょ）1打 = 常に2打。
//
// このモジュールは方式のモード定義を提供する。子音テーブル・コーパス分解・
// スロットモデルは後続ステージで追加される。

/// 拗音方式のモード。
///
/// - `None`: 既存動作。拗音は独立した小書き文字として扱う（第3層なし）。
/// - `Hybrid`: 子音面を追加し、拗音を「子音 + ゃ/ゅ/ょ」の2打に分解する。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum YoonMode {
    #[default]
    None,
    Hybrid,
}

impl YoonMode {
    /// 設定文字列（config.toml / CLI）から解釈する。不明値は None にフォールバック。
    pub fn from_config_str(s: &str) -> Self {
        match s {
            "none" => Self::None,
            "hybrid" => Self::Hybrid,
            other => {
                eprintln!("警告: 不明な yoon.mode '{}' → none を使用します", other);
                Self::None
            }
        }
    }

    /// 設定・ログ表示用の文字列。
    pub fn config_label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Hybrid => "hybrid",
        }
    }

    /// この方式が拗音面（第3層）を持つか。
    pub fn is_hybrid(&self) -> bool {
        matches!(self, Self::Hybrid)
    }
}

use std::collections::HashMap;

use crate::chars::{build_char_to_id, CharId, CONSONANT_FIRST};

/// 拗音シフトキー（後置シフト）の小書きかな。ゃ ゅ ょ の3キーのみ。
pub const YOON_SHIFT_CHARS: [char; 3] = ['ゃ', 'ゅ', 'ょ'];

/// デフォルト子音セット（必須11種、Dy を除く）。
pub const DEFAULT_CONSONANTS: &str = "KyGyShJChNyHyByPyMyRy";

/// 子音レジストリの1エントリ（正準順）。
///
/// 各子音は `base` + {ゃ,ゅ,ょ} の拗音を表す。`base` は原文に現れる
/// 生のかな（濁音・半濁音は合成済みの単一コードポイント）。
struct ConsonantDef {
    token: &'static str,
    base: char,
}

/// ゃゅょ拗音の子音レジストリ（正準順）。CharId はこの順で 64 から詰めて割り当てる。
/// 外来音（F/V/W/T/D）は ぁぃぅぇぉ のシフトキー化を要するため v1 では非対応。
const CONSONANT_REGISTRY: [ConsonantDef; 12] = [
    ConsonantDef { token: "Ky", base: 'き' },
    ConsonantDef { token: "Gy", base: 'ぎ' },
    ConsonantDef { token: "Sh", base: 'し' },
    ConsonantDef { token: "J", base: 'じ' },
    ConsonantDef { token: "Ch", base: 'ち' },
    ConsonantDef { token: "Dy", base: 'ぢ' },
    ConsonantDef { token: "Ny", base: 'に' },
    ConsonantDef { token: "Hy", base: 'ひ' },
    ConsonantDef { token: "By", base: 'び' },
    ConsonantDef { token: "Py", base: 'ぴ' },
    ConsonantDef { token: "My", base: 'み' },
    ConsonantDef { token: "Ry", base: 'り' },
];

/// 有効化された子音1つ。
#[derive(Clone, Copy, Debug)]
pub struct ActiveConsonant {
    /// 表示・設定用トークン（例 "Ky"）。
    pub token: &'static str,
    /// 分解の基底かな（例 'き'）。
    pub base: char,
    /// 割り当てられた CharId（CONSONANT_FIRST 以上）。
    pub id: CharId,
}

/// 拗音方式の子音テーブル。有効子音と、原文分解用の (基底かな, 小書きかな) → 子音CharId マップを保持する。
#[derive(Clone, Debug)]
pub struct YoonTable {
    consonants: Vec<ActiveConsonant>,
    /// (base_char, small_char) → 子音 CharId
    unit_map: HashMap<(char, char), CharId>,
    /// 小書きかな char → CharId（ゃゅょ）
    shift_ids: HashMap<char, CharId>,
}

impl YoonTable {
    /// 子音セット文字列（例 "KyGyShJChNyHyByPyMyRy"）を解釈してテーブルを構築する。
    ///
    /// トークンは最長一致で分割する（2文字トークン優先、1文字は "J" のみ）。
    /// 未知トークン・重複・v1非対応の外来音は Err を返す。
    pub fn from_spec(spec: &str) -> Result<Self, String> {
        let chars: Vec<char> = spec.chars().collect();
        let mut active_tokens: Vec<&'static str> = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let two: Option<String> = if i + 1 < chars.len() {
                Some(format!("{}{}", chars[i], chars[i + 1]))
            } else {
                None
            };
            let one = chars[i].to_string();

            let matched = two
                .as_deref()
                .and_then(registry_token)
                .map(|t| (t, 2))
                .or_else(|| registry_token(&one).map(|t| (t, 1)));

            match matched {
                Some((tok, adv)) => {
                    if active_tokens.contains(&tok) {
                        return Err(format!("子音セットにトークン '{}' が重複しています", tok));
                    }
                    active_tokens.push(tok);
                    i += adv;
                }
                None => {
                    return Err(format!(
                        "子音セットの解釈に失敗しました（位置 {} 付近: '{}'）。\
                         有効なトークン: Ky Gy Sh J Ch Dy Ny Hy By Py My Ry。\
                         外来音 F/V/W/T/D は v1 では非対応です。",
                        i, chars[i]
                    ));
                }
            }
        }

        Self::from_tokens(&active_tokens)
    }

    /// 正準順（レジストリ順）に子音 CharId を詰めて割り当ててテーブルを構築する。
    fn from_tokens(active_tokens: &[&str]) -> Result<Self, String> {
        let map = build_char_to_id();
        let mut consonants = Vec::new();
        let mut unit_map = HashMap::new();
        let mut next_id = CONSONANT_FIRST;

        // レジストリ順に走査 → 有効なものだけ dense に採番（spec の並び順に依存しない）
        for def in &CONSONANT_REGISTRY {
            if !active_tokens.contains(&def.token) {
                continue;
            }
            let id = next_id;
            next_id += 1;
            consonants.push(ActiveConsonant {
                token: def.token,
                base: def.base,
                id,
            });
            for &small in &YOON_SHIFT_CHARS {
                unit_map.insert((def.base, small), id);
            }
        }

        if consonants.is_empty() {
            return Err("子音セットが空です".to_string());
        }

        let shift_ids: HashMap<char, CharId> = YOON_SHIFT_CHARS
            .iter()
            .map(|&c| {
                let id = *map
                    .get(&c)
                    .expect("拗音シフトかな（ゃゅょ）は CHAR_LIST に存在する");
                (c, id)
            })
            .collect();

        Ok(YoonTable {
            consonants,
            unit_map,
            shift_ids,
        })
    }

    /// 有効子音のスライス（CharId 昇順 = レジストリ順）。
    pub fn consonants(&self) -> &[ActiveConsonant] {
        &self.consonants
    }

    /// 有効子音数。
    pub fn num_consonants(&self) -> usize {
        self.consonants.len()
    }

    /// (基底かな, 小書きかな) が拗音ユニットなら子音 CharId を返す。
    pub fn lookup_unit(&self, base: char, small: char) -> Option<CharId> {
        self.unit_map.get(&(base, small)).copied()
    }

    /// 小書きかな（ゃゅょ）の CharId を返す。
    pub fn shift_id(&self, small: char) -> Option<CharId> {
        self.shift_ids.get(&small).copied()
    }

    /// c が拗音シフトかな（ゃゅょ）か。
    pub fn is_shift_char(c: char) -> bool {
        YOON_SHIFT_CHARS.contains(&c)
    }
}

/// トークン文字列がレジストリに存在すれば正準の &'static str を返す。
fn registry_token(tok: &str) -> Option<&'static str> {
    CONSONANT_REGISTRY
        .iter()
        .find(|d| d.token == tok)
        .map(|d| d.token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_from_config_str() {
        assert_eq!(YoonMode::from_config_str("none"), YoonMode::None);
        assert_eq!(YoonMode::from_config_str("hybrid"), YoonMode::Hybrid);
        assert_eq!(YoonMode::from_config_str("xxx"), YoonMode::None);
        assert!(YoonMode::Hybrid.is_hybrid());
        assert!(!YoonMode::None.is_hybrid());
    }

    #[test]
    fn test_default_consonants_dense_ids() {
        let yt = YoonTable::from_spec(DEFAULT_CONSONANTS).unwrap();
        assert_eq!(yt.num_consonants(), 11); // 必須11種（Dy除く）
        // CONSONANT_FIRST から連番、レジストリ順
        assert_eq!(yt.consonants()[0].token, "Ky");
        assert_eq!(yt.consonants()[0].id, CONSONANT_FIRST);
        assert_eq!(yt.lookup_unit('き', 'ゃ'), Some(CONSONANT_FIRST));
        assert_eq!(yt.lookup_unit('き', 'ゅ'), Some(CONSONANT_FIRST));
        assert_eq!(yt.lookup_unit('き', 'ょ'), Some(CONSONANT_FIRST));
        // Dy はデフォルトに含まれない
        assert_eq!(yt.lookup_unit('ぢ', 'ゃ'), None);
    }

    #[test]
    fn test_optional_dy_registry_order() {
        // spec の並びに依らずレジストリ順に dense 採番される（Ky→Dy の順）
        let yt = YoonTable::from_spec("DyKy").unwrap();
        assert_eq!(yt.num_consonants(), 2);
        assert_eq!(yt.lookup_unit('き', 'ゃ'), Some(CONSONANT_FIRST)); // Ky が先
        assert_eq!(yt.lookup_unit('ぢ', 'ょ'), Some(CONSONANT_FIRST + 1)); // Dy が後
    }

    #[test]
    fn test_longest_match_j_and_ch() {
        // "JCh" → J(1文字) + Ch(2文字)
        let yt = YoonTable::from_spec("JCh").unwrap();
        assert_eq!(yt.num_consonants(), 2);
        assert!(yt.lookup_unit('じ', 'ゅ').is_some());
        assert!(yt.lookup_unit('ち', 'ゃ').is_some());
    }

    #[test]
    fn test_parse_errors() {
        assert!(YoonTable::from_spec("KyKy").is_err()); // 重複
        assert!(YoonTable::from_spec("F").is_err()); // 外来音（v1非対応）
        assert!(YoonTable::from_spec("Xy").is_err()); // 未知
        assert!(YoonTable::from_spec("").is_err()); // 空
    }
}

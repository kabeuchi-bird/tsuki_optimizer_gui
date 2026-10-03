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
}

use std::collections::HashMap;

use crate::chars::{CharId, CONSONANT_FIRST};

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

/// 拗音方式の子音テーブル。
///
/// 有効子音の集合は `registry_mask`（レジストリのビットマスク）が唯一の表現で、
/// CharId はレジストリ順に `CONSONANT_FIRST` から詰めて採番される。
/// `unit_map` は分解用の 基底かな → 子音CharId の逆引き。
#[derive(Clone, Debug)]
pub struct YoonTable {
    /// 基底かな（き/ぎ/し…）→ 子音 CharId
    unit_map: HashMap<char, CharId>,
    /// レジストリの有効ビットマスク（bit i = CONSONANT_REGISTRY[i] が有効）
    registry_mask: u16,
}

impl YoonTable {
    /// 子音セット文字列（例 "KyGyShJChNyHyByPyMyRy"）を解釈してテーブルを構築する。
    ///
    /// トークンは最長一致で分割する（2文字トークン優先、1文字は "J" のみ）。
    /// 未知トークン・重複・v1非対応の外来音は Err を返す。
    pub fn from_spec(spec: &str) -> Result<Self, String> {
        let mut registry_mask: u16 = 0;
        let mut rest = spec;
        while !rest.is_empty() {
            // 長いトークンを優先して前方一致（"JCh" → J + Ch）
            let hit = CONSONANT_REGISTRY
                .iter()
                .enumerate()
                .filter(|(_, d)| rest.starts_with(d.token))
                .max_by_key(|(_, d)| d.token.len());
            match hit {
                Some((ri, def)) => {
                    let bit = 1u16 << ri;
                    if registry_mask & bit != 0 {
                        return Err(format!(
                            "子音セットにトークン '{}' が重複しています",
                            def.token
                        ));
                    }
                    registry_mask |= bit;
                    rest = &rest[def.token.len()..];
                }
                None => {
                    return Err(format!(
                        "子音セットの解釈に失敗しました（'{}' 付近）。\
                         有効なトークン: Ky Gy Sh J Ch Dy Ny Hy By Py My Ry。\
                         外来音 F/V/W/T/D は v1 では非対応です。",
                        rest
                    ));
                }
            }
        }
        Self::from_mask(registry_mask)
    }

    /// レジストリマスクからテーブルを構築する（CharId はレジストリ順に dense 採番）。
    fn from_mask(registry_mask: u16) -> Result<Self, String> {
        if registry_mask == 0 {
            return Err("子音セットが空です".to_string());
        }
        let mut unit_map = HashMap::new();
        let mut next_id = CONSONANT_FIRST;
        for (ri, def) in CONSONANT_REGISTRY.iter().enumerate() {
            if registry_mask & (1 << ri) == 0 {
                continue;
            }
            unit_map.insert(def.base, next_id);
            next_id += 1;
        }
        Ok(YoonTable {
            unit_map,
            registry_mask,
        })
    }

    /// レジストリ有効ビットマスク（KeyboardParams に載せて表示ラベル復元に使う）。
    pub fn registry_mask(&self) -> u16 {
        self.registry_mask
    }

    /// 有効子音数。
    pub fn num_consonants(&self) -> usize {
        self.registry_mask.count_ones() as usize
    }

    /// (基底かな, 小書きかな) が拗音ユニットなら子音 CharId を返す。
    ///
    /// 子音はゃゅょのどれと組んでも同じなので、判定は `small` が拗音シフトかどうかと
    /// `base` が有効子音の基底かどうかの2点。
    pub fn lookup_unit(&self, base: char, small: char) -> Option<CharId> {
        if !Self::is_shift_char(small) {
            return None;
        }
        self.unit_map.get(&base).copied()
    }

    /// c が拗音シフトかな（ゃゅょ）か。
    pub fn is_shift_char(c: char) -> bool {
        YOON_SHIFT_CHARS.contains(&c)
    }
}

/// 拗音シフトかな（ゃゅょ）の CharId を返す。
///
/// `YOON_SHIFT_CHARS` と `chars::YOON_SHIFT_IDS` は同順であることが前提。
pub fn yoon_shift_id(c: char) -> Option<CharId> {
    YOON_SHIFT_CHARS
        .iter()
        .position(|&s| s == c)
        .map(|i| crate::chars::YOON_SHIFT_IDS[i])
}

/// 拗音方式の初期化結果。
///
/// hybrid を有効にするには「拗音面つき KeyboardParams」「同じテーブルで分解した
/// コーパス」「ゃゅょ を含む l1_only」の3点が揃っている必要があり、どれか1つでも
/// 欠けると無言で壊れる（子音の頻度が全て0になる等）。それらを取り違えられない
/// よう、この構造体が一括で提供する。
pub struct YoonSetup {
    /// 拗音面を反映した KeyboardParams（none ならそのまま）
    pub kp: crate::layout::KeyboardParams,
    /// 子音テーブル（none なら None。hybrid か否かもこれで判定する）。`Corpus::from_file_with_yoon` に渡すこと。
    pub table: Option<YoonTable>,
}

impl YoonSetup {
    /// 拗音方式を解決し、KeyboardParams に拗音面を反映する。
    ///
    /// `mode` は CLI/TOML で解決済みの値、`consonants` は設定の子音セット
    /// （None ならデフォルト11種）。CLI と GUI の両方がこの1関数を呼ぶ。
    pub fn resolve(
        kp: crate::layout::KeyboardParams,
        mode: YoonMode,
        consonants: Option<&str>,
    ) -> Result<Self, String> {
        if mode == YoonMode::None {
            return Ok(YoonSetup { kp, table: None });
        }
        let table = YoonTable::from_spec(consonants.unwrap_or(DEFAULT_CONSONANTS))
            .map_err(|e| format!("子音セットが不正です: {e}"))?;
        let kp = kp
            .with_yoon(table.registry_mask())
            .map_err(|e| format!("拗音面を構成できません: {e}"))?;
        Ok(YoonSetup {
            kp,
            table: Some(table),
        })
    }

    /// 拗音シフト ゃゅょ を L1 固定集合に加える（hybrid のみ）。
    ///
    /// ゃゅょ が1打で打てなければ方式が成立しないため、hybrid では必須。
    pub fn extend_l1_only(&self, l1_only: &mut std::collections::HashSet<CharId>) {
        if self.table.is_some() {
            l1_only.extend(crate::chars::YOON_SHIFT_IDS);
        }
    }
}

/// レジストリマスクと子音 CharId から表示ラベル（"Ky" 等）を復元する。
///
/// 子音 CharId は CONSONANT_FIRST から有効子音（レジストリ順）に dense 採番されるので、
/// `id - CONSONANT_FIRST` 番目に立っているビットが対応するレジストリ項目。
pub fn consonant_label(registry_mask: u16, id: CharId) -> Option<&'static str> {
    if id < CONSONANT_FIRST {
        return None;
    }
    let mut rank = (id - CONSONANT_FIRST) as u32;
    for (ri, def) in CONSONANT_REGISTRY.iter().enumerate() {
        if registry_mask & (1 << ri) != 0 {
            if rank == 0 {
                return Some(def.token);
            }
            rank -= 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_from_config_str() {
        assert_eq!(YoonMode::from_config_str("none"), YoonMode::None);
        assert_eq!(YoonMode::from_config_str("hybrid"), YoonMode::Hybrid);
        assert_eq!(YoonMode::from_config_str("xxx"), YoonMode::None);
    }

    #[test]
    fn test_default_consonants_dense_ids() {
        let yt = YoonTable::from_spec(DEFAULT_CONSONANTS).unwrap();
        assert_eq!(yt.num_consonants(), 11); // 必須11種（Dy除く）
        // CONSONANT_FIRST から連番、レジストリ順
        assert_eq!(
            consonant_label(yt.registry_mask(), CONSONANT_FIRST),
            Some("Ky")
        );
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

// config.rs — TOMLベース設定ファイルの読み込みと構造体への変換

use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

use crate::chars::{self, CharId, MAX_CHARS};
use crate::cost::Weights;
use crate::layout::{ExclusivePair, KeyboardParams, KeyboardSize};
use crate::search::{InitialLayoutMode, SearchConfig};
use crate::yoon::YoonMode;

// ──────────────────────────────────────
// TOMLファイルのトップレベル構造
// ──────────────────────────────────────
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub run: RunConfig,
    #[serde(default)]
    pub weights: WeightsConfig,
    #[serde(default)]
    pub slot_difficulty: SlotDifficultyConfig,
    #[serde(default)]
    pub constraints: ConstraintsConfig,
    #[serde(default)]
    pub yoon: YoonConfig,
}

// ──────────────────────────────────────
// [run] セクション
// ──────────────────────────────────────
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    pub corpus: Option<String>,
    pub seed: Option<u64>,
    pub max_iter: Option<usize>,
    pub restart_after: Option<usize>,
    pub max_restarts: Option<usize>,
    /// タブーテニュア（近傍サイズ比）。`[run.tabu_ratio]` サブテーブル。
    #[serde(default)]
    pub tabu_ratio: TabuRatioConfig,

    /// 廃止済み（絶対手数指定）。`deny_unknown_fields` で古い設定が即エラーに
    /// ならないよう受け取るだけで、値は使わず警告する。
    pub tabu_l1: Option<usize>,
    pub tabu_l2: Option<usize>,
    pub tabu_inter: Option<usize>,
    pub tabu_yoon: Option<usize>,
    pub inter_sample: Option<usize>,
    pub ab_sample_limit: Option<usize>,
    pub log_interval: Option<usize>,
    pub perturbation_swaps: Option<usize>,
    /// 多様化の開始閾値。旧名 `tenure_grow_threshold` も受け付ける。
    #[serde(alias = "tenure_grow_threshold")]
    pub diversify_threshold: Option<f64>,
    /// 廃止済み（テニュア拡大）。受け取るだけで、値は使わず警告する。
    pub tenure_grow_interval: Option<usize>,
    pub tenure_max_scale: Option<f64>,
    /// 多様化（頻度ベース長期記憶）の強さ。0.0 で無効。
    pub diversification: Option<f64>,

    /// キーボードサイズ: "3x10"（デフォルト）/ "3x10_single_shift" / "3x11"
    pub keyboard_size: Option<String>,
    /// 初期配列モード: "2-263"（デフォルト）または "random"
    pub initial_layout: Option<String>,
}

/// [run.tabu_ratio] サブテーブル。キー名から `OpKind` への対応をここ1か所で持つ。
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TabuRatioConfig {
    pub l1: Option<f64>,
    pub l2: Option<f64>,
    pub inter: Option<f64>,
    pub yoon: Option<f64>,
}

// ──────────────────────────────────────
// [weights] セクション
// ──────────────────────────────────────
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WeightsConfig {
    pub stroke_scale: Option<f64>,
    pub same_finger_penalty: Option<f64>,
    pub same_key_penalty: Option<f64>,
    pub upper_lower_jump: Option<f64>,
    pub same_hand_base: Option<f64>,
    pub alternation_bonus: Option<f64>,
    pub outroll_bonus_2gram: Option<f64>,
    pub inroll_bonus_2gram: Option<f64>,
    pub quasi_alt_bonus: Option<f64>,
    pub outroll_bonus_3gram: Option<f64>,
    pub inroll_bonus_3gram: Option<f64>,
    pub allow_index_roll: Option<bool>,
}

// ──────────────────────────────────────
// [slot_difficulty] セクション
//
// 各行（row0/row1/row2）を Vec<f64> で指定する。
// 3x10 の場合は 10 要素、3x11 の場合は 11 要素。
// 要素数が不足する場合はデフォルト値で補完する。
// ──────────────────────────────────────
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SlotDifficultyConfig {
    pub row0: Option<Vec<f64>>,
    pub row1: Option<Vec<f64>>,
    pub row2: Option<Vec<f64>>,
}

// ──────────────────────────────────────
// ファイルからの読み込み
// ──────────────────────────────────────
impl Config {
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("設定ファイル読み込みエラー: {}", e))?;
        toml::from_str(&text).map_err(|e| format!("設定ファイルのパースエラー: {}", e))
    }

    /// 設定ファイルを読み込む。ファイルが無ければデフォルト値を返す（CLI / GUI 共通）
    pub fn load_or_default(path: &Path) -> Result<Self, String> {
        if path.exists() {
            Self::from_file(path)
        } else {
            Ok(Self::default())
        }
    }

    /// keyboard_size 設定から KeyboardParams を生成する
    pub fn build_keyboard_params(&self) -> KeyboardParams {
        match self.run.keyboard_size.as_deref() {
            Some(s) => keyboard_params_from_str(s),
            None => KeyboardParams::k3x10(),
        }
    }

    /// yoon.mode 設定から YoonMode を生成する（未指定は None）
    pub fn build_yoon_mode(&self) -> YoonMode {
        self.yoon.mode.as_deref().map_or(YoonMode::None, YoonMode::from_config_str)
    }

    /// 設定ファイル固有の検証（`SearchConfig::validate` と同じ出力先へ書く）。
    ///
    /// `build_search_config` は副作用を持たせず、警告はこちらに集める。
    pub fn validate(&self, out: &mut impl std::io::Write) {
        let r = &self.run;
        const TABU: &str = "（絶対手数指定）→ 無視します。近傍サイズ比で指定する [run.tabu_ratio] を使ってください";
        const TENURE: &str = "（テニュア拡大は多様化で代替）→ 無視します";
        for (key, given, hint) in [
            ("tabu_l1", r.tabu_l1.is_some(), TABU),
            ("tabu_l2", r.tabu_l2.is_some(), TABU),
            ("tabu_inter", r.tabu_inter.is_some(), TABU),
            ("tabu_yoon", r.tabu_yoon.is_some(), TABU),
            ("tenure_grow_interval", r.tenure_grow_interval.is_some(), TENURE),
            ("tenure_max_scale", r.tenure_max_scale.is_some(), TENURE),
        ] {
            if given {
                let _ = writeln!(out, "警告: {key} は廃止されました{hint}");
            }
        }
    }

    /// デフォルト値と設定ファイルの内容をマージして SearchConfig を生成する
    pub fn build_search_config(&self) -> SearchConfig {
        let r = &self.run;
        let d = SearchConfig::default();
        SearchConfig {
            max_iter: r.max_iter.unwrap_or(d.max_iter),
            restart_after: r.restart_after.unwrap_or(d.restart_after),
            max_restarts: r.max_restarts.unwrap_or(d.max_restarts),
            tabu_ratio: {
                let t = &r.tabu_ratio;
                let mut ratio = d.tabu_ratio;
                // 並び順は OpKind の判別子（SwapL1, SwapL2, InterLayer, SwapYoon）に対応
                for (i, v) in [t.l1, t.l2, t.inter, t.yoon].into_iter().enumerate() {
                    if let Some(v) = v {
                        ratio[i] = v;
                    }
                }
                ratio
            },
            inter_sample: r.inter_sample.unwrap_or(d.inter_sample),
            ab_sample_limit: r.ab_sample_limit.unwrap_or(d.ab_sample_limit),
            log_interval: r.log_interval.unwrap_or(d.log_interval),
            perturbation_swaps: r.perturbation_swaps.unwrap_or(d.perturbation_swaps),
            diversify_threshold: r.diversify_threshold.unwrap_or(d.diversify_threshold),
            diversification: r.diversification.unwrap_or(d.diversification),
            initial_layout_mode: r
                .initial_layout
                .as_deref()
                .map_or_else(InitialLayoutMode::default, InitialLayoutMode::from_config_str),
        }
    }

    /// デフォルト値と設定ファイルの内容をマージして Weights を生成する
    /// kp は呼び出し側で build_keyboard_params() から取得して渡す
    pub fn build_weights(&self, kp: KeyboardParams) -> Weights {
        let w = &self.weights;
        let s = &self.slot_difficulty;
        let d = Weights::default();

        Weights {
            kp,
            stroke_scale: w.stroke_scale.unwrap_or(d.stroke_scale),
            same_finger_penalty: w.same_finger_penalty.unwrap_or(d.same_finger_penalty),
            same_key_penalty: w.same_key_penalty.unwrap_or(d.same_key_penalty),
            upper_lower_jump: w.upper_lower_jump.unwrap_or(d.upper_lower_jump),
            same_hand_base: w.same_hand_base.unwrap_or(d.same_hand_base),
            alternation_bonus: w.alternation_bonus.unwrap_or(d.alternation_bonus),
            outroll_bonus_2gram: w.outroll_bonus_2gram.unwrap_or(d.outroll_bonus_2gram),
            inroll_bonus_2gram: w.inroll_bonus_2gram.unwrap_or(d.inroll_bonus_2gram),
            quasi_alt_bonus: w.quasi_alt_bonus.unwrap_or(d.quasi_alt_bonus),
            outroll_bonus_3gram: w.outroll_bonus_3gram.unwrap_or(d.outroll_bonus_3gram),
            inroll_bonus_3gram: w.inroll_bonus_3gram.unwrap_or(d.inroll_bonus_3gram),
            allow_index_roll: w.allow_index_roll.unwrap_or(d.allow_index_roll),
            slot_difficulty: [
                parse_difficulty_row(s.row0.as_deref(), d.slot_difficulty[0]),
                parse_difficulty_row(s.row1.as_deref(), d.slot_difficulty[1]),
                parse_difficulty_row(s.row2.as_deref(), d.slot_difficulty[2]),
            ],
            // L2 にある文字の直後に゛が来ると -1打鍵（゛は常にL1固定なのでトリガー対象外）
            daku_l2_trigger: self
                .preset_trigger("うかきくけこさしすせそたちつてとはひふへほ", "きしちひ"),
            // L2 にある文字の直後に゜が来ると -1打鍵（対象はは行のみ）
            handaku_l2_trigger: self.preset_trigger("はひふへほ", "ひ"),
        }
    }

    pub fn corpus_path(&self, cli_override: Option<&str>) -> String {
        cli_override
            .map(|s| s.to_owned())
            .or_else(|| self.run.corpus.clone())
            .unwrap_or_else(|| "corpus.txt".to_owned())
    }

    pub fn seed(&self, cli_override: Option<u64>) -> u64 {
        cli_override.or(self.run.seed).unwrap_or_else(rand::random)
    }
}

// ──────────────────────────────────────
// [constraints] セクション
// ──────────────────────────────────────
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ConstraintsConfig {
    #[serde(default)]
    pub exclusive_pairs: Vec<ExclusivePairConfig>,
    /// プリセット名: "all-daku" または "i-daku"（未指定は None）
    pub preset: Option<String>,
    /// Layer 1 に固定する文字（省略時: "゛゜"）
    /// 空文字列 "" で全文字がレイヤー間移動可能になる
    pub l1_only: Option<String>,
}

/// [[constraints.exclusive_pairs]] の1エントリ
#[derive(Debug, Deserialize)]
pub struct ExclusivePairConfig {
    /// 制約グループA（かな文字列、例: "ゃゅょ"）
    pub group_a: String,
    /// 制約グループB（かな文字列、例: "きしちにひみり"）
    pub group_b: String,
}

// ──────────────────────────────────────
// [yoon] セクション（ハイブリッド拗音方式）
// ──────────────────────────────────────
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct YoonConfig {
    /// 拗音方式: "none"（デフォルト・既存動作）/ "hybrid"
    pub mode: Option<String>,
    /// 子音セット（トークン連結、例: "KyGyShJChNyHyByPyMyRy"）。
    /// 省略時は必須11種。後続ステージで解釈される。
    pub consonants: Option<String>,
}

// 濁音になりうる音すべて ＋ 濁点
const DAKUON_BASE: &str = "うかきくけこさしすせそたちつてとはひふへほ゛、";
// イ段のみ（きしちひ）
const DAKUON_I_ROW: &str = "きしちひ";

impl Config {
    /// 排他配置ペア設定（プリセット + 明示ペア）を ExclusivePair リストに変換する
    pub fn build_exclusive_pairs(&self) -> Vec<ExclusivePair> {
        let char_map = chars::build_char_to_id();
        let mut result = Vec::new();

        // プリセット展開
        if let Some(preset) = &self.constraints.preset {
            let daku_set: HashSet<_> = DAKUON_BASE
                .chars()
                .filter_map(|c| char_map.get(&c).copied())
                .collect();
            match preset.as_str() {
                "all-daku" => result.push(ExclusivePair {
                    group_a: daku_set.clone(),
                    group_b: daku_set,
                }),
                "i-daku" => {
                    let i_row: HashSet<_> = DAKUON_I_ROW
                        .chars()
                        .filter_map(|c| char_map.get(&c).copied())
                        .collect();
                    result.push(ExclusivePair {
                        group_a: i_row,
                        group_b: daku_set,
                    });
                }
                other => eprintln!("警告: 不明なプリセット '{}' → 無視します", other),
            }
        }

        // 明示的ペア（プリセットと併用可）
        for p in &self.constraints.exclusive_pairs {
            result.push(ExclusivePair {
                group_a: p
                    .group_a
                    .chars()
                    .filter_map(|c| char_map.get(&c).copied())
                    .collect(),
                group_b: p
                    .group_b
                    .chars()
                    .filter_map(|c| char_map.get(&c).copied())
                    .collect(),
            });
        }

        result
    }

    /// L1固定文字セットを構築する（省略時は゛゜がデフォルト）
    pub fn build_l1_only_set(&self) -> HashSet<CharId> {
        let char_map = chars::build_char_to_id();
        let text = self.constraints.l1_only.as_deref().unwrap_or("゛゜");
        text.chars()
            .filter_map(|c| char_map.get(&c).copied())
            .collect()
    }

    /// preset に応じて all_daku / i_daku の文字を true にした配列を返す（preset なしは全 false）
    fn preset_trigger(&self, all_daku: &str, i_daku: &str) -> [bool; MAX_CHARS] {
        let mut trigger = [false; MAX_CHARS];
        let target_str = match self.constraints.preset.as_deref() {
            Some("all-daku") => all_daku,
            Some("i-daku") => i_daku,
            _ => return trigger,
        };
        let char_map = chars::build_char_to_id();
        for id in target_str.chars().filter_map(|c| char_map.get(&c)) {
            trigger[*id as usize] = true;
        }
        trigger
    }
}

/// Vec<f64> から [f64; 11] に変換する。
/// 要素数が 11 未満の場合はデフォルト値で補完し、超える場合は切り捨てて警告を出す。
fn parse_difficulty_row(src: Option<&[f64]>, default: [f64; 11]) -> [f64; 11] {
    let Some(v) = src else {
        return default;
    };
    if v.len() > 11 {
        eprintln!(
            "警告: slot_difficulty の行の要素数が 11 を超えています（{}要素）→ 先頭11個を使用",
            v.len()
        );
    }
    let mut arr = default;
    for (d, s) in arr.iter_mut().zip(v) {
        *d = *s;
    }
    arr
}

/// キーボードサイズ文字列から KeyboardParams を生成する（CLI / GUI 共通）
pub fn keyboard_params_from_str(s: &str) -> KeyboardParams {
    match s {
        "3x11" => KeyboardParams::k3x11(),
        "3x10_single_shift" => KeyboardParams::k3x10_single_shift(),
        "3x10" => KeyboardParams::k3x10(),
        other => {
            eprintln!("警告: 不明な keyboard_size '{}' → 3x10 を使用します", other);
            KeyboardParams::k3x10()
        }
    }
}

/// KeyboardParams からキーボードサイズ文字列を返す（表示用）
pub fn keyboard_size_str(kp: &KeyboardParams) -> &'static str {
    match kp.size {
        KeyboardSize::K3x10 => "3x10",
        KeyboardSize::K3x10SingleShift => "3x10_single_shift",
        KeyboardSize::K3x11 => "3x11",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chars::{DAKUTEN_ID, HANDAKUTEN_ID};

    #[test]
    fn test_default_config_search() {
        let config = Config::default();
        let sc = config.build_search_config();
        assert!(sc.max_iter > 0);
        assert!(sc.restart_after > 0);
        assert!(sc.tabu_ratio.iter().all(|&r| r > 0.0));
    }

    #[test]
    fn test_build_l1_only_set_default() {
        let config = Config::default();
        let set = config.build_l1_only_set();
        assert!(set.contains(&DAKUTEN_ID));
        assert!(set.contains(&HANDAKUTEN_ID));
    }
}

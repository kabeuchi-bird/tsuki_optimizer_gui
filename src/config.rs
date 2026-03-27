// config.rs — TOMLベース設定ファイルの読み込みと構造体への変換

use std::path::Path;
use serde::Deserialize;

use crate::chars;
use crate::cost::Weights;
use crate::layout::{ExclusivePair, KeyboardParams, KeyboardSize};
use crate::search::SearchConfig;

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
    pub tabu_l1: Option<usize>,
    pub tabu_l2: Option<usize>,
    pub tabu_inter: Option<usize>,
    pub inter_sample: Option<usize>,
    pub ab_sample_limit: Option<usize>,
    pub log_interval: Option<usize>,
    pub perturbation_swaps: Option<usize>,
    pub tenure_grow_threshold: Option<f64>,
    pub tenure_grow_interval: Option<usize>,
    pub tenure_max_scale: Option<f64>,

    /// キーボードサイズ: "3x10"（デフォルト）または "3x11"
    pub keyboard_size: Option<String>,
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
    pub outroll_bonus: Option<f64>,
    pub inroll_bonus: Option<f64>,
    pub quasi_alt_bonus: Option<f64>,
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
        toml::from_str(&text)
            .map_err(|e| format!("設定ファイルのパースエラー: {}", e))
    }

    /// keyboard_size 設定から KeyboardParams を生成する
    pub fn build_keyboard_params(&self) -> KeyboardParams {
        match self.run.keyboard_size.as_deref() {
            Some("3x11") => KeyboardParams::k3x11(),
            Some("3x10") | None => KeyboardParams::k3x10(),
            Some(other) => {
                eprintln!("警告: 不明な keyboard_size '{}' → 3x10 を使用します", other);
                KeyboardParams::k3x10()
            }
        }
    }

    /// デフォルト値と設定ファイルの内容をマージして SearchConfig を生成する
    pub fn build_search_config(&self) -> SearchConfig {
        let r = &self.run;
        let d = SearchConfig::default();
        SearchConfig {
            max_iter:              r.max_iter.unwrap_or(d.max_iter),
            restart_after:         r.restart_after.unwrap_or(d.restart_after),
            max_restarts:          r.max_restarts.unwrap_or(d.max_restarts),
            tabu_l1:               r.tabu_l1.unwrap_or(d.tabu_l1),
            tabu_l2:               r.tabu_l2.unwrap_or(d.tabu_l2),
            tabu_inter:            r.tabu_inter.unwrap_or(d.tabu_inter),
            inter_sample:          r.inter_sample.unwrap_or(d.inter_sample),
            ab_sample_limit:       r.ab_sample_limit.unwrap_or(d.ab_sample_limit),
            log_interval:          r.log_interval.unwrap_or(d.log_interval),
            perturbation_swaps:    r.perturbation_swaps.unwrap_or(d.perturbation_swaps),
            tenure_grow_threshold: r.tenure_grow_threshold.unwrap_or(d.tenure_grow_threshold),
            tenure_grow_interval:  r.tenure_grow_interval.unwrap_or(d.tenure_grow_interval),
            tenure_max_scale:      r.tenure_max_scale.unwrap_or(d.tenure_max_scale),
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
            stroke_scale:        w.stroke_scale.unwrap_or(d.stroke_scale),
            same_finger_penalty: w.same_finger_penalty.unwrap_or(d.same_finger_penalty),
            same_key_penalty:    w.same_key_penalty.unwrap_or(d.same_key_penalty),
            upper_lower_jump:    w.upper_lower_jump.unwrap_or(d.upper_lower_jump),
            same_hand_base:      w.same_hand_base.unwrap_or(d.same_hand_base),
            alternation_bonus:   w.alternation_bonus.unwrap_or(d.alternation_bonus),
            outroll_bonus:       w.outroll_bonus.unwrap_or(d.outroll_bonus),
            inroll_bonus:        w.inroll_bonus.unwrap_or(d.inroll_bonus),
            quasi_alt_bonus:     w.quasi_alt_bonus.unwrap_or(d.quasi_alt_bonus),
            slot_difficulty: [
                parse_difficulty_row(s.row0.as_deref(), d.slot_difficulty[0]),
                parse_difficulty_row(s.row1.as_deref(), d.slot_difficulty[1]),
                parse_difficulty_row(s.row2.as_deref(), d.slot_difficulty[2]),
            ],
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
}

/// [[constraints.exclusive_pairs]] の1エントリ
#[derive(Debug, Deserialize)]
pub struct ExclusivePairConfig {
    /// 制約グループA（かな文字列、例: "ゃゅょ"）
    pub group_a: String,
    /// 制約グループB（かな文字列、例: "きしちにひみり"）
    pub group_b: String,
}

impl Config {
    /// 排他配置ペア設定を ExclusivePair リストに変換する
    pub fn build_exclusive_pairs(&self) -> Vec<ExclusivePair> {
        let char_map = chars::build_char_to_id();
        self.constraints.exclusive_pairs.iter().map(|p| ExclusivePair {
            group_a: p.group_a.chars()
                .filter_map(|c| char_map.get(&c).copied())
                .collect(),
            group_b: p.group_b.chars()
                .filter_map(|c| char_map.get(&c).copied())
                .collect(),
        }).collect()
    }
}

/// Vec<f64> から [f64; 11] に変換する。
/// 要素数が 11 未満の場合はデフォルト値で補完し、超える場合は切り捨てて警告を出す。
fn parse_difficulty_row(src: Option<&[f64]>, default: [f64; 11]) -> [f64; 11] {
    let Some(v) = src else { return default; };
    if v.len() > 11 {
        eprintln!("警告: slot_difficulty の行の要素数が 11 を超えています（{}要素）→ 先頭11個を使用", v.len());
    }
    let mut arr = default;
    for (i, &val) in v.iter().enumerate().take(11) {
        arr[i] = val;
    }
    arr
}

/// TOMLの keyboard_size 文字列から KeyboardSize を解析（表示用）
pub fn keyboard_size_str(kp: &KeyboardParams) -> &'static str {
    match kp.size {
        KeyboardSize::K3x10 => "3x10",
        KeyboardSize::K3x11 => "3x11",
    }
}

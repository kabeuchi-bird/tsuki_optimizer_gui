// config.rs — TOMLベース設定ファイルの読み込みと構造体への変換
//
// すべてのフィールドは Option<T> で定義する。
// 「設定ファイルで指定されたものだけ上書き、残りはデフォルト」という
// マージ方式を採用することで、最小限の記述で使えるようにする。

use std::path::Path;
use serde::Deserialize;

use crate::cost::Weights;
use crate::search::SearchConfig;

// ──────────────────────────────────────
// TOMLファイルのトップレベル構造
// ──────────────────────────────────────
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// 実行時パラメータ（CLIオプションと同等）
    #[serde(default)]
    pub run: RunConfig,

    /// 評価重みパラメータ
    #[serde(default)]
    pub weights: WeightsConfig,

    /// スロット難易度テーブル
    #[serde(default)]
    pub slot_difficulty: SlotDifficultyConfig,
}

// ──────────────────────────────────────
// [run] セクション
// ──────────────────────────────────────
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    /// コーパスファイルパス
    pub corpus: Option<String>,

    /// 乱数シード
    pub seed: Option<u64>,

    /// 最大イテレーション数
    pub max_iter: Option<usize>,

    /// 改善なしで再起動するイテレーション数
    pub restart_after: Option<usize>,

    /// 最大再起動回数
    pub max_restarts: Option<usize>,

    /// Layer 1 内スワップ タブーテニュア長
    pub tabu_l1: Option<usize>,

    /// Layer 2 内スワップ タブーテニュア長
    pub tabu_l2: Option<usize>,

    /// 層間スワップ タブーテニュア長
    pub tabu_inter: Option<usize>,

    /// 層間スワップ候補サンプリング数
    pub inter_sample: Option<usize>,

    /// 同レイヤー内スワップの全列挙上限（超えたらランダムサンプリング）
    pub ab_sample_limit: Option<usize>,

    /// 進捗ログ出力間隔（イテレーション数）
    pub log_interval: Option<usize>,
    /// 再起動時のランダム層間スワップ回数
    pub perturbation_swaps: Option<usize>,
    /// テニュア増加を開始する no_improve の割合（0.0〜1.0、restart_after との積）
    pub tenure_grow_threshold: Option<f64>,
    /// テニュア増加のインターバル（イテレーション数）
    pub tenure_grow_interval: Option<usize>,
    /// テニュアの上限倍率（初期値 × この値）
    pub tenure_max_scale: Option<f64>,
}

// ──────────────────────────────────────
// [weights] セクション
// ──────────────────────────────────────
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WeightsConfig {
    /// 打鍵数コストのスケール係数（他の評価項目より優先させるため大きく設定）
    pub stroke_scale: Option<f64>,

    /// 同指連打ペナルティ
    pub same_finger_penalty: Option<f64>,

    /// 同キー連打ペナルティ
    pub same_key_penalty: Option<f64>,

    /// 同手・上段↔下段の段跨ぎペナルティ
    pub upper_lower_jump: Option<f64>,

    /// 同手・異指の基礎コスト
    pub same_hand_base: Option<f64>,

    /// 左右交互打鍵ボーナス（スコアから引く量）
    pub alternation_bonus: Option<f64>,

    /// アウトロール（小指方向）ボーナス
    pub outroll_bonus: Option<f64>,

    /// インロール（人差し指方向）ボーナス
    pub inroll_bonus: Option<f64>,

    /// 準交互打鍵（LLR/RRL等）ボーナス（trigram単位）
    pub quasi_alt_bonus: Option<f64>,
}

// ──────────────────────────────────────
// [slot_difficulty] セクション
//
// 各行（row0/row1/row2）を10要素の配列で指定する。
// 列の順序: [左小, 左薬, 左中(D), 左人×2, 右人×2, 右中(K), 右薬, 右小]
// ──────────────────────────────────────
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SlotDifficultyConfig {
    /// 上段（行0）: 左小指〜右小指の10キー
    pub row0: Option<[f64; 10]>,
    /// 中段ホーム（行1）
    pub row1: Option<[f64; 10]>,
    /// 下段（行2）
    pub row2: Option<[f64; 10]>,
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

    /// デフォルト値と設定ファイルの内容をマージして SearchConfig を生成する
    pub fn build_search_config(&self) -> SearchConfig {
        let r = &self.run;
        let d = SearchConfig::default();
        SearchConfig {
            max_iter:        r.max_iter.unwrap_or(d.max_iter),
            restart_after:   r.restart_after.unwrap_or(d.restart_after),
            max_restarts:    r.max_restarts.unwrap_or(d.max_restarts),
            tabu_l1:         r.tabu_l1.unwrap_or(d.tabu_l1),
            tabu_l2:         r.tabu_l2.unwrap_or(d.tabu_l2),
            tabu_inter:      r.tabu_inter.unwrap_or(d.tabu_inter),
            inter_sample:    r.inter_sample.unwrap_or(d.inter_sample),
            ab_sample_limit:     r.ab_sample_limit.unwrap_or(d.ab_sample_limit),
            log_interval:        r.log_interval.unwrap_or(d.log_interval),
            perturbation_swaps:    r.perturbation_swaps.unwrap_or(d.perturbation_swaps),
            tenure_grow_threshold: r.tenure_grow_threshold.unwrap_or(d.tenure_grow_threshold),
            tenure_grow_interval:  r.tenure_grow_interval.unwrap_or(d.tenure_grow_interval),
            tenure_max_scale:      r.tenure_max_scale.unwrap_or(d.tenure_max_scale),
        }
    }

    /// デフォルト値と設定ファイルの内容をマージして Weights を生成する
    pub fn build_weights(&self) -> Weights {
        let w = &self.weights;
        let s = &self.slot_difficulty;
        let d = Weights::default();
        Weights {
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
                s.row0.unwrap_or(d.slot_difficulty[0]),
                s.row1.unwrap_or(d.slot_difficulty[1]),
                s.row2.unwrap_or(d.slot_difficulty[2]),
            ],
        }
    }

    /// 実効的なコーパスパスを返す（CLIオプションで上書き可能）
    pub fn corpus_path(&self, cli_override: Option<&str>) -> String {
        cli_override
            .map(|s| s.to_owned())
            .or_else(|| self.run.corpus.clone())
            .unwrap_or_else(|| "corpus.txt".to_owned())
    }

    /// 実効的なシードを返す（CLIオプションで上書き可能）
    /// 未指定の場合はOS乱数から生成する
    pub fn seed(&self, cli_override: Option<u64>) -> u64 {
        cli_override.or(self.run.seed).unwrap_or_else(rand::random)
    }
}

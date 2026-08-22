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

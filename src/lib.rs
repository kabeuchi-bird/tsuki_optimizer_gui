// lib.rs — tsuki_optimize ライブラリクレート
//
// CLI (main.rs) と GUI (bin/gui.rs) の両方から利用される。

pub mod chars;
pub mod config;
pub mod corpus;
pub mod cost;
pub mod layout;
pub mod search;

/// ローカルタイムのタイムスタンプ文字列（YYMMDD_HHMMSS）を生成する
pub fn local_timestamp() -> String {
    chrono::Local::now().format("%y%m%d_%H%M%S").to_string()
}

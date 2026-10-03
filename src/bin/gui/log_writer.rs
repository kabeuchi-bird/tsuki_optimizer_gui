use std::io::Write;
use std::sync::mpsc;

use tsuki_optimize::chars::MAX_CHARS;

// ──────────────────────────────────────────────────────────────
// 色分けモード
// ──────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Fitness,
    Frequency,
    FingerLoad,
    Log,
}

/// 色分けモードの事前計算データ
pub enum ColorData {
    /// 文字ごとの「頻度順位とスロット難易度順位のずれ」（0 = ぴったり、1 以上 = 最大のずれ）。
    /// 基底かな（L1/L2）と子音（拗音面）は別々の群で順位付けする。
    /// 更新ごとに一度だけ作るキャッシュなので、enum を肥大させないよう Box に入れる。
    Fitness { mismatch: Box<[f32; MAX_CHARS]> },
    Frequency {
        max_freq: f64,
        /// シフトキーの打鍵頻度 [shift_left, shift_right]
        shift_freq: [f64; 2],
    },
    None,
}

// ──────────────────────────────────────────────────────────────
// ChannelWriter: ログテキストを GUI チャネルへ送る（`LogTee` の出力先）
// ──────────────────────────────────────────────────────────────
pub struct ChannelWriter(pub mpsc::Sender<String>);

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = self.0.send(String::from_utf8_lossy(buf).into_owned());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

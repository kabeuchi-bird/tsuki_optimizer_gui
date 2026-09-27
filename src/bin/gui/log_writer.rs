use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

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
// GuiLogWriter: ログテキストを GUI チャネル + ファイルに書き込む
// ──────────────────────────────────────────────────────────────
pub struct GuiLogWriter {
    pub tx: mpsc::Sender<String>,
    pub file: Option<BufWriter<File>>,
    pub stop_flag: Arc<AtomicBool>,
}

impl GuiLogWriter {
    /// ファイル書き込みエラーを GUI に通知し、探索を中断してファイル出力を止める
    fn fail(&mut self, e: std::io::Error) {
        let msg = format!("⚠ ログファイル書き込みエラー: {e}\n探索を中断します。\n");
        let _ = self.tx.send(msg);
        self.stop_flag.store(true, Ordering::Relaxed);
        self.file = None;
    }
}

impl Write for GuiLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let _ = self.tx.send(text.into_owned());
        if let Some(Err(e)) = self.file.as_mut().map(|f| f.write_all(buf)) {
            self.fail(e);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(Err(e)) = self.file.as_mut().map(|f| f.flush()) {
            self.fail(e);
        }
        Ok(())
    }
}

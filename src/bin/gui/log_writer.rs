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
    Fitness {
        freq_rank: [u8; MAX_CHARS],
        slot_rank: [u8; 66],
        num_valid: f32,
    },
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

impl Write for GuiLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let _ = self.tx.send(text.into_owned());
        if let Some(ref mut f) = self.file {
            if let Err(e) = f.write_all(buf) {
                let msg = format!("⚠ ログファイル書き込みエラー: {e}\n探索を中断します。\n");
                let _ = self.tx.send(msg);
                self.stop_flag.store(true, Ordering::Relaxed);
                self.file = None;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(ref mut f) = self.file {
            if let Err(e) = f.flush() {
                let msg = format!("⚠ ログファイル書き込みエラー: {e}\n探索を中断します。\n");
                let _ = self.tx.send(msg);
                self.stop_flag.store(true, Ordering::Relaxed);
                self.file = None;
            }
        }
        Ok(())
    }
}

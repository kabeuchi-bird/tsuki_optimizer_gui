// corpus.rs — コーパス読み込みとn-gram統計

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::chars::{CharId, NUM_CHARS, build_char_to_id, decompose};

/// ——————————————————————————————
/// コーパス統計
/// ——————————————————————————————
pub struct Corpus {
    /// ユニグラム頻度（CharId → 正規化頻度）
    pub unigrams: [f64; NUM_CHARS],

    /// バイグラム：(c1, c2) → 正規化頻度
    pub bigrams: Vec<BigramEntry>,

    /// トライグラム：(c1, c2, c3) → 正規化頻度（準交互評価用）
    pub trigrams: Vec<TrigramEntry>,

    /// バイグラム隣接リスト: bigram_adj[c] = そのcharが絡む bigrams インデックス群
    pub bigram_adj: Vec<Vec<usize>>,

    /// トライグラム隣接リスト: trigram_adj[c]
    pub trigram_adj: Vec<Vec<usize>>,
}

#[derive(Clone, Copy)]
pub struct BigramEntry {
    pub c1: CharId,
    pub c2: CharId,
    pub freq: f64,
}

#[derive(Clone, Copy)]
pub struct TrigramEntry {
    pub c1: CharId,
    pub c2: CharId,
    pub c3: CharId,
    pub freq: f64,
}

impl Corpus {
    /// テキストファイルからコーパスを構築する。
    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(Self::from_str(&text))
    }

    /// テキスト文字列からコーパスを構築する。
    ///
    /// # セグメント分割ルール
    /// - 配字されている文字（有声音含む）→ CharId に変換してセグメントに追加
    /// - 改行文字（`\n`, `\r`）         → スキップ（セグメントを切らない）
    /// - それ以外の配字外文字           → セグメントを切る
    ///
    /// バイグラム・トライグラムはセグメント内でのみカウントされる。
    /// これにより、漢字や記号などをまたいだ誤ったn-gramが生成されない。
    pub fn from_str(text: &str) -> Self {
        let map = build_char_to_id();

        // ── テキスト → セグメント列に変換 ────────────────
        // セグメント: 配字外文字（改行除く）で切られた CharId の連続列
        let mut segments: Vec<Vec<CharId>> = Vec::new();
        let mut current: Vec<CharId> = Vec::new();
        let mut skipped_chars = 0u64;

        for c in text.chars() {
            // 改行はスキップ（セグメントを切らない）
            if c == '\n' || c == '\r' {
                continue;
            }

            let ids = decompose(c, &map);
            if ids.as_slice().is_empty() {
                // 配字外文字 → 現在のセグメントを確定して新しく始める
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
                skipped_chars += 1;
            } else {
                current.extend_from_slice(ids.as_slice());
            }
        }
        // 末尾のセグメントを確定
        if !current.is_empty() {
            segments.push(current);
        }

        // ── n-gramカウント ────────────────────────────────
        let mut uni_count = [0u64; NUM_CHARS];
        let mut bi_count:  HashMap<(CharId, CharId), u64>         = HashMap::new();
        let mut tri_count: HashMap<(CharId, CharId, CharId), u64> = HashMap::new();
        let mut total_chars = 0u64;

        for seg in &segments {
            let n = seg.len();
            total_chars += n as u64;

            for i in 0..n {
                uni_count[seg[i] as usize] += 1;
            }
            // バイグラム: セグメント内のみ
            for i in 0..n.saturating_sub(1) {
                *bi_count.entry((seg[i], seg[i + 1])).or_insert(0) += 1;
            }
            // トライグラム: セグメント内のみ
            for i in 0..n.saturating_sub(2) {
                *tri_count.entry((seg[i], seg[i + 1], seg[i + 2])).or_insert(0) += 1;
            }
        }

        if total_chars == 0 {
            return Self::empty();
        }

        let total = total_chars as f64;

        // ── 正規化 ────────────────────────────────────────
        let mut unigrams = [0.0f64; NUM_CHARS];
        for (i, &c) in uni_count.iter().enumerate() {
            unigrams[i] = c as f64 / total;
        }

        let bigrams: Vec<BigramEntry> = bi_count.iter()
            .map(|(&(c1, c2), &cnt)| BigramEntry { c1, c2, freq: cnt as f64 / total })
            .collect();

        let trigrams: Vec<TrigramEntry> = tri_count.iter()
            .map(|(&(c1, c2, c3), &cnt)| TrigramEntry { c1, c2, c3, freq: cnt as f64 / total })
            .collect();

        // ── 隣接リスト構築 ────────────────────────────────
        let bigram_adj  = Self::build_bigram_adj(&bigrams);
        let trigram_adj = Self::build_trigram_adj(&trigrams);

        eprintln!(
            "コーパス統計: 有効文字数={}, スキップ文字数={}, セグメント数={}, \
             ユニグラム種={}, バイグラム種={}, トライグラム種={}",
            total_chars,
            skipped_chars,
            segments.len(),
            uni_count.iter().filter(|&&c| c > 0).count(),
            bigrams.len(),
            trigrams.len(),
        );

        Corpus { unigrams, bigrams, trigrams, bigram_adj, trigram_adj }
    }

    fn empty() -> Self {
        Corpus {
            unigrams:    [0.0; NUM_CHARS],
            bigrams:     vec![],
            trigrams:    vec![],
            bigram_adj:  vec![vec![]; NUM_CHARS],
            trigram_adj: vec![vec![]; NUM_CHARS],
        }
    }

    fn build_bigram_adj(bigrams: &[BigramEntry]) -> Vec<Vec<usize>> {
        let mut adj = vec![vec![]; NUM_CHARS];
        for (idx, bg) in bigrams.iter().enumerate() {
            adj[bg.c1 as usize].push(idx);
            if bg.c2 != bg.c1 {
                adj[bg.c2 as usize].push(idx);
            }
        }
        adj
    }

    fn build_trigram_adj(trigrams: &[TrigramEntry]) -> Vec<Vec<usize>> {
        let mut adj = vec![vec![]; NUM_CHARS];
        for (idx, tg) in trigrams.iter().enumerate() {
            // 同じcharが複数位置に現れる場合も1回だけ追加
            let mut seen: u64 = 0; // CharId < 64 なので u64 ビットマスクで足りる
            for &c in &[tg.c1, tg.c2, tg.c3] {
                let bit = 1u64 << c;
                if seen & bit == 0 {
                    adj[c as usize].push(idx);
                    seen |= bit;
                }
            }
        }
        adj
    }
}

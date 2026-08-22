// corpus.rs — コーパス読み込みとn-gram統計

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::chars::{build_char_to_id, decompose, CharId, MAX_CHARS};
use crate::yoon::YoonTable;

/// ——————————————————————————————
/// コーパス統計
/// ——————————————————————————————
#[derive(Clone)]
pub struct Corpus {
    /// ユニグラム頻度（CharId → 正規化頻度）
    /// サイズは MAX_CHARS（64）。3x10 では [60..64] は 0.0。
    pub unigrams: [f64; MAX_CHARS],

    /// バイグラム：(c1, c2) → 正規化頻度
    pub bigrams: Vec<BigramEntry>,

    /// トライグラム：(c1, c2, c3) → 正規化頻度
    pub trigrams: Vec<TrigramEntry>,

    /// バイグラム隣接リスト: bigram_adj[c] = そのcharが絡む bigrams インデックス群
    pub bigram_adj: Vec<Vec<usize>>,

    /// トライグラム隣接リスト
    pub trigram_adj: Vec<Vec<usize>>,

    /// コーパス構築時の統計情報
    pub stats: CorpusStats,
}

#[derive(Clone, Default)]
pub struct CorpusStats {
    pub total_chars: u64,
    pub skipped_chars: u64,
    /// 子音に先行されない小書きゃゅょ（てゃ/ふゅ等）でスキップした数（hybrid のみ）
    pub yoon_skipped: u64,
    pub num_segments: usize,
    pub num_unigrams: usize,
    pub num_bigrams: usize,
    pub num_trigrams: usize,
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
    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        Self::from_file_with_yoon(path, None)
    }

    /// 拗音テーブルを指定してファイルからコーパスを構築する（None で既存動作）。
    pub fn from_file_with_yoon(path: &Path, yoon: Option<&YoonTable>) -> std::io::Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(Self::from_str_with_yoon(&text, yoon))
    }

    /// テキスト文字列からコーパスを構築する（既存動作、拗音分解なし）。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Self {
        Self::from_str_with_yoon(text, None)
    }

    /// テキスト文字列からコーパスを構築する。
    ///
    /// # セグメント分割ルール
    /// - 配字されている文字（有声音含む）→ CharId に変換してセグメントに追加
    /// - 改行文字（`\n`, `\r`）         → スキップ
    /// - それ以外の配字外文字           → セグメントを切る
    ///
    /// # 拗音分解（`yoon = Some(table)` のとき）
    /// - `基底かな + ゃ/ゅ/ょ` を最優先でユニット化 → `[子音CharId, 小書きCharId]` の2トークン
    /// - 子音に先行されない小書き ゃゅょ（てゃ/ふゅ等）→ セグメントを切り、yoon_skipped を加算
    pub fn from_str_with_yoon(text: &str, yoon: Option<&YoonTable>) -> Self {
        let map = build_char_to_id();

        let chars: Vec<char> = text.chars().collect();
        let mut segments: Vec<Vec<CharId>> = Vec::new();
        let mut current: Vec<CharId> = Vec::new();
        let mut skipped_chars = 0u64;
        let mut yoon_skipped = 0u64;

        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '\n' || c == '\r' {
                i += 1;
                continue;
            }

            if let Some(yt) = yoon {
                // 拗音ユニットの先読み（基底かな + 小書きゃゅょ）
                if i + 1 < chars.len() && YoonTable::is_shift_char(chars[i + 1]) {
                    if let Some(cons_id) = yt.lookup_unit(c, chars[i + 1]) {
                        let small_id = yt
                            .shift_id(chars[i + 1])
                            .expect("拗音シフトかなは CharId を持つ");
                        current.push(cons_id);
                        current.push(small_id);
                        i += 2;
                        continue;
                    }
                }
                // 子音に先行されない小書き ゃゅょ → セグメント区切り＋スキップ
                if YoonTable::is_shift_char(c) {
                    if !current.is_empty() {
                        segments.push(std::mem::take(&mut current));
                    }
                    yoon_skipped += 1;
                    i += 1;
                    continue;
                }
            }

            let ids = decompose(c, &map);
            if ids.as_slice().is_empty() {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
                skipped_chars += 1;
            } else {
                current.extend_from_slice(ids.as_slice());
            }
            i += 1;
        }
        if !current.is_empty() {
            segments.push(current);
        }

        let mut uni_count = [0u64; MAX_CHARS];
        let mut bi_count: HashMap<(CharId, CharId), u64> = HashMap::new();
        let mut tri_count: HashMap<(CharId, CharId, CharId), u64> = HashMap::new();
        let mut total_chars = 0u64;

        for seg in &segments {
            let n = seg.len();
            total_chars += n as u64;

            for i in 0..n {
                uni_count[seg[i] as usize] += 1;
            }
            for i in 0..n.saturating_sub(1) {
                *bi_count.entry((seg[i], seg[i + 1])).or_insert(0) += 1;
            }
            for i in 0..n.saturating_sub(2) {
                *tri_count
                    .entry((seg[i], seg[i + 1], seg[i + 2]))
                    .or_insert(0) += 1;
            }
        }

        if total_chars == 0 {
            return Self::empty();
        }

        let total = total_chars as f64;

        let mut unigrams = [0.0f64; MAX_CHARS];
        for (i, &c) in uni_count.iter().enumerate() {
            unigrams[i] = c as f64 / total;
        }

        let bigrams: Vec<BigramEntry> = bi_count
            .iter()
            .map(|(&(c1, c2), &cnt)| BigramEntry {
                c1,
                c2,
                freq: cnt as f64 / total,
            })
            .collect();

        let trigrams: Vec<TrigramEntry> = tri_count
            .iter()
            .map(|(&(c1, c2, c3), &cnt)| TrigramEntry {
                c1,
                c2,
                c3,
                freq: cnt as f64 / total,
            })
            .collect();

        let bigram_adj = Self::build_bigram_adj(&bigrams);
        let trigram_adj = Self::build_trigram_adj(&trigrams);

        let stats = CorpusStats {
            total_chars,
            skipped_chars,
            yoon_skipped,
            num_segments: segments.len(),
            num_unigrams: uni_count.iter().filter(|&&c| c > 0).count(),
            num_bigrams: bigrams.len(),
            num_trigrams: trigrams.len(),
        };

        Corpus {
            unigrams,
            bigrams,
            trigrams,
            bigram_adj,
            trigram_adj,
            stats,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.stats.total_chars == 0
    }

    fn empty() -> Self {
        Corpus {
            unigrams: [0.0; MAX_CHARS],
            bigrams: vec![],
            trigrams: vec![],
            bigram_adj: vec![vec![]; MAX_CHARS],
            trigram_adj: vec![vec![]; MAX_CHARS],
            stats: CorpusStats::default(),
        }
    }

    fn build_bigram_adj(bigrams: &[BigramEntry]) -> Vec<Vec<usize>> {
        let mut adj = vec![vec![]; MAX_CHARS];
        for (idx, bg) in bigrams.iter().enumerate() {
            adj[bg.c1 as usize].push(idx);
            if bg.c2 != bg.c1 {
                adj[bg.c2 as usize].push(idx);
            }
        }
        adj
    }

    fn build_trigram_adj(trigrams: &[TrigramEntry]) -> Vec<Vec<usize>> {
        let mut adj = vec![vec![]; MAX_CHARS];
        for (idx, tg) in trigrams.iter().enumerate() {
            // CharId < 128 なので u128 ビットマスクで重複排除
            let mut seen: u128 = 0;
            for &c in &[tg.c1, tg.c2, tg.c3] {
                let bit = 1u128 << c;
                if seen & bit == 0 {
                    adj[c as usize].push(idx);
                    seen |= bit;
                }
            }
        }
        adj
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_str_unigrams_normalized() {
        let corpus = Corpus::from_str("ああいう");
        let total: f64 = corpus.unigrams.iter().sum();
        // ユニグラム頻度は正規化されている（合計 ≈ 1.0）
        assert!((total - 1.0).abs() < 1e-10);
        // 'あ' の頻度 > 'い' の頻度（2回 vs 1回）
        let map = crate::chars::build_char_to_id();
        let a_id = map[&'あ'] as usize;
        let i_id = map[&'い'] as usize;
        assert!(corpus.unigrams[a_id] > corpus.unigrams[i_id]);
    }

    #[test]
    fn test_from_str_bigrams_nonempty() {
        let corpus = Corpus::from_str("あいう");
        // バイグラム隣接リストが空でない
        let has_bigrams = corpus.bigram_adj.iter().any(|v| !v.is_empty());
        assert!(has_bigrams);
    }

    // ── 拗音分解（hybrid）─────────────────────────
    fn default_table() -> YoonTable {
        YoonTable::from_spec(crate::yoon::DEFAULT_CONSONANTS).unwrap()
    }

    fn has_bigram(corpus: &Corpus, c1: CharId, c2: CharId) -> bool {
        corpus.bigrams.iter().any(|b| b.c1 == c1 && b.c2 == c2)
    }

    #[test]
    fn test_yoon_unit_kya() {
        // きゃ → [Ky, ゃ]（2トークン、き単独は現れない）
        let yt = default_table();
        let map = crate::chars::build_char_to_id();
        let corpus = Corpus::from_str_with_yoon("きゃ", Some(&yt));
        let ky = yt.lookup_unit('き', 'ゃ').unwrap();
        let ya = map[&'ゃ'];
        assert_eq!(corpus.stats.total_chars, 2);
        assert!(corpus.unigrams[ky as usize] > 0.0);
        assert!(corpus.unigrams[ya as usize] > 0.0);
        assert_eq!(corpus.unigrams[map[&'き'] as usize], 0.0);
        assert!(has_bigram(&corpus, ky, ya));
    }

    #[test]
    fn test_yoon_unit_voiced_and_semivoiced() {
        // ぎょ → [Gy, ょ]、じゅ → [J, ゅ]、ぴゃ → [Py, ゃ]
        let yt = default_table();
        let map = crate::chars::build_char_to_id();
        for (word, base, small) in [("ぎょ", 'ぎ', 'ょ'), ("じゅ", 'じ', 'ゅ'), ("ぴゃ", 'ぴ', 'ゃ')] {
            let corpus = Corpus::from_str_with_yoon(word, Some(&yt));
            let cons = yt.lookup_unit(base, small).unwrap();
            let small_id = map[&small];
            assert_eq!(corpus.stats.total_chars, 2, "{}", word);
            assert!(corpus.unigrams[cons as usize] > 0.0, "{}", word);
            assert!(has_bigram(&corpus, cons, small_id), "{}", word);
            // 濁点/半濁点は展開されない（子音が濁りを内包する）
            assert_eq!(corpus.unigrams[crate::chars::DAKUTEN_ID as usize], 0.0, "{}", word);
            assert_eq!(corpus.unigrams[crate::chars::HANDAKUTEN_ID as usize], 0.0, "{}", word);
        }
    }

    #[test]
    fn test_yoon_unit_with_trailing() {
        // きょう → [Ky, ょ, う]（3トークン）
        let yt = default_table();
        let map = crate::chars::build_char_to_id();
        let corpus = Corpus::from_str_with_yoon("きょう", Some(&yt));
        let ky = yt.lookup_unit('き', 'ょ').unwrap();
        let yo = map[&'ょ'];
        let u = map[&'う'];
        assert_eq!(corpus.stats.total_chars, 3);
        assert!(has_bigram(&corpus, ky, yo));
        assert!(has_bigram(&corpus, yo, u));
    }

    #[test]
    fn test_yoon_orphan_small_kana_skipped() {
        // てゃ → 子音に先行されない小書き ゃ でセグメント区切り＋スキップ。て は通常文字。
        let yt = default_table();
        let map = crate::chars::build_char_to_id();
        let corpus = Corpus::from_str_with_yoon("てゃ", Some(&yt));
        assert_eq!(corpus.stats.yoon_skipped, 1);
        assert!(corpus.unigrams[map[&'て'] as usize] > 0.0);
        // ゃ 単独はユニグラムに現れない（スキップ）
        assert_eq!(corpus.unigrams[map[&'ゃ'] as usize], 0.0);
    }

    #[test]
    fn test_yoon_none_matches_plain() {
        // yoon = None は既存動作と完全一致（きゃ は き + ゃ の2文字）
        let map = crate::chars::build_char_to_id();
        let corpus = Corpus::from_str("きゃ");
        assert_eq!(corpus.stats.total_chars, 2);
        assert!(corpus.unigrams[map[&'き'] as usize] > 0.0);
        assert!(corpus.unigrams[map[&'ゃ'] as usize] > 0.0);
        assert_eq!(corpus.stats.yoon_skipped, 0);
    }
}

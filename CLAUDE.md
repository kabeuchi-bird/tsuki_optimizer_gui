# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

月配列2-263改変版のかな配列をコーパスに基づきタブーサーチで最適化する Rust ツール。CLI (`tsuki_optimize`) と GUI (`gui`, eframe/egui) の2バイナリが同じライブラリクレートを共有する。コメント・ログ・README・レビュー（CodeRabbit は `ja-JP`）はすべて日本語。

## コマンド

```bash
cargo build --release                 # 両バイナリ → target/release/
cargo test                            # 全テスト（各モジュール内の #[cfg(test)] mod tests）
cargo test <テスト名の一部>            # 単一テスト（例: cargo test delta）
cargo test -p tsuki_optimize cost::   # モジュール単位
cargo clippy -- -D warnings           # CI は警告をエラー扱い
./target/release/tsuki_optimize --iter 100000 --seed 42 --keyboard-size 3x11 --yoon hybrid
./target/release/gui
```

CI (`.github/workflows/ci.yml`) は Linux/Windows/macOS で build → test → clippy。Linux で GUI をビルドするには `libxkbcommon-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libfontconfig1-dev` が必要。

## アーキテクチャ

データの流れ: `config.toml`（+CLI 上書き）→ `KeyboardParams` / `Weights` / `SearchConfig` → コーパス解析 → 初期配列生成 → `search::run` → ログ出力。CLI と GUI は設定の解決（CLI 引数 or GUI 入力欄）とエラー表示だけを持ち、コーパス読込（`load_corpus`）・ログ作成（`create_log_file`）・検証から結果出力までの本体（`Run::execute`）は `lib.rs` で共有する。

- **`chars.rs`** — 文字は `CharId = u8` で扱う。ID 空間は領域分割されている: `[0..62)` 基底かな、`[62..64)` L1/L2 の void、`[64..64+npl)` 拗音面（子音 → void の順に動的採番）。`MAX_CHARS = 97`。子音/拗音 void の判定は `KeyboardParams::is_consonant` / `is_yoon_void` のみを使うこと。
- **`layout.rs`** — `KeyboardParams`（`3x10` / `3x10_single_shift` / `3x11`、yoon 有無）と `Layout`（`char_to_slot` / `slot_to_char` の双方向マップ、両方の整合を常に保つ）。スロットは層ごとに連番（L1, L2, 拗音面）で、`physical_of` で物理キーに戻す。固定文字・シフトキー位置・排他ペア・拗音制約によるスワップ禁止判定（`swap_would_violate`）もここ。
- **`corpus.rs`** — 配列外の文字を区切りとしてセグメント分割し、ユニ/バイ/トライグラム頻度（合計≈1.0 に正規化）と隣接リストを作る。hybrid 時は `yoon.rs` の最長一致分解（`しゃ`→`Sh`+`ゃ`）を通す。
- **`cost.rs`** — `score()`（全体評価）と `delta_score()`（スワップ差分評価）。**両者は常に一致しなければならない**（テストで検証）。重みを追加・変更したら両方と `score_breakdown_data` に反映する。
- **`search.rs`** — タブーサーチ本体。性能最優先のホットループで、ループ内のヒープアロケーションは避ける（バッファは事前確保して `_into` 系関数で再利用）。要素:
  - 操作種別 `OpKind`（L1内 / L2内 / 層間 / 拗音面）ごとにタブーテニュアを**近傍サイズ比**（`tabu_ratio`）で持ち、初回反復の実測候補数から実手数を決める。
  - タブー判定はペアインデックスのビットセット、`DeltaPairCache` は u128 の dirty mask で無効化（`MAX_CHARS <= 128` を静的アサート）。
  - テニュアは固定。停滞時に頻度ベース長期記憶（`diversification`）、さらに停滞でリスタート（摂動シャッフル）。
  - 停止/中間報告は `Arc<AtomicBool>`（CLI では SIGINT/SIGUSR1、GUI では停止ボタン）、進捗は `on_update` コールバックで `SearchUpdate` を渡す。
- **`yoon.rs`** — ハイブリッド拗音方式（子音1打＋後置シフト `ゃゅょ`）。子音レジストリと `YoonSetup`。
- **`user_layout.rs`** — `initial_layout.toml` の読み込み・検証。失敗時はランダム配置にフォールバック。
- **`config.rs`** — TOML 設定。全構造体が `serde(deny_unknown_fields)` なので、フィールド追加時は `config.toml` とデフォルト値も合わせて更新する。廃止フィールド（`tabu_l1` 等）は `Option` で受けて警告のみ出す。
- **`src/bin/gui/`** — 探索は `std::thread::spawn` した別スレッドで実行し、`mpsc` で `SearchUpdate` とログ文字列を UI に送る（`app.rs`）。`update.rs` が `eframe::App`、`draw.rs` が描画・色分け。日本語フォントは `assets/ipag.ttf` を埋め込み。

## 注意点

- 設定の優先順位は CLI → TOML → デフォルト。新しい探索パラメータは `config.rs`・`SearchConfig`・`main.rs` の CLI パース・GUI のパラメータ欄・`lib.rs::write_config_summary`・`config.toml` コメント・README の該当表をまとめて更新する。
- 3x10 / 3x10_single_shift / 3x11 と yoon `none` / `hybrid` のすべての組み合わせで動くことを意識する。`mode = "none"` の挙動とスコアは既存と完全に同じでなければならない。

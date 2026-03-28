# tsuki_optimizer GUI 実装方針書

## 概要

既存の `tsuki_optimizer`（かな配列タブーサーチ最適化ツール）のコードをライブラリとして活用し、最適解（ベストスコア配列）をリアルタイムに可視化・更新し続ける GUI アプリケーションを構築する。

- リポジトリ: <https://github.com/kabeuchi-bird/tsuki_optimizer/>
- 言語: Rust
- GUI フレームワーク: **eframe (egui)**

---

## アーキテクチャ

### スレッド構成

```
┌─────────────────────────────────────────┐
│  GUI スレッド (eframe/egui)             │
│                                         │
│  [開始] → spawn background thread       │
│  [停止] → stop_flag.store(true)         │
│                                         │
│  毎フレーム: try_recv() で最新状態を描画 │
└──────────────┬──────────────────────────┘
               │ mpsc::channel
               │ (SearchUpdate を送る)
┌──────────────┴──────────────────────────┐
│  探索スレッド                            │
│                                         │
│  search::run() のループ内で             │
│  ベスト更新時 → tx.send(SearchUpdate)   │
│  stop_flag チェック → true なら終了      │
└─────────────────────────────────────────┘
```

### 通信データ

```rust
pub struct SearchUpdate {
    pub iter: usize,
    pub restarts: usize,
    pub current_score: f64,
    pub best_score: f64,
    pub best_layout: Layout,       // Clone して送る
    pub phase: SearchPhase,        // Running / Restarting / Finished
}

pub enum SearchPhase {
    Running,
    Restarting,
    Finished,
}
```

---

## 既存コードへの変更

### 1. クレート構成の変更

現在 `main.rs` にすべてが入っている。以下のように分離する。

- `src/lib.rs` を新設し、`chars`, `config`, `corpus`, `cost`, `layout`, `search` の各モジュールを `pub mod` として公開する
- `src/main.rs` は CLI エントリポイントとして残す（`lib.rs` を利用する形に書き換え）
- GUI 用エントリポイントは `src/bin/gui.rs`（または別クレートで workspace 構成）

### 2. `search::run()` へのコールバック追加

```rust
// 変更前
pub fn run(
    initial_layout: Layout,
    ctx: &SearchContext,
    config: &SearchConfig,
    rng: &mut impl Rng,
    stop_flag: &Arc<AtomicBool>,
    report_flag: &Arc<AtomicBool>,
    out: &mut impl Write,
) -> Layout

// 変更後
pub fn run(
    initial_layout: Layout,
    ctx: &SearchContext,
    config: &SearchConfig,
    rng: &mut impl Rng,
    stop_flag: &Arc<AtomicBool>,
    report_flag: &Arc<AtomicBool>,
    on_update: &mut impl FnMut(&SearchUpdate),  // ← 追加
    out: &mut impl Write,
) -> Layout
```

#### コールバック呼び出しタイミング

`search.rs` のループ内で以下の3箇所に `on_update` 呼び出しを挿入する。

1. **ベストスコア更新時**（`best_score` が改善された直後）
2. **ログ間隔ごと**（既存の `log_interval` タイミングに合わせる）
3. **探索終了時**（`phase: Finished` を送信）

#### CLI 側の互換性維持

既存の `main.rs` からは空のクロージャ `&mut |_| {}` を渡せば動作が変わらない。

### 3. `Layout` に `Clone` の追加

`Layout` 構造体が `Clone` を derive していなければ追加する（チャネル越しに送るため必須）。
`SearchUpdate` も `Clone` を derive する。

### 4. `score_breakdown` の構造化

現在の `score_breakdown()` は `impl Write` に直接テキストを書き出す。GUI で各内訳（打鍵数コスト、同指ペナルティ、交互打鍵ボーナス等）を個別に表示するため、内訳を構造体で返すバリアントを用意する。

```rust
pub struct ScoreBreakdown {
    pub total: f64,
    pub stroke_cost: f64,
    pub same_finger_penalty: f64,
    pub same_hand_penalty: f64,
    pub alternation_bonus: f64,
    pub inroll_bonus: f64,
    pub outroll_bonus: f64,
    // ... 必要に応じて追加
}

pub fn score_breakdown_data(layout: &Layout, corpus: &Corpus, weights: &Weights) -> ScoreBreakdown
```

---

## GUI 設計

### 画面レイアウト

```
┌─────────────────────────────────────────────────┐
│  ツールバー                                      │
│  [▶ 開始] [⏹ 停止]  seed: [____]  iter: [____]  │
├──────────────────────┬──────────────────────────┤
│                      │                          │
│  キーボード表示       │  スコア推移グラフ         │
│  (Layer 1 / Layer 2) │  (egui_plot)             │
│                      │                          │
├──────────────────────┴──────────────────────────┤
│  ステータスバー                                   │
│  iter: 12345 | restarts: 3 | best: 0.4321       │
├─────────────────────────────────────────────────┤
│  スコア内訳パネル (ScoreBreakdown)               │
│  指負荷バランス棒グラフ                           │
└─────────────────────────────────────────────────┘
```

### 停止ボタンの実装

- GUI スレッドで `Arc<AtomicBool>` を保持
- 「開始」ボタン押下時:
  1. `stop_flag` を `false` にリセット
  2. 探索スレッドを `std::thread::spawn` で起動
  3. ボタン表示を「停止」に切り替え
- 「停止」ボタン押下時:
  1. `stop_flag.store(true, Ordering::Relaxed)`
  2. 探索スレッドは次のイテレーションで自然に終了する（既存の SIGINT ハンドリングと同じ仕組み）
  3. 終了後、最終結果を表示しボタンを「開始」に戻す

### キーボード表示の色分け

3つの表示モードを用意し、ラジオボタンで切り替える。

#### モード1: フィットネスマップ（デフォルト）

「この文字はこの位置にふさわしいか」を示す。

- **計算方法**: 文字の出現頻度ランクとスロットの打ちやすさランク（`slot_difficulty` の逆順）のズレ（差の絶対値）
- **配色**:
  - ズレ小（良い配置）→ 緑系 `rgb(46, 160, 67)` ～ `rgb(144, 238, 144)`
  - ズレ中（普通）→ 黄系 `rgb(255, 255, 200)`
  - ズレ大（悪い配置）→ 赤系 `rgb(255, 160, 80)` ～ `rgb(220, 50, 50)`
- **意味**: 高頻度文字がホームポジションにあれば緑、小指上段にあれば赤。パッと見て最適化の質がわかる

#### モード2: 頻度ヒートマップ

各キーに割り当てられた文字のコーパス出現頻度で塗る。

- 高頻度 → 暖色（赤～オレンジ）
- 低頻度 → 寒色（青～紫）
- **見方**: 理想的な配列ではキーボード中央（ホーム段）が暖色、周辺が寒色になる

#### モード3: 指負荷バランス

キーボード表示ではなく棒グラフで表示。

```
左小 左薬 左中 左人 | 右人 右中 右薬 右小
 ██  ███ ████ ███  | ███ ████ ███  ██
```

- 各指に割り当てられた全文字の出現頻度合計を積み上げ
- 人差し指・中指に集中し小指が軽いほど良い

#### 共通の表示ルール

- Layer 1 / Layer 2 の区別: **枠線スタイル**で行う（L1=実線太枠、L2=破線枠）。色チャネルは色分けモードに確保する
- 各キーにはかな文字を重ねて表示
- シフトキー位置（☆/★）は灰色固定で表示

### スコア推移グラフ

- `egui_plot` を使用
- X 軸: イテレーション数
- Y 軸: スコア（低いほど良い）
- 2本の線を描画:
  - `current_score`（現在解、薄い線）
  - `best_score`（最良解、太い線）
- リスタート発生時点に縦線マーカーを入れると探索の挙動がわかりやすい

---

## 依存クレート（追加分）

```toml
[dependencies]
eframe = "0.31"         # egui フレームワーク
egui_plot = "0.31"      # グラフ描画 (eframe と同じバージョンに揃える)
```

※ バージョンは実装時点の最新に合わせること。

---

## ファイル構成案

```
tsuki_optimizer/
├── Cargo.toml             # workspace or bin targets 追加
├── src/
│   ├── lib.rs             # 新設: pub mod で各モジュールを公開
│   ├── main.rs            # CLI エントリポイント（既存を改修）
│   ├── bin/
│   │   └── gui.rs         # GUI エントリポイント
│   ├── chars.rs           # 既存（変更なし）
│   ├── config.rs          # 既存（変更なし）
│   ├── corpus.rs          # 既存（変更なし）
│   ├── cost.rs            # score_breakdown_data() を追加
│   ├── layout.rs          # Clone derive 確認
│   └── search.rs          # on_update コールバック追加, SearchUpdate 定義
└── tsuki_optimize.toml    # 既存（変更なし）
```

---

## 実装の優先順位

1. **lib.rs 分離 + CLI の動作確認** — 既存テストが壊れないことを確認
2. **`search::run()` へのコールバック追加** — 空クロージャで CLI が従来通り動くことを確認
3. **GUI の骨格** — eframe で窓を出し、開始/停止ボタン + ステータス表示
4. **キーボードグリッド描画** — Layer 1/2 のかな文字をグリッド表示（色分けなし）
5. **フィットネスマップ色分け** — デフォルト色分けモード
6. **スコア推移グラフ** — `egui_plot` でリアルタイム更新
7. **追加の色分けモード** — 頻度ヒートマップ、指負荷バランス
8. **`score_breakdown_data()` + 内訳パネル**

---

## 注意事項

- git への commit は手動で行うこと（自動 commit 禁止）
- `Layout` はスロット数が最大 66（`MAX_SLOTS`）の固定長配列で軽量なので、`Clone` して `mpsc` で送るコストは無視できる
- `SearchContext` はコーパスや重みへの参照（`&'a`）を持つため、探索スレッドに渡す際は `Arc` でラップするか、所有権ごと渡す設計にする必要がある
- 既存の `TeeWriter`（stderr + ログファイル）は GUI モードでは不要になる可能性がある。`out` 引数に `io::sink()` を渡すか、GUI のログパネルに流す `Write` 実装を用意する

---

## CI/CD: GitHub Actions によるクロスプラットフォームビルド

eframe (egui) は Windows / Linux 両対応だが、クロスコンパイル（例: Linux 上で Windows バイナリを生成）は GPU バックエンドの依存関係が複雑になるため、**GitHub Actions で各 OS のネイティブランナーを使ってビルドする**方針を取る。

### ワークフロー概要

```yaml
# .github/workflows/build.yml
name: Build

on:
  push:
    tags: ["v*"]       # タグ push 時にリリースビルド
  workflow_dispatch:    # 手動トリガーも可能に

jobs:
  build:
    strategy:
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            artifact_name: tsuki-optimizer-gui-linux-x86_64
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            artifact_name: tsuki-optimizer-gui-windows-x86_64

    runs-on: ${{ matrix.os }}

    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      # Linux のみ: egui に必要なシステムライブラリをインストール
      - name: Install Linux dependencies
        if: runner.os == 'Linux'
        run: |
          sudo apt-get update
          sudo apt-get install -y \
            libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
            libxkbcommon-dev libssl-dev libgtk-3-dev

      - name: Build (release)
        run: cargo build --release --bin gui --target ${{ matrix.target }}

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.artifact_name }}
          path: target/${{ matrix.target }}/release/gui*
```

### ポイント

- **Linux ランナー**: egui (glow バックエンド) が X11/Wayland 関連の C ライブラリを要求するため `apt-get install` ステップが必要
- **Windows ランナー**: MSVC ツールチェインが標準で入っているため追加依存なし
- **トリガー**: タグ push (`v*`) でリリースビルドを走らせる。開発中は `workflow_dispatch` で手動実行も可
- **成果物**: `actions/upload-artifact` でバイナリを保存。必要に応じて `softprops/action-gh-release` でリリースページへの自動アップロードも追加できる
- **CLI バイナリ**: GUI と並行して CLI バイナリ（`--bin tsuki_optimizer`）も同じワークフローでビルド・配布できる

### signal_hook の OS 依存について

既存コードの `signal_hook`（SIGINT / SIGUSR1）は Unix 専用。GUI モードでは停止制御をボタン経由で行うためシグナルハンドラは不要だが、CLI モードを Windows でもビルドする場合は `#[cfg(unix)]` ガードで囲む必要がある。

```rust
// main.rs（CLI）での条件コンパイル例
#[cfg(unix)]
{
    use signal_hook::consts::{SIGINT, SIGUSR1};
    use signal_hook::flag;
    flag::register(SIGINT, Arc::clone(&stop_flag)).expect("SIGINTハンドラの登録に失敗");
    flag::register(SIGUSR1, Arc::clone(&report_flag)).expect("SIGUSR1ハンドラの登録に失敗");
}
#[cfg(not(unix))]
{
    // Windows: Ctrl+C は std::process の既定動作に任せる
    // SIGUSR1 相当の機能は省略（GUI を使う想定）
    ctrlc::set_handler({
        let flag = Arc::clone(&stop_flag);
        move || { flag.store(true, std::sync::atomic::Ordering::Relaxed); }
    }).expect("Ctrl+Cハンドラの登録に失敗");
}
```

Windows CLI 用に `ctrlc` クレートを追加するか、CLI は Linux 専用と割り切るかは要判断。

#!/bin/bash
# run.sh — tsuki_optimize 実行スクリプト
# 標準出力・標準エラーを両方コンソールに表示しつつ log/ にも保存する

LOGDIR="$(dirname "$0")/log"
mkdir -p "$LOGDIR"

LOGFILE="$LOGDIR/$(date '+%y%m%d_%H%M%S').log"

echo "ログファイル: $LOGFILE" >&2

"$(dirname "$0")/target/release/tsuki_optimize" "$@" 2>&1 | tee "$LOGFILE"

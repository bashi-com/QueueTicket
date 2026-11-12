#!/usr/bin/env bash
# ==============================================================================
# QueueTicket PostgreSQL バックアップスクリプト (Raspberry Pi / cron 用)
#
# 【使い方】
#   1. PostgreSQL クライアントをインストールする
#      ※ pg_dump のメジャーバージョンは接続先 DB サーバー以上が必要です。
#        例 (Debian / Raspberry Pi OS):
#          sudo apt update && sudo apt install -y postgresql-client-16
#
#   2. 設定ファイル scripts/backup.env を作成する (chmod 600 推奨)
#      ※ Render の External Database URL を指定してください。
#        SSL 必須のため末尾に ?sslmode=require を付与してください。
#      例:
#        DATABASE_URL="postgres://user:pass@xxx.render.com/dbname?sslmode=require"
#        BACKUP_DIR="/home/pi/queueticket-backups"
#        KEEP=300
#
#   3. 手動実行で動作確認
#      /path/to/scripts/backup.sh
#
#   4. cron 設定例 (10分ごとに実行)
#      */10 * * * * /path/to/scripts/backup.sh >> $HOME/queueticket-backups/backup.log 2>&1
#
# 【復元方法】
#   ※ 復元前に必ずアプリケーション（Render の Web Service 等）を停止してください。
#   gunzip -c /path/to/queueticket-YYYYmmdd-HHMMSS.sql.gz | psql -v ON_ERROR_STOP=1 "$DATABASE_URL"
# ==============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "${SCRIPT_DIR}/backup.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  . "${SCRIPT_DIR}/backup.env"
  set +a
fi

# DATABASE_URL は必須（機密情報のため値はログに出力しない）
# Render の External Database URL は SSL 必須のため ?sslmode=require が必要です
if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "[$(TZ=Asia/Tokyo date '+%Y-%m-%d %H:%M:%S')] ERROR: DATABASE_URL is not set." >&2
  exit 2
fi

BACKUP_DIR="${BACKUP_DIR:-${HOME}/queueticket-backups}"
KEEP="${KEEP:-300}"
PG_DUMP="${PG_DUMP:-pg_dump}"

if ! [[ "${KEEP}" =~ ^[1-9][0-9]*$ ]]; then
  echo "[$(TZ=Asia/Tokyo date '+%Y-%m-%d %H:%M:%S')] ERROR: KEEP must be a positive integer (got: ${KEEP})." >&2
  exit 1
fi

mkdir -p "${BACKUP_DIR}"

LOCK_FILE="${BACKUP_DIR}/.lock"
exec 9>"${LOCK_FILE}"
if ! flock -n 9; then
  echo "[$(TZ=Asia/Tokyo date '+%Y-%m-%d %H:%M:%S')] INFO: 前回のバックアップが実行中のためスキップします。"
  exit 0
fi

TIMESTAMP="$(TZ=Asia/Tokyo date '+%Y%m%d-%H%M%S')"
FILENAME="queueticket-${TIMESTAMP}.sql.gz"
TARGET_PATH="${BACKUP_DIR}/${FILENAME}"
TMP_PATH="${TARGET_PATH}.tmp"

cleanup_tmp() {
  rm -f "${TMP_PATH}"
}
trap cleanup_tmp EXIT

if ! "${PG_DUMP}" --no-owner --no-privileges --clean --if-exists "${DATABASE_URL}" | gzip > "${TMP_PATH}"; then
  rm -f "${TMP_PATH}"
  echo "[$(TZ=Asia/Tokyo date '+%Y-%m-%d %H:%M:%S')] ERROR: pg_dump failed." >&2
  exit 1
fi

if ! gzip -t "${TMP_PATH}"; then
  rm -f "${TMP_PATH}"
  echo "[$(TZ=Asia/Tokyo date '+%Y-%m-%d %H:%M:%S')] ERROR: gzip integrity check failed." >&2
  exit 1
fi

mv -f "${TMP_PATH}" "${TARGET_PATH}"
trap - EXIT

# ローテーション: queueticket-*.sql.gz を新しい順に並べて KEEP 件を超える古いものを削除
mapfile -t BACKUP_FILES < <(
  find "${BACKUP_DIR}" -maxdepth 1 -type f -name 'queueticket-*.sql.gz' -printf '%f\n' | sort -r
)

if (( ${#BACKUP_FILES[@]} > KEEP )); then
  for (( i=KEEP; i<${#BACKUP_FILES[@]}; i++ )); do
    rm -f "${BACKUP_DIR}/${BACKUP_FILES[$i]}"
  done
fi

FILE_SIZE="$(du -h "${TARGET_PATH}" | cut -f1)"
echo "[$(TZ=Asia/Tokyo date '+%Y-%m-%d %H:%M:%S')] OK: ${FILENAME} (${FILE_SIZE})"

#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKUP_SH="${ROOT_DIR}/scripts/backup.sh"

TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/queueticket-backup-test.XXXXXX")"
cleanup() {
  rm -rf "${TMP_ROOT}"
}
trap cleanup EXIT

FAKE_PG_DUMP_OK="${TMP_ROOT}/fake_pg_dump_ok.sh"
cat <<'EOF' > "${FAKE_PG_DUMP_OK}"
#!/usr/bin/env bash
set -euo pipefail
echo "-- Fake PostgreSQL database dump"
echo "CREATE TABLE tickets (id int);"
EOF
chmod +x "${FAKE_PG_DUMP_OK}"

FAKE_PG_DUMP_FAIL="${TMP_ROOT}/fake_pg_dump_fail.sh"
cat <<'EOF' > "${FAKE_PG_DUMP_FAIL}"
#!/usr/bin/env bash
echo "pg_dump simulated error" >&2
exit 1
EOF
chmod +x "${FAKE_PG_DUMP_FAIL}"

# 1. 成功時に gz ファイルが1つでき、gunzip -c の中身が偽 pg_dump の出力と一致
DIR1="${TMP_ROOT}/case1"
DATABASE_URL="postgres://dummy:pass@localhost:5432/db?sslmode=require" \
  BACKUP_DIR="${DIR1}" \
  PG_DUMP="${FAKE_PG_DUMP_OK}" \
  "${BACKUP_SH}" >/dev/null

shopt -s nullglob
FILES1=("${DIR1}"/queueticket-*.sql.gz)
shopt -u nullglob
if [[ ${#FILES1[@]} -ne 1 ]]; then
  echo "FAIL case1: expected 1 gz file, got ${#FILES1[@]}" >&2
  exit 1
fi

EXPECTED_CONTENT=$'-- Fake PostgreSQL database dump\nCREATE TABLE tickets (id int);'
ACTUAL_CONTENT="$(gunzip -c "${FILES1[0]}")"
if [[ "${ACTUAL_CONTENT}" != "${EXPECTED_CONTENT}" ]]; then
  echo "FAIL case1: content mismatch" >&2
  exit 1
fi

# 2. KEEP=3 で5件分（既存ダミー4件 + 実行1件）→ 最新3件だけ残る（他ファイルは消えない）
DIR2="${TMP_ROOT}/case2"
mkdir -p "${DIR2}"
for ts in 20200101-000001 20200101-000002 20200101-000003 20200101-000004; do
  echo "dummy ${ts}" | gzip > "${DIR2}/queueticket-${ts}.sql.gz"
done
touch "${DIR2}/keep-other-file.txt"

DATABASE_URL="postgres://dummy:pass@localhost:5432/db?sslmode=require" \
  BACKUP_DIR="${DIR2}" \
  KEEP=3 \
  PG_DUMP="${FAKE_PG_DUMP_OK}" \
  "${BACKUP_SH}" >/dev/null

shopt -s nullglob
FILES2=("${DIR2}"/queueticket-*.sql.gz)
shopt -u nullglob
if [[ ${#FILES2[@]} -ne 3 ]]; then
  echo "FAIL case2: expected 3 gz files, got ${#FILES2[@]}" >&2
  exit 1
fi
if [[ -e "${DIR2}/queueticket-20200101-000001.sql.gz" || -e "${DIR2}/queueticket-20200101-000002.sql.gz" ]]; then
  echo "FAIL case2: oldest files were not removed" >&2
  exit 1
fi
if [[ ! -e "${DIR2}/queueticket-20200101-000003.sql.gz" || ! -e "${DIR2}/queueticket-20200101-000004.sql.gz" ]]; then
  echo "FAIL case2: newer files were unexpectedly removed" >&2
  exit 1
fi
if [[ ! -e "${DIR2}/keep-other-file.txt" ]]; then
  echo "FAIL case2: non-backup file was removed" >&2
  exit 1
fi

# 3. 偽 pg_dump が exit 1 → スクリプトが非0終了し、.sql.gz も .tmp も残らない
DIR3="${TMP_ROOT}/case3"
set +e
DATABASE_URL="postgres://dummy:pass@localhost:5432/db?sslmode=require" \
  BACKUP_DIR="${DIR3}" \
  PG_DUMP="${FAKE_PG_DUMP_FAIL}" \
  "${BACKUP_SH}" >/dev/null 2>&1
RC3=$?
set -e
if [[ ${RC3} -eq 0 ]]; then
  echo "FAIL case3: expected non-zero exit code when pg_dump fails" >&2
  exit 1
fi
shopt -s nullglob
LEFT3=("${DIR3}"/*.sql.gz "${DIR3}"/*.tmp)
shopt -u nullglob
if [[ ${#LEFT3[@]} -ne 0 ]]; then
  echo "FAIL case3: leftover files found: ${LEFT3[*]}" >&2
  exit 1
fi

# 4. DATABASE_URL 未設定 → exit 2
DIR4="${TMP_ROOT}/case4"
set +e
env -u DATABASE_URL \
  BACKUP_DIR="${DIR4}" \
  PG_DUMP="${FAKE_PG_DUMP_OK}" \
  "${BACKUP_SH}" >/dev/null 2>&1
RC4=$?
set -e
if [[ ${RC4} -ne 2 ]]; then
  echo "FAIL case4: expected exit code 2 when DATABASE_URL is unset, got ${RC4}" >&2
  exit 1
fi

# 5. KEEP=abc → 非0終了
DIR5="${TMP_ROOT}/case5"
set +e
DATABASE_URL="postgres://dummy:pass@localhost:5432/db?sslmode=require" \
  BACKUP_DIR="${DIR5}" \
  KEEP="abc" \
  PG_DUMP="${FAKE_PG_DUMP_OK}" \
  "${BACKUP_SH}" >/dev/null 2>&1
RC5=$?
set -e
if [[ ${RC5} -eq 0 ]]; then
  echo "FAIL case5: expected non-zero exit code when KEEP=abc" >&2
  exit 1
fi

echo "backup tests: OK"
exit 0

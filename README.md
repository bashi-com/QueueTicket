# 整理番号管理Webシステム v1.5.0

大学の文化祭やイベントでの混雑緩和を目的とした、Webベースの整理券発券・呼び出しシステムです。
Rust製の通常のバイナリとして起動し、PostgreSQLに接続して運用できます。

## 📖 概要

*   **出展者側**: PCやタブレットから整理番号を発券し、QRコードを表示。呼び出し管理画面からワンタップでステータス更新が可能。QRコード読み取り画面・プロジェクター投影画面にも対応。
*   **来場者側**: スマホでQRコードを読み取るだけで、専用アプリ不要で待ち状況を確認。順番が近づくと自動で画面が更新され通知されます。
*   **分析機能**: 発券・呼出・完了・戻り申告・初回表示の時刻ログをCSVで出力することが可能。

## ✨ 主な機能

1.  **整理券発券 (受付)**
    *   人数を入力してワンクリックで発券。
    *   来場者読み取り用のQRコードを即座に生成・表示。
    *   1〜999の整理番号を循環し、未完了の番号を飛ばして発券。再送による二重発券を防止し、直近20件の履歴からQRを再表示できます。
2.  **呼び出し管理 (誘導)**
    *   スマホ・タブレットに最適化されたカード型レイアウト（10秒間隔で自動更新）。
    *   「次の番号を呼ぶ」ボタンで待機中の最も若い番号を自動選択して呼び出し（二重送信や複数端末の同時押しでも1組だけ呼び出します）。各カードの個別「呼び出す」ボタンも併用可能。
    *   「戻り済み」「呼出中」「未着（呼出から10分以上）」「待機中」の4区分表示と、呼出からの経過分の表示。
    *   未着の整理券に対するスタッフ代理の「戻り確認」ボタンと、「完了する」ボタンによるステータス変更。
    *   「一部案内」ボタン（残り2名以上のとき）で、大人数グループの一部だけを先に案内可能。画面下部から出る人数ボタン（1〜残り−1名）を1タップで記録し、残りの方は「戻り済み・残りの方」として最優先で残ります。案内済み人数が全員に達すると自動で完了します（二重送信・複数端末の同時操作でも二重に数えません）。
3.  **来場者用マイページ**
    *   「あと何組待ちか」をリアルタイム表示（待ち状況に応じて15〜60秒間隔で自動更新）。
    *   直近30分の呼出ペースから算出した「呼び出しの目安」時刻を表示（直近30分の呼出が3件未満のときは非表示）と、10分ルールの注意書きを表示。
    *   呼び出しから10分以上経過して未着になった場合、「会場に戻りました」ボタンから戻り申告が可能（呼出から10分以降のみ受付）。申告後は「戻り受付済み」と表示されます。
    *   呼び出し時に画面デザイン・タブタイトルが変化し、対応端末では振動で通知。音は画面のボタンで有効にした場合のみ鳴ります。通信失敗時は表示を保持し、再接続を試みます。
4.  **QRコード読み取り画面 (`/admin/display/front`)**
    *   直近に発券（または履歴から再表示）した整理券のQRコードと整理番号・人数を大きく表示（2秒間隔で自動更新）。
    *   来場者がQRコードを読み取ると「✓ 読み取りました」を8秒間表示して待機画面に戻ります。未読み取りのまま5分経過した場合や、発券画面の「QRコード読み取り画面の表示を消す」を押した場合も待機画面に戻ります。
    *   待機画面には現在の待ち組数と、該当があるときのみお呼び出し中番号・未着番号を表示します。
5.  **プロジェクター投影画面 (`/admin/display/screen`)**
    *   端末内のローカルPV動画ファイルを選択して左7割でループ再生（動画はサーバーには送信しません）。右3割にお呼び出し中・次にお呼びする番号（最大3件）・お呼びした番号（未着）（いずれも該当があるときのみ表示）と、待ち組数・現在時刻を表示（3秒間隔で自動更新）。
    *   新しい呼び出しを検知すると、8秒間の全画面ポップアップ（「順番になりました」）とチャイム音で通知します（その間PV音量は自動で下がります）。
    *   動画なしモード（情報欄を全画面表示）や、右下の「⚙」ボタンからの動画差し替え・音声切り替えに対応。
6.  **データ分析**
    *   全データをCSV形式でダウンロード可能（発券・呼出・完了に加え、戻り申告時刻・初回表示時刻・案内済み人数を記録）。
    *   タイムスタンプ記録による回転率・ピークタイム分析。
7.  **セキュリティ**
    *   管理画面へのBasic認証。
    *   CSRF対策（Origin/Refererチェック）。
    *   タイミング攻撃耐性のある認証比較。

## 🛠 技術スタック

Rust / Tokio、Axum、SQLx、PostgreSQL、Askama (HTML/CSS)、自前のJavaScriptによる定期更新を使用します。SQLは実行時に評価するため、ビルド時にDB接続は不要です。

## 🚀 セットアップとデプロイ

Rust / CargoとPostgreSQLを用意し、`.env.example` を `.env` にコピーして `DATABASE_URL`、`ADMIN_PASSWORD`、`BASE_URL`（来場者がアクセスできる公開URL）を必ず設定します。`PORT` は既定8000、`BIND_ADDR` は既定 `0.0.0.0` です。`cargo run` で起動するとマイグレーションを自動適用します。ローカルの例は `BASE_URL=http://localhost:8000` で、管理画面は同URLの `/admin` にあります。

運用時は `cargo build --release` で作った `target/release/queueticket` に同じ環境変数を渡して実行します（`.env` があれば読み込みます）。PostgreSQLへの接続と公開URLからのアクセスを確保してください。Ctrl+CまたはSIGTERMで終了します。特定のホスティングサービスには依存しません。

## Docker での起動

Docker Engine + Docker Compose（Windows は Docker Desktop の Linux コンテナ）を用意します。

1. `.env.example` を `.env` にコピーします（`cp .env.example .env`、Windows のコマンドプロンプトでは `copy .env.example .env`）。
2. `.env` の `POSTGRES_PASSWORD` と `ADMIN_PASSWORD` を変更し、`BASE_URL=http://localhost:8000` を設定します。DBのユーザー名・DB名・パスワードは接続URLに直接使うため、英数字と `_`、`-` にしてください。
3. 次のコマンドでビルド・起動します。

```sh
docker compose up -d
docker compose logs -f app
```

起動後、<http://localhost:8000/admin> を開き、ユーザー名 `admin` と設定した `ADMIN_PASSWORD` でログインします。`APP_PORT` を変更するとホスト側ポートが変わるので、`BASE_URL` のポートも合わせてください。Compose は `DATABASE_URL` をDBコンテナ宛てに生成し、コンテナ内の `PORT=8000`、`BIND_ADDR=0.0.0.0` を設定します。`.env` の通常起動用 `DATABASE_URL`、`PORT`、`BIND_ADDR` は使用しません。

アプリのホスト側ポートは `127.0.0.1` のみに公開し、DBのポートは公開しません。データは名前付きボリューム `pgdata` に保存され、`docker compose down` で停止しても残ります。`docker compose down -v` はDBデータも削除します。

Windows では Docker Desktop の「Start Docker Desktop when you sign in to your computer」を有効にしてください。再起動後の復旧にはWindowsへのサインインとDocker Desktopの起動が必要です。Linux ではDockerサービスを自動起動に設定します。Docker起動後、コンテナは `restart: unless-stopped` で復旧します。PCがスリープしないよう電源設定も確認してください。

### Cloudflare Tunnel 経由で公開する場合

Cloudflare でトークンを使うTunnelと公開ホスト名を作成し、接続先サービスを `http://app:8000` に設定します。`.env` の `TUNNEL_TOKEN` にトークン、`BASE_URL` に来場者がアクセスする公開URL（例: `https://tickets.example.com`）を設定して起動します。

```sh
docker compose --profile tunnel up -d
```

`cloudflared` はこのプロファイルを指定したときだけ起動します。設定は [Cloudflare Tunnel の公式手順](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/get-started/create-remote-tunnel/) を参照してください。コンテナは [2026.10.7](https://github.com/cloudflare/cloudflared/releases/tag/2026.10.7) に固定しています。

### DBのバックアップと復元

バックアップは起動中のDBから取得できます。次のコマンドはDBコンテナ内の `POSTGRES_USER` / `POSTGRES_DB` を使います。Windows では文字コードの変換を避けるため、これらのリダイレクトはコマンドプロンプト（cmd.exe）で実行してください。

```sh
docker compose exec -T db sh -c 'pg_dump --clean --if-exists -U "$POSTGRES_USER" -d "$POSTGRES_DB"' > backup.sql
```

Windows（cmd.exe）では次を使います。

```bat
docker compose exec -T db sh -c "pg_dump --clean --if-exists -U $POSTGRES_USER -d $POSTGRES_DB" > backup.sql
```

復元は現在のDBの内容をバックアップ時点に戻します。アプリを止めてから実行し、成功後に再起動します（DBは起動したままにします）。

```sh
docker compose stop app
docker compose exec -T db sh -c 'psql -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB"' < backup.sql
docker compose start app
```

Windows（cmd.exe）では復元コマンドを次に置き換えます。

```bat
docker compose exec -T db sh -c "psql -v ON_ERROR_STOP=1 -U $POSTGRES_USER -d $POSTGRES_DB" < backup.sql
```

#### Raspberry Pi などで Render の DB を定期バックアップする

`scripts/backup.sh` を使うと、Render などの外部 PostgreSQL を `pg_dump` + `gzip` で定期的にダンプし、世代管理（既定で最新300件保持）できます。

1. 接続先 DB サーバー以上のメジャーバージョンの PostgreSQL クライアント（`pg_dump`）をインストールします。
2. `scripts/backup.env`（`.gitignore` 済み、`chmod 600` 推奨）を作成し、Render の External Database URL（SSL 必須のため末尾に `?sslmode=require` を付与）を設定します。

```sh
DATABASE_URL="postgres://user:pass@xxx.render.com/dbname?sslmode=require"
BACKUP_DIR="/home/pi/queueticket-backups"
KEEP=300
```

3. cron に登録して定期実行します（10分ごとの例）。

```cron
*/10 * * * * /path/to/scripts/backup.sh >> $HOME/queueticket-backups/backup.log 2>&1
```

スクリプトの動作テストは `bash tests/backup_test.sh` で実行できます。復元時はアプリケーション（Render の Web Service 等）を停止してから次を実行します。

```sh
gunzip -c /path/to/queueticket-YYYYmmdd-HHMMSS.sql.gz | psql -v ON_ERROR_STOP=1 "$DATABASE_URL"
```

### Render で同じ Dockerfile を使う場合

Dockerランタイムでこのリポジトリをデプロイし、外部PostgreSQLへの `DATABASE_URL`、`ADMIN_PASSWORD`、公開URLの `BASE_URL` を環境変数として設定します。アプリはRenderから渡される `PORT` を読み取り、`0.0.0.0` で待ち受けます。HTML・マイグレーション・JavaScriptはビルド時にバイナリへ埋め込まれるため、実行用イメージに別途配置する必要はありません。

## 📱 使い方

### 管理者メニューへのアクセス
*   URL: `BASE_URL` で設定したURLの `/admin`
*   初回アクセス時に認証が求められます。
    *   **ユーザー名**: `admin`
    *   **パスワード**: `ADMIN_PASSWORD` で設定した値

### 各画面の役割
*   **発券画面 (`/admin/front`)**: 受付担当者が使用します。人数を入力して発券し、QRコードをお客様に提示してください。
*   **呼び出し管理 (`/admin/call`)**: 誘導・案内担当者が使用します。順番が来たら呼び出し、案内が終わったら「完了する」を押してください。
*   **QRコード読み取り画面 (`/admin/display/front`)**: 受付のQRコード提示用端末に表示します。発券直後のQRコード・整理番号や現在の待ち状況を大きく表示します。
*   **プロジェクター投影画面 (`/admin/display/screen`)**: 会場内のスクリーン等に表示します。PV動画と現在の呼び出し状況・新規呼び出しポップアップを表示します。
*   **管理メニュー (`/admin`)**: CSVのダウンロードや、データの全消去（リセット）が行えます。

## 🎪 当日の運用ガイド

### 役割と端末

どの端末もブラウザでURLを開いてBasic認証にログインするだけで利用でき、専用ソフトのインストールは不要です（素の Windows + Chrome / Edge 等でそのまま動作します）。

| 役割 | 推奨端末 | 開く画面 |
| :--- | :--- | :--- |
| 受付（発券操作） | 発券用PC | `/admin/front` |
| QRコード読み取り画面 | 任意のPC・タブレットのブラウザ | `/admin/display/front` |
| 場内スタッフ（誘導） | スマホ・タブレット | `/admin/call` |
| プロジェクター投影画面 | ノートPC（プロジェクター接続） | `/admin/display/screen` |

### 呼び出しの流れ
1. 席が空いたら、まず **「🔙 戻り済み」** にカードがあれば最優先で案内し、「完了する」を押します。
2. 「戻り済み」がいなければ、画面上部の **「📢 次の番号を呼ぶ」** を押します（待機中の最も若い番号が自動で呼ばれます）。
3. 来場した組を案内したら **「✅ 完了する」** を押します。
4. 呼び出しから10分が経過すると、スタッフの操作なしで自動的に **「⏰ 未着」** へ移ります。呼出キャンセル機能はなく、来なかった番号はそのまま未着として残します。
5. 未着の来場者が戻ってきたら、来場者自身のスマホ画面の「会場に戻りました」ボタン、またはスタッフ画面の **「🙋 戻り確認」** ボタンを押すことで「戻り済み」に移ります。
6. グループの一部だけ席に案内できるときは **「👥 一部案内」** を押し、案内した人数をタップします。番号は1つのまま残りの人数が表示され、残りの方は次の空席で最優先になります。全員を案内したら自動で完了します（残り全員を一度に案内する場合は「完了する」でも構いません）。人数の取り消し機能はないため、押し間違えに注意してください。

### 来場者への案内文の例
> 「順番になってから10分以内に会場までお越しください。過ぎてしまった場合も、戻られたらこの画面のボタンでお知らせいただければ優先してご案内します」

### 表示用 Windows 端末の準備チェックリスト
*   AC電源に接続していること。
*   Windowsの電源とスリープ設定で、画面オフ・スリープをともに「なし」にしていること。
*   Windows Update を一時停止していること。
*   ブラウザを全画面表示（`F11` キー）にしていること。
*   プロジェクター投影画面（`/admin/display/screen`）は、ページ再読み込みで動画の選択が消えるため、「開始」を押すたびにローカルのPV動画ファイルを選び直すこと。
*   PV音声や呼び出しチャイムを鳴らす場合は、端末の音量・ミュート解除を確認すること。

### 開場前の注意（Render 無料プラン利用時）
Render の無料プランは15分間アクセスがないとスリープし、復帰に約1分かかります。開場前に表示端末（`/admin/display/front` や `/admin/display/screen`）を起動して画面を開いておくと、2〜3秒ごとの定期通信が行われるためスリープを防げます。

## 📊 ログデータとタイムゾーンについて

管理画面からダウンロードできるCSVファイル (`tickets_log.csv`) には、以下の時刻データが含まれます。

*   **発券時刻 (created_at)**
*   **呼出時刻 (called_at)**
*   **完了時刻 (completed_at)**
*   **戻り申告時刻 (returned_at)**
*   **初回表示時刻 (first_viewed_at)**
*   **案内済み人数 (seated_count)**（「完了する」で完了した場合は人数と同じ値）

### タイムゾーンと内部通番

CSVと発券履歴の時刻はサーバーのタイムゾーンによらず、明示的に **日本時間 (JST / UTC+9)** の `YYYY-MM-DD HH:MM:SS` で出力します。集計時の+9時間補正は不要です。CSVには内部通番 `seq` も含み、表示番号が循環した後も発券順に並びます。DBの時刻列は `TIMESTAMPTZ` です。

## 🔒 セキュリティ仕様
*   **認証**: 管理者エリア (`/admin/*`) はBasic認証により保護されています。
*   **CSRF対策**: 管理画面のPOSTリクエスト（発券、状態更新、QRコード読み取り画面の表示クリア、リセット）および来場者の戻り申告（`POST /guest/{id}/return`）の実行時、Origin（無い場合はReferer）のスキーム・ホスト・ポートが `BASE_URL` のoriginと完全一致するか検証します。ヘッダー無しや `null` は拒否します。
*   **DB操作**: `sqlx` のプレースホルダを使用し、SQLインジェクションを防いでいます。

## 📜 ライセンス
This project is for educational purposes.

---
*Created for University Festival 202X*

## 検証

`cargo build`、`cargo clippy --all-targets -- -D warnings`、`cargo test` でDB無しのビルド・検査・単体テストを実行できます。`TEST_DATABASE_URL` を指定するとDB結合テストも実行します（未設定時はスキップ）。結合テストはテストごとに専用スキーマを作成して削除するため、スキーマ作成権限が必要です。

定期更新の通信失敗・タイムアウト・表示復帰・通知のテストは `node tests/poll.cjs`、QRコード読み取り画面・プロジェクター投影画面のテストは `node tests/display.cjs`、一部案内のボトムシートのテストは `node tests/call.cjs` で実行できます（Node.jsのみ、追加パッケージ不要）。バックアップスクリプトのテストは `bash tests/backup_test.sh` で実行できます。

# 整理番号管理Webシステム v1.4.0

大学の文化祭やイベントでの混雑緩和を目的とした、Webベースの整理券発券・呼び出しシステムです。
Rust製の通常のバイナリとして起動し、PostgreSQLに接続して運用できます。

## 📖 概要

*   **出展者側**: PCやタブレットから整理番号を発券し、QRコードを表示。呼び出し管理画面からワンタップでステータス更新が可能。
*   **来場者側**: スマホでQRコードを読み取るだけで、専用アプリ不要で待ち状況を確認。順番が近づくと自動で画面が更新され通知されます。
*   **分析機能**: 発券・呼出・完了の時刻ログをCSVで出力することが可能。

## ✨ 主な機能

1.  **整理券発券 (受付)**
    *   人数を入力してワンクリックで発券。
    *   来場者読み取り用のQRコードを即座に生成・表示。
    *   1〜999の整理番号を循環し、未完了の番号を飛ばして発券。再送による二重発券を防止し、直近20件の履歴からQRを再表示できます。
2.  **呼び出し管理 (誘導)**
    *   スマホ・タブレットに最適化されたカード型レイアウト。
    *   待機中人数のリアルタイム把握。
    *   「呼び出し」「完了」のステータス変更。
3.  **来場者用マイページ**
    *   「あと何組待ちか」をリアルタイム表示（待ち状況に応じて15〜60秒間隔で自動更新）。
    *   呼び出し時に画面デザイン・タブタイトルが変化し、対応端末では振動で通知。音は画面のボタンで有効にした場合のみ鳴ります。通信失敗時は表示を保持し、再接続を試みます。
4.  **データ分析**
    *   全データをCSV形式でダウンロード可能。
    *   タイムスタンプ記録による回転率・ピークタイム分析。
5.  **セキュリティ**
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

### Render で同じ Dockerfile を使う場合

Dockerランタイムでこのリポジトリをデプロイし、外部PostgreSQLへの `DATABASE_URL`、`ADMIN_PASSWORD`、公開URLの `BASE_URL` を環境変数として設定します。アプリはRenderから渡される `PORT` を読み取り、`0.0.0.0` で待ち受けます。HTML・マイグレーション・JavaScriptはビルド時にバイナリへ埋め込まれるため、実行用イメージに別途配置する必要はありません。

## 📱 使い方

### 管理者メニューへのアクセス
*   URL: `BASE_URL` で設定したURLの `/admin`
*   初回アクセス時に認証が求められます。
    *   **ユーザー名**: `admin`
    *   **パスワード**: `ADMIN_PASSWORD` で設定した値

### 各画面の役割
*   **発券画面**: 受付担当者が使用します。人数を入力して発券し、QRコードをお客様に提示してください。
*   **呼び出し管理**: 誘導・案内担当者が使用します。順番が来たら「呼び出し」を、案内が終わったら「完了」を押してください。
*   **管理メニュー**: CSVのダウンロードや、データの全消去（リセット）が行えます。

## 📊 ログデータとタイムゾーンについて

管理画面からダウンロードできるCSVファイル (`tickets_log.csv`) には、以下の時刻データが含まれます。

*   **発券時刻 (created_at)**
*   **呼出時刻 (called_at)**
*   **完了時刻 (completed_at)**

### タイムゾーンと内部通番

CSVと発券履歴の時刻はサーバーのタイムゾーンによらず、明示的に **日本時間 (JST / UTC+9)** の `YYYY-MM-DD HH:MM:SS` で出力します。集計時の+9時間補正は不要です。CSVには内部通番 `seq` も含み、表示番号が循環した後も発券順に並びます。DBの時刻列は `TIMESTAMPTZ` です。

## 🔒 セキュリティ仕様
*   **認証**: 管理者エリア (`/admin/*`) はBasic認証により保護されています。
*   **CSRF対策**: POSTリクエスト（発券、状態更新、リセット）実行時、Origin（無い場合はReferer）のスキーム・ホスト・ポートが `BASE_URL` のoriginと完全一致するか検証します。ヘッダー無しや `null` は拒否します。
*   **DB操作**: `sqlx` のプレースホルダを使用し、SQLインジェクションを防いでいます。

## 📜 ライセンス
This project is for educational purposes.

---
*Created for University Festival 202X*

## 検証

`cargo build`、`cargo clippy --all-targets -- -D warnings`、`cargo test` でDB無しのビルド・検査・単体テストを実行できます。`TEST_DATABASE_URL` を指定するとDB結合テストも実行します（未設定時はスキップ）。結合テストはテストごとに専用スキーマを作成して削除するため、スキーマ作成権限が必要です。

定期更新の通信失敗・タイムアウト・表示復帰・通知のテストは、Node.jsがあれば `node tests/poll.cjs` で実行できます。追加パッケージは不要です。

# book_register

ISBNを入力すると、[OpenBD](https://openbd.jp/) から書誌情報を取得してNotionデータベースに自動登録するCLIツール。

公開用に最低限の構成を整えたリポジトリです。ローカルの `.env` はコミットせず、`.env.example` を雛形として使ってください。

## 必要環境

- Rust 1.94.1 以上
- Notion integration と登録先データベース
- OpenBD にアクセスできるネットワーク環境

## セットアップ

### 1. 環境変数の設定

`.env.example` をコピーして `.env` を作成し、値を入力する。

```sh
cp .env.example .env
```

- **NOTION_API_KEY**: [My integrations](https://www.notion.so/my-integrations) でインテグレーションを作成すると発行される。登録先のデータベースにそのインテグレーションを接続しておく必要がある。
- **NOTION_DATABASE_ID**: データベースをブラウザで開いたときのURL `https://www.notion.so/<ID>?v=...` の `<ID>` 部分（32文字）。

公開リポジトリで運用する場合:

- `.env` はコミットしない
- キーを画面共有や CI ログに出さない
- もし漏えいした可能性があれば Notion integration token を再発行する

### 2. Notionデータベースの構成

以下のプロパティを持つデータベースを用意する。

| プロパティ名 | 型       |
|------------|---------|
| 名前        | タイトル  |
| 代表著者    | テキスト  |
| 出版月      | テキスト  |
| 概要        | テキスト  |
| 購入年月    | 日付     |
| 価格        | 数値     |
| AmazonURL  | URL      |
| 画像        | URL      |

## Releaseバイナリの使い方

ビルド済みバイナリを [GitHub Releases](https://github.com/t-tkm/book-register/releases) からダウンロードして使う方法。

### 1. バイナリのダウンロード

Releases ページから OS に合ったアーカイブを取得する。

| OS      | ファイル名の例                                      |
|---------|---------------------------------------------------|
| macOS   | `book_register-v0.x.x-aarch64-apple-darwin.tar.gz`（Apple Silicon）<br>`book_register-v0.x.x-x86_64-apple-darwin.tar.gz`（Intel） |
| Linux   | `book_register-v0.x.x-x86_64-unknown-linux-gnu.tar.gz` |
| Windows | `book_register-v0.x.x-x86_64-pc-windows-msvc.zip` |

### 2. 展開と実行権限の付与（macOS / Linux）

```sh
tar xzf book_register-*.tar.gz
chmod +x book_register
```

### 3. macOS: 隔離属性の解除

macOS では Gatekeeper によりインターネット経由でダウンロードしたバイナリに隔離属性が付与され、そのままでは実行できない。以下のコマンドで属性を削除する。

```sh
xattr -d com.apple.quarantine book_register
```

> **確認方法**: `xattr book_register` を実行して何も表示されなければ解除済み。

### 4. パスの通った場所へ移動（任意）

```sh
mv book_register /usr/local/bin/
```

移動後はどのディレクトリからでも `book_register` コマンドとして呼び出せる。

---

## 使い方

```sh
# ISBNを直接指定（複数可）
book_register 9784478039670 9784798067278

# 購入日を指定（省略時は空で登録）
book_register -d 2026-03-15 9784478039670

# ISBNリストファイルを指定
book_register -f isbn_list.txt

# ドライラン（Notion には送らず、取得結果と送信予定 JSON を標準出力のみ）
book_register --dry-run 9784478039670
```

ISBNリストファイルは1行1ISBN。`#` で始まる行はコメントとして無視される。

対応形式: ISBN-13（ハイフン有無）・ISBN-10（ハイフン有無・X チェックディジット）

---

## アーキテクチャ

```
入力 (ISBN)
    │
    ▼
ISBN正規化
  ISBN-10 / ISBN-13・ハイフン有無を吸収 → ISBN-13 に統一
    │
    ▼
OpenBD API  https://api.openbd.jp/v1/get?isbn={ISBN-13}
  日本の書籍流通データベース（無料・登録不要）
    │
    ▼
Notion API  https://api.notion.com/v1/pages
  取得した書誌情報をページとして挿入
```

### フィールドマッピング

| Notion プロパティ | 型       | 取得元                                              |
|----------------|---------|-----------------------------------------------------|
| 名前            | タイトル  | `summary.title`                                     |
| 代表著者        | テキスト  | `summary.author`                                    |
| 出版月          | テキスト  | `summary.pubdate`（`YYYYMMDD` / `YYYYMM` → `YYYYMM` に変換）|
| 概要            | テキスト  | `onix.CollateralDetail.TextContent`（データがあれば取得、空の場合はNotionで手動入力） |
| 購入年月        | 日付     | `--date` オプションで指定（省略時は空。Notion GUIで入力）|
| 価格            | 数値     | `onix.ProductSupply.SupplyDetail.Price[].PriceAmount`（税抜）|
| AmazonURL      | URL      | ISBN-13 → ISBN-10 変換後、`https://www.amazon.co.jp/dp/{ISBN-10}/` を生成 |
| 画像            | URL      | `summary.cover`                                     |

---

## 開発

### ビルド

```sh
cargo build --release
```

### 実行

```sh
cargo run -- 9784478039670
```

### テスト

```sh
cargo test
```

### GitHub Actions

- `CI`: `main` / `master` への push と Pull Request で `cargo fmt --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test` を実行
- `Release`: `v*` タグ push で Linux/macOS/Windows 向けバイナリをビルドし、GitHub Release にアーカイブを添付

リリース例:

```sh
git tag v0.1.0
git push origin v0.1.0
```

## ライセンス

MIT License

# book_register

ISBNを入力すると、[OpenBD](https://openbd.jp/) から書誌情報を取得してNotionデータベースに自動登録するCLIツール。

## セットアップ

### 1. 環境変数の設定

`.env.example` をコピーして `.env` を作成し、値を入力する。

```sh
cp .env.example .env
```

- **NOTION_API_KEY**: [My integrations](https://www.notion.so/my-integrations) でインテグレーションを作成すると発行される。登録先のデータベースにそのインテグレーションを接続しておく必要がある。
- **NOTION_DATABASE_ID**: データベースをブラウザで開いたときのURL `https://www.notion.so/<ID>?v=...` の `<ID>` 部分（32文字）。

### 2. Notionデータベースの構成

以下のプロパティを持つデータベースを用意する。

| プロパティ名 | 型       |
|------------|---------|
| 名前        | タイトル  |
| 代表著者    | テキスト  |
| 発売日      | テキスト  |
| 概要        | テキスト  |
| 購入年月    | 日付     |
| 価格        | 数値     |
| AmazonURL  | URL      |
| 画像        | URL      |

## 使い方

```sh
# ISBNを直接指定（複数可）
book_register 9784478039670 9784798067278

# 購入日を指定（省略時は今日）
book_register -d 2026-03-15 9784478039670

# ISBNリストファイルを指定
book_register -f isbn_list.txt
```

ISBNリストファイルは1行1ISBN。`#` で始まる行はコメントとして無視される。

```
# 2024年購入分
9784478039670
978-4-7980-6727-8   # ハイフン付きも可
4873119464          # ISBN-10も可
```

対応形式: ISBN-13（ハイフン有無）・ISBN-10（ハイフン有無・X チェックディジット）

## 実行例

```
書籍DB登録ツール
   API Key: secret_xxxxxxxxxxxx...
   Database ID: xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
   購入年月日: 2026-04-03

📚 2件のISBNを処理します

============================================================

[1/2] 9784478039670
  📗 ゼロから作るDeep Learning
     著者: 斎藤,康毅
     定価: ￥3200（税抜）
     発売: 2016-09-24
  ✅ Notion登録完了

[2/2] 9784798067278
  📗 Python実践データ分析100本ノック
     著者: 下山,輝昌 松田,雄馬 三木,孝行
     定価: ￥2400（税抜）
     発売: 2022-06-09
  ✅ Notion登録完了

============================================================
📊 結果: 成功=2  スキップ=0  失敗=0  合計=2
```

---

## 付録

### アーキテクチャ

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
    │
    ▼
Notion データベース
```

### フィールドマッピング

| Notion プロパティ | 型       | 取得元                                              |
|----------------|---------|-----------------------------------------------------|
| 名前            | タイトル  | `summary.title`                                     |
| 代表著者        | テキスト  | `summary.author`                                    |
| 発売日          | テキスト  | `summary.pubdate`（`YYYYMMDD` → `YYYY-MM-DD` に変換）|
| 概要            | テキスト  | `onix.CollateralDetail.TextContent[].Text`（HTMLタグ除去・2000文字上限）|
| 購入年月        | 日付     | 実行日（`--date` オプションで上書き可）               |
| 価格            | 数値     | `onix.ProductSupply.SupplyDetail.Price[].PriceAmount`（税抜）|
| AmazonURL      | URL      | ISBN-13 → ISBN-10 変換後、`https://www.amazon.co.jp/dp/{ISBN-10}/` を生成 |
| 画像            | URL      | `summary.cover`                                     |

### 利用サービス

| サービス | 用途 | 認証 |
|---------|------|------|
| [OpenBD](https://openbd.jp/) | 書誌情報取得 | 不要 |
| [Notion API](https://developers.notion.com/) | データベース挿入 | `NOTION_API_KEY` |

---

## 開発

### ビルド

```sh
cargo build --release
```

Rust 1.70 以上が必要。

### テスト

```sh
cargo test
```

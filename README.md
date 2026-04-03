# book_register

ISBNを入力すると、[OpenBD](https://openbd.jp/) から書誌情報を取得してNotionデータベースに自動登録するCLIツール。

## 必要なもの

- Rust 1.70以上
- Notion APIキー
- Notion データベースID

## セットアップ

### 1. ビルド

```sh
cargo build --release
```

### 2. 環境変数の設定

`.env.example` をコピーして `.env` を作成し、値を入力する。

```sh
cp .env.example .env
```

```
# Notion API キー
# 取得先: https://www.notion.so/my-integrations
NOTION_API_KEY=secret_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx

# Notion データベース ID
# データベースURLの末尾32文字: https://www.notion.so/xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx?v=...
NOTION_DATABASE_ID=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
```

- **NOTION_API_KEY**: Notion の [My integrations](https://www.notion.so/my-integrations) でインテグレーションを作成すると発行される。登録先のデータベースにそのインテグレーションを接続しておく必要がある。
- **NOTION_DATABASE_ID**: データベースをブラウザで開いたときのURL `https://www.notion.so/<ID>?v=...` の `<ID>` 部分（32文字）。

### 3. Notionデータベースの構成

以下のプロパティを持つデータベースを用意する。

| プロパティ名 | 型         |
|------------|-----------|
| 名前        | タイトル    |
| 代表著者    | テキスト    |
| 発売日      | テキスト    |
| 概要        | テキスト    |
| 購入年月    | 日付       |
| 価格        | 数値       |
| AmazonURL  | URL        |
| 画像        | URL        |

## 使い方

```sh
# ISBNを直接指定（複数可）
./target/release/book_register 9784478039670
./target/release/book_register 9784478039670 9784798067278

# ISBNリストファイルを指定
./target/release/book_register -f isbn_list.txt
```

ISBNリストファイルは1行1ISBN。`#` で始まる行はコメントとして無視される。

```
# 2024年購入分
9784478039670
978-4-7980-6727-8   # ハイフン付きも可
4873119464          # ISBN-10も可
```

## 実行例

```
書籍DB登録ツール
   API Key: secret_xxxxxxxxxxxx...
   Database ID: xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx

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

## 対応ISBN形式

- ISBN-13（数字13桁）: `9784478039670`
- ISBN-13（ハイフン付き）: `978-4-47-803967-0`
- ISBN-10: `4478039674`
- ISBN-10（ハイフン付き）: `4-47-803967-4`

ISBN-10は内部でISBN-13に変換して処理する。

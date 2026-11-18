use std::{path::Path, process, time::Duration};

use anyhow::{Context, Result};
use chrono::NaiveDate;
use clap::Parser;
use regex::Regex;
use serde_json::{json, Map, Value};

// ============================================================
// CLI
// ============================================================

#[derive(Parser)]
#[command(
    about = "ISBN → OpenBD → Notion 書籍DB登録",
    after_help = "例: book_register 9784478039670 9784798067278\n    book_register --delete 9784478039670"
)]
struct Cli {
    /// ISBN-10 または ISBN-13（複数指定可）
    isbn: Vec<String>,

    /// ISBNリストファイル（1行1ISBN）
    #[arg(short, long)]
    file: Option<String>,

    /// 購入年月日（YYYY-MM-DD、省略時は空で登録）
    #[arg(short, long, value_name = "YYYY-MM-DD")]
    date: Option<String>,

    /// Notion に送信せず、OpenBDの取得結果と送信予定 JSON を標準出力のみに出す（--delete 時は削除対象の表示のみ）
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// 重複確認をスキップして強制登録する（--delete 時は削除確認を省略）
    #[arg(long)]
    force: bool,

    /// 指定 ISBN の登録済みエントリを削除する（Notion のゴミ箱へ移動）
    #[arg(long, conflicts_with = "date")]
    delete: bool,
}

// ============================================================
// Config
// ============================================================

struct Config {
    notion_api_key: String,
    notion_database_id: String,
    tax_rate: f64,
}

impl Config {
    /// `require_notion` が false のとき（登録のドライラン）は API キー等を省略可能にする
    fn from_env(require_notion: bool) -> Result<Self, String> {
        let tax_rate = std::env::var("TAX_RATE")
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(1.10);

        if !require_notion {
            return Ok(Self {
                notion_api_key: std::env::var("NOTION_API_KEY")
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                notion_database_id: std::env::var("NOTION_DATABASE_ID")
                    .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000000".to_string())
                    .trim()
                    .to_string(),
                tax_rate,
            });
        }
        let api_key = std::env::var("NOTION_API_KEY")
            .map(|s| s.trim().to_string())
            .map_err(|_| "❌ .env に NOTION_API_KEY を設定してください".to_string())?;
        let database_id = std::env::var("NOTION_DATABASE_ID")
            .map(|s| s.trim().to_string())
            .map_err(|_| "❌ .env に NOTION_DATABASE_ID を設定してください".to_string())?;
        Ok(Self {
            notion_api_key: api_key,
            notion_database_id: database_id,
            tax_rate,
        })
    }
}

// ============================================================
// Book
// ============================================================

struct Book {
    title: String,
    author: String,
    pubdate: String,
    cover: String,
    price: Option<u32>,
    description: String,
    isbn: String,
}

// ============================================================
// ISBN
// ============================================================

fn normalize_isbn(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .collect();
    match cleaned.len() {
        13 if cleaned.chars().all(|c| c.is_ascii_digit()) && is_valid_isbn13(&cleaned) => {
            Some(cleaned)
        }
        10 if cleaned[..9].chars().all(|c| c.is_ascii_digit())
            && cleaned
                .chars()
                .last()
                .is_some_and(|c| c.is_ascii_digit() || matches!(c, 'X' | 'x'))
            && is_valid_isbn10(&cleaned) =>
        {
            Some(isbn10_to_isbn13(&cleaned))
        }
        _ => None,
    }
}

fn is_valid_isbn10(isbn10: &str) -> bool {
    if isbn10.len() != 10 {
        return false;
    }

    let mut total = 0u32;
    for (i, c) in isbn10.chars().enumerate() {
        let digit = match c {
            '0'..='9' => c.to_digit(10).unwrap(),
            'X' | 'x' if i == 9 => 10,
            _ => return false,
        };
        total += digit * (10 - i as u32);
    }

    total.is_multiple_of(11)
}

fn is_valid_isbn13(isbn13: &str) -> bool {
    if isbn13.len() != 13 || !isbn13.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    let total: u32 = isbn13
        .chars()
        .take(12)
        .enumerate()
        .map(|(i, c)| c.to_digit(10).unwrap() * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    let check = (10 - total % 10) % 10;

    isbn13.chars().last().and_then(|c| c.to_digit(10)) == Some(check)
}

fn isbn10_to_isbn13(isbn10: &str) -> String {
    let base = format!("978{}", &isbn10[..9]);
    let total: u32 = base
        .chars()
        .enumerate()
        .map(|(i, c)| c.to_digit(10).unwrap() * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    let check = (10 - total % 10) % 10;
    format!("{base}{check}")
}

fn get_book_cover_url(isbn13: &str, openbd_cover: &str) -> String {
    if !openbd_cover.is_empty() {
        return openbd_cover.to_string();
    }

    // OpenBDに画像がない場合、Amazon商品画像を使用
    if let Some(isbn10) = isbn13_to_isbn10(isbn13) {
        format!(
            "https://images-na.ssl-images-amazon.com/images/P/{}.01.L.jpg",
            isbn10
        )
    } else {
        String::new()
    }
}

fn isbn13_to_isbn10(isbn13: &str) -> Option<String> {
    if isbn13.len() != 13 || !isbn13.starts_with("978") {
        return None;
    }
    let base = &isbn13[3..12];
    let total: u32 = base
        .chars()
        .enumerate()
        .map(|(i, c)| c.to_digit(10).unwrap() * (10 - i as u32))
        .sum();
    let check = (11 - total % 11) % 11;
    let check_char = if check == 10 {
        "X".to_string()
    } else {
        check.to_string()
    };
    Some(format!("{base}{check_char}"))
}

// ============================================================
// OpenBD
// ============================================================

/// OpenBD から書誌情報を取得する。
///
/// - `Ok(Some(book))`: 取得成功
/// - `Ok(None)`: OpenBD に該当データなし
/// - `Err(_)`: 通信エラー・HTTP エラー・レスポンス解析失敗
async fn fetch_book(client: &reqwest::Client, isbn13: &str, tax_rate: f64) -> Result<Option<Book>> {
    let url = format!("https://api.openbd.jp/v1/get?isbn={isbn13}");
    let data: Value = client
        .get(&url)
        .send()
        .await
        .context("通信に失敗")?
        .error_for_status()
        .context("HTTP エラー")?
        .json()
        .await
        .context("レスポンスの JSON 解析に失敗")?;
    parse_openbd_response(&data, tax_rate)
}

/// OpenBD のレスポンス（配列）を解釈する。該当なしは `null` 要素で返る。
fn parse_openbd_response(data: &Value, tax_rate: f64) -> Result<Option<Book>> {
    let entry = data
        .as_array()
        .context("想定外のレスポンス形式（配列ではない）")?
        .first()
        .context("想定外のレスポンス形式（空の配列）")?;
    if entry.is_null() {
        return Ok(None);
    }
    Ok(parse_openbd(entry, tax_rate))
}

fn parse_openbd(data: &Value, tax_rate: f64) -> Option<Book> {
    let summary = &data["summary"];
    let onix = &data["onix"];

    let title = summary["title"].as_str().unwrap_or("").to_string();
    if title.is_empty() {
        return None;
    }

    Some(Book {
        title,
        author: summary["author"].as_str().unwrap_or("").to_string(),
        pubdate: format_date(summary["pubdate"].as_str().unwrap_or("")),
        cover: get_book_cover_url(
            summary["isbn"].as_str().unwrap_or(""),
            summary["cover"].as_str().unwrap_or(""),
        ),
        isbn: summary["isbn"].as_str().unwrap_or("").to_string(),
        price: extract_price(onix, tax_rate),
        description: extract_description(onix),
    })
}

fn format_date(raw: &str) -> String {
    if raw.len() >= 6 && raw[..6].chars().all(|c| c.is_ascii_digit()) {
        raw[..6].to_string()
    } else {
        String::new()
    }
}

/// ONIX PriceType の優先順位と、税抜価格かどうか。
/// 上ほど優先する。`None` は PriceType が無いデータ（従来互換で税抜とみなす）。
const PRICE_TYPE_PRIORITY: &[(Option<&str>, bool)] = &[
    (Some("03"), true),  // 定価（税抜） Fixed retail price excluding tax
    (Some("01"), true),  // 希望小売価格（税抜） RRP excluding tax
    (Some("04"), false), // 定価（税込） Fixed retail price including tax
    (Some("02"), false), // 希望小売価格（税込） RRP including tax
    (None, true),        // PriceType 未設定
];

/// 税込価格（円）を返す。
///
/// - `PRICE_TYPE_PRIORITY` の順に価格を選ぶ。特価（11 など）や上記以外の種別は使わない
/// - `CurrencyCode` が指定されていて `JPY` 以外のものは除外する
/// - 税抜価格には `tax_rate` を掛け、1円未満は四捨五入する。税込価格はそのまま使う
fn extract_price(onix: &Value, tax_rate: f64) -> Option<u32> {
    let prices = &onix["ProductSupply"]["SupplyDetail"]["Price"];
    let list: Vec<&Value> = match prices {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![prices],
        _ => return None,
    };
    let candidates: Vec<(Option<&str>, f64)> = list
        .iter()
        .filter(|p| p["CurrencyCode"].as_str().is_none_or(|c| c == "JPY"))
        .filter_map(|p| {
            let amount = p["PriceAmount"].as_str()?.trim().parse::<f64>().ok()?;
            (amount.is_finite() && amount >= 0.0).then_some((p["PriceType"].as_str(), amount))
        })
        .collect();
    PRICE_TYPE_PRIORITY.iter().find_map(|(ty, excl_tax)| {
        let (_, amount) = candidates.iter().find(|(t, _)| t == ty)?;
        let price = if *excl_tax {
            amount * tax_rate
        } else {
            *amount
        };
        Some(price.round() as u32)
    })
}

fn extract_description(onix: &Value) -> String {
    let texts = &onix["CollateralDetail"]["TextContent"];
    let list: Vec<&Value> = match texts {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![texts],
        _ => return String::new(),
    };
    let html_tag = Regex::new(r"<[^>]+>").unwrap();
    list.iter()
        .find_map(|t| {
            let text = t["Text"].as_str()?;
            if text.is_empty() {
                return None;
            }
            let clean = html_tag.replace_all(text, "").trim().to_string();
            Some(clean.chars().take(2000).collect())
        })
        .unwrap_or_default()
}

// ============================================================
// Notion
// ============================================================

fn build_notion_payload(book: &Book, database_id: &str, purchase_date: &str) -> Value {
    let mut props = Map::new();
    props.insert(
        "名前".into(),
        json!({"title": [{"text": {"content": book.title}}]}),
    );
    if !purchase_date.is_empty() {
        props.insert("購入年月".into(), json!({"date": {"start": purchase_date}}));
    }

    for (key, value) in [
        ("代表著者", &book.author),
        ("出版月", &book.pubdate),
        ("概要", &book.description),
    ] {
        if !value.is_empty() {
            props.insert(
                key.into(),
                json!({"rich_text": [{"text": {"content": value}}]}),
            );
        }
    }
    if let Some(price) = book.price {
        props.insert("価格".into(), json!({"number": price}));
    }
    if let Some(isbn10) = isbn13_to_isbn10(&book.isbn) {
        props.insert(
            "AmazonURL".into(),
            json!({"url": format!("https://www.amazon.co.jp/dp/{isbn10}/")}),
        );
    }
    if !book.cover.is_empty() {
        props.insert(
            "画像".into(),
            json!({
                "files": [{
                    "name": "cover.jpg",
                    "external": {"url": book.cover}
                }]
            }),
        );
    }

    json!({ "parent": {"database_id": database_id}, "properties": props })
}

async fn find_duplicate_in_notion(
    client: &reqwest::Client,
    isbn13: &str,
    config: &Config,
) -> Result<Option<(String, Value)>> {
    // 979 始まりの ISBN は ISBN-10 に変換できず AmazonURL も登録しないため、照合しようがない
    let Some(isbn10) = isbn13_to_isbn10(isbn13) else {
        return Ok(None);
    };
    let amazon_url = format!("https://www.amazon.co.jp/dp/{isbn10}/");
    let query = json!({
        "filter": {
            "property": "AmazonURL",
            "url": {"equals": amazon_url}
        }
    });
    let data: Value = client
        .post(format!(
            "https://api.notion.com/v1/databases/{}/query",
            config.notion_database_id
        ))
        .header("Authorization", format!("Bearer {}", config.notion_api_key))
        .header("Notion-Version", "2022-06-28")
        .json(&query)
        .send()
        .await?
        .json()
        .await?;
    parse_duplicate_query_response(&data)
}

/// Notion のデータベース検索レスポンスから、最初に一致したページの ID とプロパティを取り出す。
/// エラーレスポンスや想定外の形は「該当なし」と区別するため Err にする。
fn parse_duplicate_query_response(data: &Value) -> Result<Option<(String, Value)>> {
    if data["object"].as_str() == Some("error") {
        let msg = data["message"].as_str().unwrap_or("Unknown error");
        anyhow::bail!("{msg}");
    }
    let Some(results) = data["results"].as_array() else {
        anyhow::bail!("検索結果の形式が不正です");
    };
    let Some(page) = results.first() else {
        return Ok(None);
    };
    let Some(page_id) = page["id"].as_str() else {
        anyhow::bail!("検索結果にページ ID がありません");
    };
    Ok(Some((page_id.to_string(), page["properties"].clone())))
}

fn print_existing_entry(props: &Value) {
    let title = props["名前"]["title"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|t| t["plain_text"].as_str())
        .unwrap_or("（不明）");
    let author = props["代表著者"]["rich_text"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|t| t["plain_text"].as_str())
        .unwrap_or("（不明）");
    let pubdate = props["出版月"]["rich_text"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|t| t["plain_text"].as_str())
        .unwrap_or("（不明）");
    let price = props["価格"]["number"]
        .as_f64()
        .map(|p| format!("￥{}", p as u64))
        .unwrap_or_else(|| "（不明）".to_string());
    let purchase = props["購入年月"]["date"]["start"]
        .as_str()
        .unwrap_or("（未登録）");

    println!("  ┌─ 既存エントリ ──────────────────────");
    println!("  │  書名:   {title}");
    println!("  │  著者:   {author}");
    println!("  │  出版月: {pubdate}");
    println!("  │  価格:   {price}");
    println!("  │  購入日: {purchase}");
    println!("  └───────────────────────────────────");
}

fn prompt_overwrite(title: &str) -> bool {
    prompt_yes_no(&format!(
        "上記エントリを「{title}」の新しいデータで上書きしますか？"
    ))
}

fn prompt_delete() -> bool {
    prompt_yes_no("上記エントリを削除（Notion のゴミ箱へ移動）しますか？")
}

fn prompt_yes_no(message: &str) -> bool {
    use std::io::{self, Write};
    print!("  ⚠️  {message} [y/N]: ");
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

async fn insert_to_notion(client: &reqwest::Client, payload: Value, config: &Config) -> Result<()> {
    let response: Value = client
        .post("https://api.notion.com/v1/pages")
        .header("Authorization", format!("Bearer {}", config.notion_api_key))
        .header("Notion-Version", "2022-06-28")
        .json(&payload)
        .send()
        .await?
        .json()
        .await?;

    if response["object"].as_str() == Some("error") {
        let msg = response["message"].as_str().unwrap_or("Unknown error");
        anyhow::bail!("{msg}");
    }
    Ok(())
}

async fn update_notion_page(
    client: &reqwest::Client,
    page_id: &str,
    payload: Value,
    config: &Config,
) -> Result<()> {
    let properties = payload["properties"].clone();
    let body = json!({ "properties": properties });
    let response: Value = client
        .patch(format!("https://api.notion.com/v1/pages/{page_id}"))
        .header("Authorization", format!("Bearer {}", config.notion_api_key))
        .header("Notion-Version", "2022-06-28")
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if response["object"].as_str() == Some("error") {
        let msg = response["message"].as_str().unwrap_or("Unknown error");
        anyhow::bail!("{msg}");
    }
    Ok(())
}

async fn archive_notion_page(
    client: &reqwest::Client,
    page_id: &str,
    config: &Config,
) -> Result<()> {
    let response: Value = client
        .patch(format!("https://api.notion.com/v1/pages/{page_id}"))
        .header("Authorization", format!("Bearer {}", config.notion_api_key))
        .header("Notion-Version", "2022-06-28")
        .json(&build_archive_payload())
        .send()
        .await?
        .json()
        .await?;

    if response["object"].as_str() == Some("error") {
        let msg = response["message"].as_str().unwrap_or("Unknown error");
        anyhow::bail!("{msg}");
    }
    Ok(())
}

fn build_archive_payload() -> Value {
    json!({ "archived": true })
}

// ============================================================
// Main processing
// ============================================================

fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("HTTPクライアントの初期化に失敗")
}

async fn process_deletes(isbn_list: Vec<String>, config: &Config, dry_run: bool, force: bool) {
    let client = build_http_client();

    let total = isbn_list.len();
    let (mut success, mut skip, mut fail) = (0usize, 0usize, 0usize);

    if dry_run {
        println!(
            "\n🗑️  {total}件のISBNの削除対象を確認します（🔍 ドライラン: 削除は行いません）\n"
        );
    } else {
        println!("\n🗑️  {total}件のISBNを削除します\n");
    }
    println!("{}", "=".repeat(60));

    for (i, raw) in isbn_list.iter().enumerate() {
        println!("\n[{}/{total}] {raw}", i + 1);

        let Some(isbn13) = normalize_isbn(raw) else {
            println!("  ⚠️  ISBN形式不正 — スキップ");
            skip += 1;
            continue;
        };

        let (page_id, props) = match find_duplicate_in_notion(&client, &isbn13, config).await {
            Ok(Some(found)) => found,
            Ok(None) => {
                println!("  ⚠️  Notion に該当エントリなし (ISBN: {isbn13}) — スキップ");
                skip += 1;
                continue;
            }
            Err(e) => {
                println!("  ❌ Notion 検索失敗: {e}");
                fail += 1;
                tokio::time::sleep(Duration::from_millis(400)).await;
                continue;
            }
        };

        print_existing_entry(&props);

        if dry_run {
            println!("  🔍 ドライラン: 削除をスキップ（page_id: {page_id}）");
            success += 1;
            continue;
        }

        if !force && !prompt_delete() {
            println!("  ⏭️  スキップ（削除キャンセル）");
            skip += 1;
            continue;
        }

        match archive_notion_page(&client, &page_id, config).await {
            Ok(()) => {
                println!("  ✅ 削除完了（Notion のゴミ箱から復元できます）");
                success += 1;
            }
            Err(e) => {
                println!("  ❌ Notion APIエラー: {e}");
                fail += 1;
            }
        }
        // Notion APIレートリミット対策（3 req/sec）
        tokio::time::sleep(Duration::from_millis(400)).await;
    }

    println!("\n{}", "=".repeat(60));
    if dry_run {
        println!(
            "📊 結果（ドライラン）: 削除対象={success}  スキップ={skip}  失敗={fail}  合計={total}"
        );
    } else {
        println!("📊 結果: 削除={success}  スキップ={skip}  失敗={fail}  合計={total}");
    }
}

async fn process_isbns(
    isbn_list: Vec<String>,
    config: &Config,
    purchase_date: &str,
    dry_run: bool,
    force: bool,
) {
    let client = build_http_client();

    let total = isbn_list.len();
    let (mut success, mut skip, mut fail) = (0usize, 0usize, 0usize);

    if dry_run {
        println!("\n📚 {total}件のISBNを処理します（🔍 ドライラン: Notion には接続しません）\n");
    } else {
        println!("\n📚 {total}件のISBNを処理します\n");
    }
    println!("{}", "=".repeat(60));

    for (i, raw) in isbn_list.iter().enumerate() {
        println!("\n[{}/{total}] {raw}", i + 1);

        let Some(isbn13) = normalize_isbn(raw) else {
            println!("  ⚠️  ISBN形式不正 — スキップ");
            skip += 1;
            continue;
        };

        let book = match fetch_book(&client, &isbn13, config.tax_rate).await {
            Ok(Some(book)) => book,
            Ok(None) => {
                println!("  ⚠️  OpenBDにデータなし (ISBN: {isbn13}) — スキップ");
                skip += 1;
                continue;
            }
            Err(e) => {
                println!("  ❌ OpenBD取得エラー (ISBN: {isbn13}): {e:#}");
                fail += 1;
                continue;
            }
        };

        println!("  📗 {}", book.title);
        println!("     著者: {}", book.author);
        if let Some(price) = book.price {
            println!("     定価: ￥{price}（税込）");
        }
        if !book.pubdate.is_empty() {
            println!("     発売: {}", book.pubdate);
        }

        if dry_run {
            if !book.isbn.is_empty() {
                println!("     ISBN: {}", book.isbn);
            }
            if !book.cover.is_empty() {
                println!("     表紙: {}", book.cover);
            }
            if !book.description.is_empty() {
                let preview: String = book.description.chars().take(400).collect();
                let ellipsis = if book.description.chars().count() > 400 {
                    "…"
                } else {
                    ""
                };
                println!("     概要: {preview}{ellipsis}");
            } else {
                println!("     概要: （データなし・手動入力用）");
            }
        }

        let payload = build_notion_payload(&book, &config.notion_database_id, purchase_date);
        if dry_run {
            println!("  🔍 ドライラン: Notion 送信をスキップ");
            match serde_json::to_string_pretty(&payload) {
                Ok(json) => println!("{json}"),
                Err(e) => println!("  ⚠️ JSON 表示エラー: {e}"),
            }
            success += 1;
        } else {
            let duplicate = if force {
                None
            } else {
                match find_duplicate_in_notion(&client, &isbn13, config).await {
                    Ok(found) => found,
                    Err(e) => {
                        println!("  ❌ 重複チェック失敗（登録を中止）: {e}");
                        fail += 1;
                        tokio::time::sleep(Duration::from_millis(400)).await;
                        continue;
                    }
                }
            };

            let result = match duplicate {
                Some((ref page_id, ref existing_props)) => {
                    print_existing_entry(existing_props);
                    if prompt_overwrite(&book.title) {
                        update_notion_page(&client, page_id, payload, config)
                            .await
                            .map(|_| true)
                    } else {
                        println!("  ⏭️  スキップ（重複登録キャンセル）");
                        skip += 1;
                        continue;
                    }
                }
                None => insert_to_notion(&client, payload, config)
                    .await
                    .map(|_| false),
            };

            match result {
                Ok(updated) => {
                    if updated {
                        println!("  ✅ Notion更新完了");
                    } else {
                        println!("  ✅ Notion登録完了");
                    }
                    success += 1;
                }
                Err(e) => {
                    println!("  ❌ Notion APIエラー: {e}");
                    fail += 1;
                }
            }
            // Notion APIレートリミット対策（3 req/sec）
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    println!("\n{}", "=".repeat(60));
    if dry_run {
        println!(
            "📊 結果（ドライラン）: 取得表示={success}  スキップ={skip}  失敗={fail}  合計={total}"
        );
    } else {
        println!("📊 結果: 成功={success}  スキップ={skip}  失敗={fail}  合計={total}");
    }
}

fn validate_purchase_date(raw: &str) -> Result<(), String> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map(|_| ())
        .map_err(|_| "❌ --date は YYYY-MM-DD 形式の実在する日付を指定してください".to_string())
}

fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return "(未設定)".to_string();
    }

    let visible = value.chars().count().min(4);
    let suffix: String = value
        .chars()
        .rev()
        .take(visible)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("***{suffix}")
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    let mut isbn_list = Vec::new();

    if let Some(file_path) = &cli.file {
        let path = Path::new(file_path);
        if !path.exists() {
            eprintln!("❌ ファイルが見つかりません: {file_path}");
            process::exit(1);
        }
        let content = std::fs::read_to_string(path).expect("ファイルの読み込みに失敗");
        for line in content.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                isbn_list.push(line.to_string());
            }
        }
    }

    isbn_list.extend(cli.isbn);

    if isbn_list.is_empty() {
        use clap::CommandFactory;
        Cli::command().print_help().unwrap();
        process::exit(0);
    }

    // 削除モードはドライランでも対象検索のため Notion に接続する
    let config = match Config::from_env(!cli.dry_run || cli.delete) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            process::exit(1);
        }
    };

    let purchase_date = cli.date.unwrap_or_default();
    if !purchase_date.is_empty() {
        if let Err(e) = validate_purchase_date(&purchase_date) {
            eprintln!("{e}");
            process::exit(1);
        }
    }

    if cli.delete {
        println!("書籍DB登録ツール（削除モード）");
        if cli.dry_run {
            println!("   モード: ドライラン（対象の検索のみ行い、削除はしません）");
        }
        println!("   API Key: {}", mask_secret(&config.notion_api_key));
        println!(
            "   Database ID: {}",
            mask_secret(&config.notion_database_id)
        );
        process_deletes(isbn_list, &config, cli.dry_run, cli.force).await;
        return;
    }

    println!("書籍DB登録ツール");
    if cli.dry_run {
        println!("   モード: ドライラン（Notion API には接続しません）");
        if config.notion_api_key.is_empty() {
            println!("   NOTION_API_KEY: （未設定・ペイロード表示用に省略可）");
        } else {
            println!("   NOTION_API_KEY: {}", mask_secret(&config.notion_api_key));
        }
        println!(
            "   NOTION_DATABASE_ID: {}{}",
            mask_secret(&config.notion_database_id),
            if std::env::var("NOTION_DATABASE_ID").is_err() {
                "（未設定時はダミーIDで JSON を生成）"
            } else {
                ""
            }
        );
    } else {
        println!("   API Key: {}", mask_secret(&config.notion_api_key));
        println!(
            "   Database ID: {}",
            mask_secret(&config.notion_database_id)
        );
    }
    println!("   購入年月日: {purchase_date}");

    process_isbns(isbn_list, &config, &purchase_date, cli.dry_run, cli.force).await;
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_isbn13_digits_only() {
        assert_eq!(
            normalize_isbn("9784478039670"),
            Some("9784478039670".into())
        );
    }

    #[test]
    fn normalize_isbn10_converts_to_isbn13() {
        assert_eq!(normalize_isbn("4478039674"), Some("9784478039670".into()));
    }

    #[test]
    fn normalize_isbn_rejects_invalid_check_digit() {
        assert_eq!(normalize_isbn("9784478039671"), None);
        assert_eq!(normalize_isbn("4478039675"), None);
    }

    #[test]
    fn isbn10_to_isbn13_roundtrip() {
        let isbn10 = "4478039674";
        let isbn13 = isbn10_to_isbn13(isbn10);
        assert_eq!(isbn13, "9784478039670");
        assert_eq!(isbn13_to_isbn10(&isbn13).unwrap(), isbn10);
    }

    #[test]
    fn extract_description_strips_html_tags() {
        let onix = json!({
            "CollateralDetail": { "TextContent": {"Text": "<p>紹介<br/>文</p>"} }
        });
        assert_eq!(extract_description(&onix), "紹介文");
    }

    fn onix_prices(prices: Value) -> Value {
        json!({ "ProductSupply": { "SupplyDetail": { "Price": prices } } })
    }

    #[test]
    fn extract_price_prefers_fixed_price_excluding_tax() {
        let onix = onix_prices(json!([
            {"PriceType": "11", "CurrencyCode": "JPY", "PriceAmount": "980"},
            {"PriceType": "03", "CurrencyCode": "JPY", "PriceAmount": "1500"},
        ]));
        assert_eq!(extract_price(&onix, 1.10), Some(1650));
    }

    #[test]
    fn extract_price_uses_tax_included_price_as_is() {
        let onix = onix_prices(json!({"PriceType": "04", "PriceAmount": "1650"}));
        assert_eq!(extract_price(&onix, 1.10), Some(1650));
    }

    #[test]
    fn extract_price_without_price_type_is_treated_as_excluding_tax() {
        let onix = onix_prices(json!({"PriceAmount": "1000"}));
        assert_eq!(extract_price(&onix, 1.10), Some(1100));
    }

    #[test]
    fn extract_price_ignores_special_and_foreign_prices() {
        let onix = onix_prices(json!([
            {"PriceType": "11", "PriceAmount": "980"},
            {"PriceType": "03", "CurrencyCode": "USD", "PriceAmount": "20"},
        ]));
        assert_eq!(extract_price(&onix, 1.10), None);
    }

    #[test]
    fn extract_price_rounds_half_up() {
        // 1234 * 1.10 = 1357.4 → 1357, 1235 * 1.10 = 1358.5 → 1359
        let onix = onix_prices(json!({"PriceType": "03", "PriceAmount": "1234"}));
        assert_eq!(extract_price(&onix, 1.10), Some(1357));
        let onix = onix_prices(json!({"PriceType": "03", "PriceAmount": "1235"}));
        assert_eq!(extract_price(&onix, 1.10), Some(1359));
    }

    #[test]
    fn extract_price_returns_none_without_price() {
        assert_eq!(extract_price(&json!({}), 1.10), None);
    }

    #[test]
    fn duplicate_response_with_match_returns_page() {
        let data = json!({
            "object": "list",
            "results": [{"id": "page-1", "properties": {"名前": {}}}]
        });
        let (id, props) = parse_duplicate_query_response(&data).unwrap().unwrap();
        assert_eq!(id, "page-1");
        assert!(props.get("名前").is_some());
    }

    #[test]
    fn duplicate_response_without_match_returns_none() {
        let data = json!({"object": "list", "results": []});
        assert!(parse_duplicate_query_response(&data).unwrap().is_none());
    }

    #[test]
    fn duplicate_response_error_is_err() {
        let data = json!({
            "object": "error",
            "status": 429,
            "code": "rate_limited",
            "message": "Rate limited"
        });
        let err = parse_duplicate_query_response(&data).unwrap_err();
        assert_eq!(err.to_string(), "Rate limited");
    }

    #[test]
    fn duplicate_response_unexpected_shape_is_err() {
        assert!(parse_duplicate_query_response(&json!({})).is_err());
    }

    #[test]
    fn parse_openbd_response_null_entry_is_none() {
        let data = json!([null]);
        assert!(matches!(parse_openbd_response(&data, 1.1), Ok(None)));
    }

    #[test]
    fn parse_openbd_response_returns_book() {
        let data = json!([{
            "summary": {"title": "テスト本", "isbn": "9784478039670", "pubdate": "20170101"},
            "onix": {}
        }]);
        let book = parse_openbd_response(&data, 1.1).unwrap().unwrap();
        assert_eq!(book.title, "テスト本");
        assert_eq!(book.pubdate, "201701");
    }

    #[test]
    fn parse_openbd_response_rejects_unexpected_format() {
        assert!(parse_openbd_response(&json!({"error": "x"}), 1.1).is_err());
        assert!(parse_openbd_response(&json!([]), 1.1).is_err());
    }

    #[test]
    fn validate_purchase_date_accepts_real_date() {
        assert!(validate_purchase_date("2026-03-15").is_ok());
    }

    #[test]
    fn archive_payload_sets_archived_true() {
        assert_eq!(build_archive_payload(), json!({"archived": true}));
    }

    #[test]
    fn cli_delete_conflicts_with_date() {
        assert!(Cli::try_parse_from([
            "book_register",
            "--delete",
            "-d",
            "2026-03-15",
            "9784478039670"
        ])
        .is_err());
        let cli =
            Cli::try_parse_from(["book_register", "--delete", "--force", "9784478039670"]).unwrap();
        assert!(cli.delete && cli.force);
        assert_eq!(cli.isbn, vec!["9784478039670"]);
    }

    #[test]
    fn validate_purchase_date_rejects_invalid_date() {
        assert!(validate_purchase_date("2026-02-30").is_err());
    }

    // ---------- build_notion_payload ----------

    fn full_book() -> Book {
        Book {
            title: "テスト書籍".into(),
            author: "山田太郎".into(),
            pubdate: "202403".into(),
            cover: "https://cover.openbd.jp/9784478039670.jpg".into(),
            price: Some(1650),
            description: "概要テキスト".into(),
            isbn: "9784478039670".into(),
        }
    }

    fn empty_book() -> Book {
        Book {
            title: "タイトルのみ".into(),
            author: String::new(),
            pubdate: String::new(),
            cover: String::new(),
            price: None,
            description: String::new(),
            // 979 始まりは ISBN-10 に変換できない
            isbn: "9791234567896".into(),
        }
    }

    #[test]
    fn build_notion_payload_includes_all_fields() {
        let payload = build_notion_payload(&full_book(), "db-id", "2026-03-15");
        let props = &payload["properties"];

        assert_eq!(payload["parent"]["database_id"], "db-id");
        assert_eq!(props["名前"]["title"][0]["text"]["content"], "テスト書籍");
        assert_eq!(props["購入年月"]["date"]["start"], "2026-03-15");
        assert_eq!(
            props["代表著者"]["rich_text"][0]["text"]["content"],
            "山田太郎"
        );
        assert_eq!(props["出版月"]["rich_text"][0]["text"]["content"], "202403");
        assert_eq!(
            props["概要"]["rich_text"][0]["text"]["content"],
            "概要テキスト"
        );
        assert_eq!(props["価格"]["number"], 1650);
        assert_eq!(
            props["AmazonURL"]["url"],
            "https://www.amazon.co.jp/dp/4478039674/"
        );
        assert_eq!(props["画像"]["files"][0]["name"], "cover.jpg");
        assert_eq!(
            props["画像"]["files"][0]["external"]["url"],
            "https://cover.openbd.jp/9784478039670.jpg"
        );
    }

    #[test]
    fn build_notion_payload_omits_empty_fields() {
        let payload = build_notion_payload(&empty_book(), "db-id", "");
        let props = payload["properties"].as_object().unwrap();

        let keys: Vec<&str> = props.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["名前"]);
    }

    #[test]
    fn build_notion_payload_purchase_date_is_optional() {
        let with_date = build_notion_payload(&full_book(), "db-id", "2026-03-15");
        assert!(with_date["properties"].get("購入年月").is_some());

        let without_date = build_notion_payload(&full_book(), "db-id", "");
        assert!(without_date["properties"].get("購入年月").is_none());
    }

    // ---------- extract_price ----------
    // PriceType による選択は #18 で扱うため、ここでは現行仕様（最初の数値 PriceAmount）を検証する

    fn onix_with_price(price: Value) -> Value {
        json!({ "ProductSupply": { "SupplyDetail": { "Price": price } } })
    }

    #[test]
    fn extract_price_from_array_uses_first_numeric_amount() {
        let onix = onix_with_price(json!([
            {"PriceType": "01"},
            {"PriceAmount": "abc"},
            {"PriceAmount": "1500"},
            {"PriceAmount": "9999"}
        ]));
        assert_eq!(extract_price(&onix, 1.10), Some(1650));
    }

    #[test]
    fn extract_price_from_object() {
        let onix = onix_with_price(json!({"PriceAmount": "2000"}));
        assert_eq!(extract_price(&onix, 1.10), Some(2200));
    }

    #[test]
    fn extract_price_applies_tax_rate() {
        let onix = onix_with_price(json!({"PriceAmount": "1000"}));
        assert_eq!(extract_price(&onix, 1.0), Some(1000));
        assert_eq!(extract_price(&onix, 1.08), Some(1080));
    }

    #[test]
    fn extract_price_rounds_to_nearest() {
        // 1001 * 1.10 = 1101.1 → 1101（切り下げ方向）
        assert_eq!(
            extract_price(&onix_with_price(json!({"PriceAmount": "1001"})), 1.10),
            Some(1101)
        );
        // 1235 * 1.08 = 1333.8 → 1334（切り上げ方向）
        assert_eq!(
            extract_price(&onix_with_price(json!({"PriceAmount": "1235"})), 1.08),
            Some(1334)
        );
        // 1005 * 1.10 = 1105.5 → 1106（0.5 は切り上げ）
        assert_eq!(
            extract_price(&onix_with_price(json!({"PriceAmount": "1005"})), 1.10),
            Some(1106)
        );
    }

    #[test]
    fn extract_price_returns_none_without_price() {
        assert_eq!(extract_price(&json!({}), 1.10), None);
        assert_eq!(extract_price(&onix_with_price(json!([])), 1.10), None);
        assert_eq!(
            extract_price(&onix_with_price(json!([{"PriceAmount": "abc"}])), 1.10),
            None
        );
    }

    // ---------- parse_openbd ----------
    // 著者名の整形は #19 で扱うため、ここでは著者名の検証は行わない

    #[test]
    fn parse_openbd_returns_none_without_title() {
        let missing = json!({ "summary": { "isbn": "9784478039670" }, "onix": {} });
        assert!(parse_openbd(&missing, 1.10).is_none());

        let empty = json!({ "summary": { "title": "", "isbn": "9784478039670" }, "onix": {} });
        assert!(parse_openbd(&empty, 1.10).is_none());
    }

    #[test]
    fn parse_openbd_builds_book() {
        let data = json!({
            "summary": {
                "title": "テスト書籍",
                "isbn": "9784478039670",
                "pubdate": "20240315",
                "cover": ""
            },
            "onix": onix_with_price(json!({"PriceAmount": "1500"}))
        });
        let book = parse_openbd(&data, 1.10).unwrap();

        assert_eq!(book.title, "テスト書籍");
        assert_eq!(book.isbn, "9784478039670");
        assert_eq!(book.pubdate, "202403");
        assert_eq!(book.price, Some(1650));
        // OpenBD に画像がない場合は Amazon 画像にフォールバック
        assert_eq!(
            book.cover,
            "https://images-na.ssl-images-amazon.com/images/P/4478039674.01.L.jpg"
        );
    }
}

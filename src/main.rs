use std::{path::Path, process, time::Duration};

use anyhow::Result;
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
    after_help = "例: book_register 9784478039670 9784798067278"
)]
struct Cli {
    /// ISBN-10 または ISBN-13（複数指定可）
    isbn: Vec<String>,

    /// ISBNリストファイル（1行1ISBN）
    #[arg(short, long)]
    file: Option<String>,

    /// 購入年月日（YYYY-MM-DD、省略時は今日）
    #[arg(short, long, value_name = "YYYY-MM-DD")]
    date: Option<String>,

    /// Notion に送信せず、OpenBDの取得結果と送信予定 JSON を標準出力のみに出す
    #[arg(long = "dry-run")]
    dry_run: bool,
}

// ============================================================
// Config
// ============================================================

struct Config {
    notion_api_key: String,
    notion_database_id: String,
}

impl Config {
    fn from_env(dry_run: bool) -> Result<Self, String> {
        if dry_run {
            return Ok(Self {
                notion_api_key: std::env::var("NOTION_API_KEY")
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                notion_database_id: std::env::var("NOTION_DATABASE_ID")
                    .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000000".to_string())
                    .trim()
                    .to_string(),
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

async fn fetch_book(client: &reqwest::Client, isbn13: &str) -> Option<Book> {
    let url = format!("https://api.openbd.jp/v1/get?isbn={isbn13}");
    let data: Value = client.get(&url).send().await.ok()?.json().await.ok()?;
    let entry = data.get(0)?;
    if entry.is_null() {
        return None;
    }
    parse_openbd(entry)
}

fn parse_openbd(data: &Value) -> Option<Book> {
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
        price: extract_price(onix),
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

fn extract_price(onix: &Value) -> Option<u32> {
    let prices = &onix["ProductSupply"]["SupplyDetail"]["Price"];
    let list: Vec<&Value> = match prices {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![prices],
        _ => return None,
    };
    list.iter()
        .find_map(|p| p["PriceAmount"].as_str()?.parse::<u32>().ok())
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
    props.insert("購入年月".into(), json!({"date": {"start": purchase_date}}));

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

// ============================================================
// Main processing
// ============================================================

async fn process_isbns(
    isbn_list: Vec<String>,
    config: &Config,
    purchase_date: &str,
    dry_run: bool,
) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("HTTPクライアントの初期化に失敗");

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

        let Some(book) = fetch_book(&client, &isbn13).await else {
            println!("  ⚠️  OpenBDにデータなし (ISBN: {isbn13}) — スキップ");
            skip += 1;
            continue;
        };

        println!("  📗 {}", book.title);
        println!("     著者: {}", book.author);
        if let Some(price) = book.price {
            println!("     定価: ￥{price}（税抜）");
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
            match insert_to_notion(&client, payload, config).await {
                Ok(()) => {
                    println!("  ✅ Notion登録完了");
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

    let config = match Config::from_env(cli.dry_run) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            process::exit(1);
        }
    };

    let purchase_date = match cli.date {
        Some(d) => d,
        None => {
            eprintln!("❌ --date を指定してください（例: --date 2026-04-19）");
            process::exit(1);
        }
    };
    if let Err(e) = validate_purchase_date(&purchase_date) {
        eprintln!("{e}");
        process::exit(1);
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

    process_isbns(isbn_list, &config, &purchase_date, cli.dry_run).await;
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

    #[test]
    fn validate_purchase_date_accepts_real_date() {
        assert!(validate_purchase_date("2026-03-15").is_ok());
    }

    #[test]
    fn validate_purchase_date_rejects_invalid_date() {
        assert!(validate_purchase_date("2026-02-30").is_err());
    }
}

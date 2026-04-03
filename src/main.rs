use std::{path::Path, process, time::Duration};

use anyhow::Result;
use chrono::Local;
use clap::Parser;
use regex::Regex;
use serde_json::{json, Map, Value};

// ============================================================
// CLI
// ============================================================

#[derive(Parser)]
#[command(
    about = "ISBN → OpenBD → Notion 書籍DB登録",
    after_help = "例: book_register 9784478039670"
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
}

// ============================================================
// Config
// ============================================================

struct Config {
    notion_api_key: String,
    notion_database_id: String,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("NOTION_API_KEY")
            .map_err(|_| "❌ .env に NOTION_API_KEY を設定してください".to_string())?;
        let database_id = std::env::var("NOTION_DATABASE_ID")
            .map_err(|_| "❌ .env に NOTION_DATABASE_ID を設定してください".to_string())?;
        Ok(Self { notion_api_key: api_key, notion_database_id: database_id })
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
    let cleaned: String = raw.trim().chars().filter(|c| !matches!(c, '-' | ' ')).collect();
    match cleaned.len() {
        13 if cleaned.chars().all(|c| c.is_ascii_digit()) => Some(cleaned),
        10 if cleaned[..9].chars().all(|c| c.is_ascii_digit())
            && cleaned.chars().last().map_or(false, |c| c.is_ascii_digit() || matches!(c, 'X' | 'x')) =>
        {
            Some(isbn10_to_isbn13(&cleaned))
        }
        _ => None,
    }
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
    let check_char = if check == 10 { "X".to_string() } else { check.to_string() };
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
        author:      summary["author"].as_str().unwrap_or("").to_string(),
        pubdate:     format_date(summary["pubdate"].as_str().unwrap_or("")),
        cover:       summary["cover"].as_str().unwrap_or("").to_string(),
        isbn:        summary["isbn"].as_str().unwrap_or("").to_string(),
        price:       extract_price(onix),
        description: extract_description(onix),
    })
}

fn format_date(raw: &str) -> String {
    if raw.len() == 8 && raw.chars().all(|c| c.is_ascii_digit()) {
        format!("{}-{}-{}", &raw[..4], &raw[4..6], &raw[6..8])
    } else {
        raw.to_string()
    }
}

fn extract_price(onix: &Value) -> Option<u32> {
    let prices = &onix["ProductSupply"]["SupplyDetail"]["Price"];
    let list: Vec<&Value> = match prices {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![prices],
        _ => return None,
    };
    list.iter().find_map(|p| {
        p["PriceAmount"].as_str()?.parse::<u32>().ok()
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
            if text.is_empty() { return None; }
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
    props.insert("名前".into(),   json!({"title": [{"text": {"content": book.title}}]}));
    props.insert("購入年月".into(), json!({"date": {"start": purchase_date}}));

    for (key, value) in [("代表著者", &book.author), ("発売日", &book.pubdate), ("概要", &book.description)] {
        if !value.is_empty() {
            props.insert(key.into(), json!({"rich_text": [{"text": {"content": value}}]}));
        }
    }
    if let Some(price) = book.price {
        props.insert("価格".into(), json!({"number": price}));
    }
    if let Some(isbn10) = isbn13_to_isbn10(&book.isbn) {
        props.insert("AmazonURL".into(), json!({"url": format!("https://www.amazon.co.jp/dp/{isbn10}/")}));
    }
    if !book.cover.is_empty() {
        props.insert("画像".into(), json!({"url": book.cover}));
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

async fn process_isbns(isbn_list: Vec<String>, config: &Config, purchase_date: &str) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("HTTPクライアントの初期化に失敗");

    let total = isbn_list.len();
    let (mut success, mut skip, mut fail) = (0usize, 0usize, 0usize);

    println!("\n📚 {total}件のISBNを処理します\n");
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

        let payload = build_notion_payload(&book, &config.notion_database_id, purchase_date);
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

    println!("\n{}", "=".repeat(60));
    println!("📊 結果: 成功={success}  スキップ={skip}  失敗={fail}  合計={total}");
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

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => { eprintln!("{e}"); process::exit(1); }
    };

    let purchase_date = cli.date.unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());

    println!("書籍DB登録ツール");
    println!("   API Key: {}...", &config.notion_api_key[..config.notion_api_key.len().min(20)]);
    println!("   Database ID: {}", config.notion_database_id);
    println!("   購入年月日: {purchase_date}");

    process_isbns(isbn_list, &config, &purchase_date).await;
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- normalize_isbn ---

    #[test]
    fn normalize_isbn13_digits_only() {
        assert_eq!(normalize_isbn("9784478039670"), Some("9784478039670".into()));
    }

    #[test]
    fn normalize_isbn13_with_hyphens() {
        assert_eq!(normalize_isbn("978-4-47-803967-0"), Some("9784478039670".into()));
    }

    #[test]
    fn normalize_isbn13_with_spaces() {
        assert_eq!(normalize_isbn("  9784478039670  "), Some("9784478039670".into()));
    }

    #[test]
    fn normalize_isbn10_converts_to_isbn13() {
        assert_eq!(normalize_isbn("4478039674"), Some("9784478039670".into()));
    }

    #[test]
    fn normalize_isbn10_with_hyphens() {
        assert_eq!(normalize_isbn("4-478-03967-4"), Some("9784478039670".into()));
    }

    #[test]
    fn normalize_isbn10_x_check_digit() {
        // ISBN-10 "080442957X" → ISBN-13 "9780804429573"
        assert_eq!(normalize_isbn("080442957X"), Some("9780804429573".into()));
        assert_eq!(normalize_isbn("080442957x"), Some("9780804429573".into()));
    }

    #[test]
    fn normalize_isbn_invalid_returns_none() {
        assert_eq!(normalize_isbn("123"), None);
        assert_eq!(normalize_isbn("abcdefghijk"), None);
        assert_eq!(normalize_isbn(""), None);
        assert_eq!(normalize_isbn("12345678901234"), None); // 14桁
    }

    // --- isbn10_to_isbn13 / isbn13_to_isbn10 ---

    #[test]
    fn isbn10_to_isbn13_roundtrip() {
        let isbn10 = "4478039674";
        let isbn13 = isbn10_to_isbn13(isbn10);
        assert_eq!(isbn13, "9784478039670");
        assert_eq!(isbn13_to_isbn10(&isbn13).unwrap(), isbn10);
    }

    #[test]
    fn isbn13_to_isbn10_x_check_digit() {
        assert_eq!(isbn13_to_isbn10("9780804429573").unwrap(), "080442957X");
    }

    #[test]
    fn isbn13_to_isbn10_non_978_returns_none() {
        assert_eq!(isbn13_to_isbn10("9794478039670"), None);
    }

    #[test]
    fn isbn13_to_isbn10_wrong_length_returns_none() {
        assert_eq!(isbn13_to_isbn10("978447803967"), None);
    }

    // --- format_date ---

    #[test]
    fn format_date_8_digits() {
        assert_eq!(format_date("20231213"), "2023-12-13");
    }

    #[test]
    fn format_date_6_digits_unchanged() {
        assert_eq!(format_date("202312"), "202312");
    }

    #[test]
    fn format_date_empty_unchanged() {
        assert_eq!(format_date(""), "");
    }

    // --- extract_price ---

    #[test]
    fn extract_price_single_object() {
        let onix = json!({
            "ProductSupply": { "SupplyDetail": { "Price": {"PriceAmount": "3200"} } }
        });
        assert_eq!(extract_price(&onix), Some(3200));
    }

    #[test]
    fn extract_price_array_returns_first() {
        let onix = json!({
            "ProductSupply": { "SupplyDetail": { "Price": [
                {"PriceAmount": "2800"},
                {"PriceAmount": "3080"}
            ]}}
        });
        assert_eq!(extract_price(&onix), Some(2800));
    }

    #[test]
    fn extract_price_missing_returns_none() {
        assert_eq!(extract_price(&json!({})), None);
    }

    // --- extract_description ---

    #[test]
    fn extract_description_single_object() {
        let onix = json!({
            "CollateralDetail": { "TextContent": {"Text": "本の紹介文です。"} }
        });
        assert_eq!(extract_description(&onix), "本の紹介文です。");
    }

    #[test]
    fn extract_description_strips_html_tags() {
        let onix = json!({
            "CollateralDetail": { "TextContent": {"Text": "<p>紹介<br/>文</p>"} }
        });
        assert_eq!(extract_description(&onix), "紹介文");
    }

    #[test]
    fn extract_description_array_skips_empty() {
        let onix = json!({
            "CollateralDetail": { "TextContent": [
                {"Text": ""},
                {"Text": "2番目のテキスト"}
            ]}
        });
        assert_eq!(extract_description(&onix), "2番目のテキスト");
    }

    #[test]
    fn extract_description_missing_returns_empty() {
        assert_eq!(extract_description(&json!({})), "");
    }

    #[test]
    fn extract_description_truncates_at_2000_chars() {
        let long_text = "あ".repeat(3000);
        let onix = json!({
            "CollateralDetail": { "TextContent": {"Text": long_text} }
        });
        assert_eq!(extract_description(&onix).chars().count(), 2000);
    }
}

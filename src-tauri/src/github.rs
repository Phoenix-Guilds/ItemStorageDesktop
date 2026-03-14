use base64::{engine::general_purpose, Engine as _};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde_json::json;
use sha1::{Digest, Sha1};

pub fn calculate_github_sha(content: &str) -> String {
    let mut hasher = Sha1::new();
    let header = format!("blob {}\0", content.len());
    hasher.update(header.as_bytes());
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn get_gh_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();

    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("ItemStorageBrowser-Manager"),
    );

    if !token.is_empty() {
        if let Ok(auth) = HeaderValue::from_str(&format!("token {}", token)) {
            headers.insert(AUTHORIZATION, auth);
        }
    }

    headers
}

pub async fn validate_token(token: &str) -> bool {
    let client = reqwest::Client::new();

    let resp = client
        .get("https://api.github.com/user")
        .headers(get_gh_headers(token))
        .send()
        .await;

    match resp {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    }
}

// Новая функция: получаем время последнего коммита файла (в Unix timestamp)
pub async fn get_remote_file_time(
    token: &str,
    path: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.github.com/repos/Phoenix-Guilds/ItemStorageBrowser/commits?path={}&sha=data&per_page=1",
        path
    );

    let resp = client
        .get(&url)
        .headers(get_gh_headers(token))
        .send()
        .await?;
    let commits: serde_json::Value = resp.json().await?;

    let date_str = commits[0]["commit"]["committer"]["date"]
        .as_str()
        .ok_or("Не удалось получить дату коммита")?;

    let datetime = chrono::DateTime::parse_from_rfc3339(date_str)?;
    Ok(datetime.timestamp() as u64)
}

pub async fn upload_to_github(
    token: &str,
    file_content: &str,
    path_in_repo: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.github.com/repos/Phoenix-Guilds/ItemStorageBrowser/contents/{}",
        path_in_repo
    );

    let get_url = format!("{}?ref=data", url);
    let resp = client
        .get(&get_url)
        .headers(get_gh_headers(token))
        .send()
        .await?;

    let mut remote_sha = String::new();
    if resp.status().is_success() {
        let json: serde_json::Value = resp.json().await?;
        remote_sha = json["sha"].as_str().unwrap_or("").to_string();
    }

    let b64_content = general_purpose::STANDARD.encode(file_content);
    let mut body = json!({
        "message": message,
        "content": b64_content,
        "branch": "data"
    });

    if !remote_sha.is_empty() {
        body.as_object_mut()
            .unwrap()
            .insert("sha".to_string(), json!(remote_sha));
    }

    let put_resp = client
        .put(&url)
        .headers(get_gh_headers(token))
        .json(&body)
        .send()
        .await?;

    if put_resp.status().is_success() {
        Ok(())
    } else {
        let err_json: serde_json::Value = put_resp.json().await?;
        Err(format!("GitHub Error: {}", err_json["message"]).into())
    }
}

pub async fn download_from_github(
    token: &str,
    path_in_repo: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.github.com/repos/Phoenix-Guilds/ItemStorageBrowser/contents/{}?ref=data",
        path_in_repo
    );

    let resp = client
        .get(&url)
        .headers(get_gh_headers(token))
        .send()
        .await?;

    if resp.status().is_success() {
        let json: serde_json::Value = resp.json().await?;
        let encoded_content = json["content"]
            .as_str()
            .ok_or("No content")?
            .replace("\n", "");
        let decoded_bytes = general_purpose::STANDARD.decode(encoded_content)?;
        Ok(String::from_utf8(decoded_bytes)?)
    } else {
        Err(format!("Ошибка скачивания: {}", resp.status()).into())
    }
}

pub async fn get_latest_release_version() -> Result<(String, String), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let url = "https://api.github.com/repos/Phoenix-Guilds/ItemStorageDesktop/releases/latest";

    // Для публичных GET запросов к релизам токен обычно не обязателен,
    // но USER_AGENT должен быть.
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("ItemStorageBrowser-Manager"),
    );

    let resp = client.get(url).headers(headers).send().await?;
    let json: serde_json::Value = resp.json().await?;

    let tag = json["tag_name"].as_str().unwrap_or("v0.0.0").to_string();
    let url = json["html_url"].as_str().unwrap_or("").to_string();

    Ok((tag, url))
}

use base64::{engine::general_purpose, Engine as _};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde_json::json;
use sha1::{Digest, Sha1};

// Вычисляет SHA-1 файла так же, как это делает GitHub (с префиксом blob)
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

    // 1. Получаем текущие данные файла из GitHub (ветка data)
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

        // СРАВНИВАЕМ: если SHA совпали, значит контент в репозитории идентичен локальному
        let local_sha = calculate_github_sha(file_content);
        if remote_sha == local_sha {
            println!("[GITHUB] Файл в репозитории уже актуален. Пропускаем.");
            return Ok(());
        }
    }

    // 2. Если SHA разные или файла нет — пушим
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
        println!("[GITHUB] Обновление успешно завершено.");
        Ok(())
    } else {
        let err_json: serde_json::Value = put_resp.json().await?;
        Err(format!("GitHub Error: {}", err_json["message"]).into())
    }
}

// Новая функция для скачивания (User Mode)
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

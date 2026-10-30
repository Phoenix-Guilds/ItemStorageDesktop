use base64::{engine::general_purpose, Engine as _};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use std::time::Duration;

fn github_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn github_content_payload(
    message: &str,
    content: &str,
    remote_sha: Option<String>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "message": message,
        "content": content,
        "branch": "data"
    });
    if let Some(sha) = remote_sha {
        body["sha"] = serde_json::Value::String(sha);
    }
    body
}

async fn github_json_request(
    request: reqwest::RequestBuilder,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| format!("GitHub Error ({}): {}", status, body))?;
    if !status.is_success() {
        let message = json["message"].as_str().unwrap_or(&body);
        return Err(format!("GitHub Error ({}): {}", status, message).into());
    }
    Ok(json)
}

async fn upload_large_file_via_git_api(
    client: &reqwest::Client,
    token: &str,
    file_content: &str,
    path_in_repo: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_root = "https://api.github.com/repos/Phoenix-Guilds/ItemStorageBrowser";
    let headers = {
        let mut headers = get_gh_headers(token);
        headers.insert(ACCEPT, HeaderValue::from_static("application/vnd.github+json"));
        headers
    };

    let reference = github_json_request(
        client
            .get(format!("{api_root}/git/ref/heads/data"))
            .headers(headers.clone()),
    )
    .await?;
    let parent_sha = reference["object"]["sha"]
        .as_str()
        .ok_or("GitHub не вернул SHA ветки data")?;

    let blob = github_json_request(
        client
            .post(format!("{api_root}/git/blobs"))
            .headers(headers.clone())
            .json(&serde_json::json!({
                "content": general_purpose::STANDARD.encode(file_content),
                "encoding": "base64"
            })),
    )
    .await?;
    let blob_sha = blob["sha"].as_str().ok_or("GitHub не вернул SHA blob")?;

    let commit = github_json_request(
        client
            .get(format!("{api_root}/git/commits/{parent_sha}"))
            .headers(headers.clone()),
    )
    .await?;
    let base_tree_sha = commit["tree"]["sha"]
        .as_str()
        .ok_or("GitHub не вернул SHA дерева")?;

    let tree = github_json_request(
        client
            .post(format!("{api_root}/git/trees"))
            .headers(headers.clone())
            .json(&serde_json::json!({
                "base_tree": base_tree_sha,
                "tree": [{
                    "path": path_in_repo,
                    "mode": "100644",
                    "type": "blob",
                    "sha": blob_sha
                }]
            })),
    )
    .await?;
    let tree_sha = tree["sha"].as_str().ok_or("GitHub не вернул SHA дерева")?;

    let new_commit = github_json_request(
        client
            .post(format!("{api_root}/git/commits"))
            .headers(headers.clone())
            .json(&serde_json::json!({
                "message": message,
                "tree": tree_sha,
                "parents": [parent_sha]
            })),
    )
    .await?;
    let new_commit_sha = new_commit["sha"].as_str().ok_or("GitHub не вернул SHA commit")?;

    github_json_request(
        client
            .patch(format!("{api_root}/git/refs/heads/data"))
            .headers(headers)
            .json(&serde_json::json!({
                "sha": new_commit_sha,
                "force": false
            })),
    )
    .await?;

    Ok(())
}

pub fn get_gh_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();

    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("ItemStorageBrowser-Manager"),
    );
    headers.insert(
        "x-github-api-version",
        HeaderValue::from_static("2022-11-28"),
    );

    if !token.is_empty() {
        if let Ok(auth) = HeaderValue::from_str(&format!("token {}", token)) {
            headers.insert(AUTHORIZATION, auth);
        }
    }

    headers
}

pub async fn validate_token(token: &str) -> bool {
    let client = github_client();

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
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let client = github_client();
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

pub async fn get_file_sha(
    token: &str,
    path_in_repo: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let client = github_client();
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
        Ok(json["sha"].as_str().map(|s| s.to_string()))
    } else if resp.status() == reqwest::StatusCode::NOT_FOUND {
        Ok(None)
    } else {
        Err(format!("GitHub Error: {}", resp.status()).into())
    }
}

pub async fn download_raw_from_github(
    token: &str,
    path_in_repo: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let client = github_client();
    let url = format!(
        "https://api.github.com/repos/Phoenix-Guilds/ItemStorageBrowser/contents/{}?ref=data",
        path_in_repo
    );

    let mut headers = get_gh_headers(token);
    headers.insert(
        reqwest::header::ACCEPT,
        HeaderValue::from_static("application/vnd.github.v3.raw"),
    );

    let resp = client.get(&url).headers(headers).send().await?;
    match resp.status() {
        reqwest::StatusCode::OK => Ok(Some(resp.text().await?)),
        reqwest::StatusCode::NOT_FOUND => Ok(None),
        status => Err(format!("Ошибка скачивания: {}", status).into()),
    }
}

pub async fn upload_to_github(
    token: &str,
    file_content: &str,
    path_in_repo: &str,
    message: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()?;
    if file_content.len() > 900_000 {
        return upload_large_file_via_git_api(
            &client,
            token,
            file_content,
            path_in_repo,
            message,
        )
        .await;
    }
    let url = format!(
        "https://api.github.com/repos/Phoenix-Guilds/ItemStorageBrowser/contents/{}",
        path_in_repo
    );

    let remote_sha = get_file_sha(token, path_in_repo).await?;
    let payload = github_content_payload(
        message,
        &general_purpose::STANDARD.encode(file_content),
        remote_sha,
    );

    let payload_bytes = serde_json::to_vec(&payload)?;
    if !payload_bytes.starts_with(b"{") {
        return Err("GitHub payload is not a JSON object".into());
    }
    let payload_len = payload_bytes.len();
    let put_resp = client
        .put(&url)
        .headers({
            let mut headers = get_gh_headers(token);
            headers.insert(ACCEPT, HeaderValue::from_static("application/vnd.github+json"));
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            headers
        })
        .body(payload_bytes)
        .send()
        .await?;

    if put_resp.status().is_success() {
        Ok(())
    } else {
        let status = put_resp.status();
        let request_id = put_resp
            .headers()
            .get("x-github-request-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("unknown")
            .to_string();
        let response_body = put_resp.text().await?;
        let message = serde_json::from_str::<serde_json::Value>(&response_body)
            .ok()
            .and_then(|body| body["message"].as_str().map(str::to_owned))
            .unwrap_or(response_body);
        Err(format!(
            "GitHub Error ({}): {} (request {}, payload {} bytes)",
            status, message, request_id, payload_len
        )
        .into())
    }
}

pub async fn download_from_github(
    token: &str,
    path_in_repo: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let client = github_client();
    let url = format!(
        "https://api.github.com/repos/Phoenix-Guilds/ItemStorageBrowser/contents/{}?ref=data",
        path_in_repo
    );

    let mut headers = get_gh_headers(token);
    // Добавляем специальный заголовок для получения "сырых" данных
    headers.insert(
        reqwest::header::ACCEPT,
        HeaderValue::from_static("application/vnd.github.v3.raw"),
    );

    let resp = client.get(&url).headers(headers).send().await?;

    if resp.status().is_success() {
        let content = resp.text().await?;
        if content.is_empty() {
            return Err("Скачанный файл пуст (0 байт)".into());
        }
        Ok(content)
    } else {
        Err(format!("Ошибка скачивания: {}", resp.status()).into())
    }
}

pub async fn get_latest_release_version() -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    let client = github_client();
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

#[cfg(test)]
mod tests {
    use super::github_content_payload;

    #[test]
    fn github_content_payload_is_json_object() {
        let payload = github_content_payload("msg", "dGVzdA==", None);
        let bytes = serde_json::to_vec(&payload).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value.is_object());
        assert_eq!(value["message"], "msg");
        assert_eq!(value["content"], "dGVzdA==");
    }
}

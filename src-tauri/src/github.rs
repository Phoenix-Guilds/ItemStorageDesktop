use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};

pub fn get_gh_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("ItemStorageBrowser-Manager"));
    if !token.is_empty() {
        if let Ok(auth) = HeaderValue::from_str(&format!("token {}", token)) {
            headers.insert(AUTHORIZATION, auth);
        }
    }
    headers
}
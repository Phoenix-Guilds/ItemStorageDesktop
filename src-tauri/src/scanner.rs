use chrono::NaiveDateTime;
use regex::Regex;
use std::fs;
use std::path::Path;

pub struct CharacterInfo {
    pub name_realm: String,
    pub last_logout: NaiveDateTime,
}

pub fn run_admin_scan(game_path: &str) -> Option<CharacterInfo> {
    let accounts = vec!["HKFIRST01", "HKFIRST02", "HKFIRST03", "HKFIRST04"];
    let mut latest_char: Option<CharacterInfo> = None;

    for acc in accounts {
        let file_path = Path::new(game_path).join(format!(
            r"WTF\Account\{}\SavedVariables\CharacterStatusLogger.lua",
            acc
        ));

        if file_path.exists() {
            if let Ok(content) = fs::read_to_string(&file_path) {
                if let Some(found) = find_latest_in_lua(&content) {
                    if latest_char.as_ref().map_or(true, |c| found.last_logout > c.last_logout) {
                        latest_char = Some(found);
                    }
                }
            }
        }
    }
    latest_char
}

fn find_latest_in_lua(content: &str) -> Option<CharacterInfo> {
    let re = Regex::new(r#"\["(?P<name>.*?)"\] = \{\s+\["lastLogout"\] = "(?P<date>.*?)","#).unwrap();
    let mut latest: Option<CharacterInfo> = None;

    for cap in re.captures_iter(content) {
        if let Ok(dt) = NaiveDateTime::parse_from_str(&cap["date"], "%Y-%m-%d %H:%M:%S") {
            let info = CharacterInfo {
                name_realm: cap["name"].to_string(),
                last_logout: dt,
            };
            if latest.as_ref().map_or(true, |curr| info.last_logout > curr.last_logout) {
                latest = Some(info);
            }
        }
    }
    latest
}
use chrono::{NaiveDateTime, TimeZone, Utc};
use regex::Regex;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub struct CharacterInfo {
    pub name_realm: String,
    pub last_logout: NaiveDateTime,
}

#[derive(Debug, Clone)]
pub struct ExportedItem {
    pub item_link: String,
    pub item_name: String,
    pub item_count: u64,
}

#[derive(Debug, Clone)]
pub struct SectionExport {
    pub last_update: Option<chrono::DateTime<chrono::Utc>>,
    pub items: Vec<ExportedItem>,
}

#[derive(Debug, Clone)]
pub struct CharacterExportData {
    pub character: String,
    pub realm: Option<String>,
    pub money: u64,
    pub location: String,
    pub free_total_slots: u64,
    pub bank: SectionExport,
    pub bags: SectionExport,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

pub fn run_admin_scan(game_path: &str) -> Option<CharacterInfo> {
    let (_, export_data) = scan_latest_admin_export(game_path)?;
    Some(CharacterInfo {
        name_realm: export_data.character,
        last_logout: export_data.last_updated.naive_utc(),
    })
}

pub fn scan_latest_admin_export(game_path: &str) -> Option<(PathBuf, CharacterExportData)> {
    let accounts = vec!["HKFIRST01", "HKFIRST02", "HKFIRST03", "HKFIRST04"];
    let mut candidates: Vec<(PathBuf, u64)> = Vec::new();

    for acc in accounts {
        let account_dir = Path::new(game_path).join(format!(r"WTF\Account\{}", acc));
        if account_dir.exists() {
            // recursively search for per-character SavedVariables files
            fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for e in entries.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            walk(&p, out);
                        } else if p.file_name().and_then(|n| n.to_str()) == Some("ItemStorageBrowser.lua") {
                            out.push(p);
                        }
                    }
                }
            }

            let mut found: Vec<PathBuf> = Vec::new();
            walk(&account_dir, &mut found);
            for f in found {
                if let Some(modified_secs) = get_file_modified_secs(&f) {
                    candidates.push((f, modified_secs));
                }
            }
        }
    }

    candidates.sort_by(|a, b| b.1.cmp(&a.1));

    for (path, _) in candidates {
        if let Ok(export_data) = read_last_character_export_data_from_file_path(&path) {
            return Some((path, export_data));
        }
    }

    None
}

fn get_file_modified_secs(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

pub fn read_last_character_export_data_from_file_path(
    file_path: &Path,
) -> Result<CharacterExportData, Box<dyn Error>> {
    let content = fs::read_to_string(file_path)?;
    let mut data = parse_export_data(&content)?;
    
    // Получаем время модификации файла
    let modified_secs = get_file_modified_secs(file_path).unwrap_or(0);
    data.last_updated = Utc.timestamp_opt(modified_secs as i64, 0).single().unwrap_or_else(|| Utc::now());
    
    Ok(data)
}

fn parse_export_data(content: &str) -> Result<CharacterExportData, Box<dyn Error>> {
    // We support two formats:
    // 1) Old format: ItemStorage_ExportData contains top-level `items` array (bags only)
    // 2) New format: ItemStorage_ExportData contains `bank` and `bags` sections each with `last_update` and `items`

    let mut in_export = false;
    let mut depth: i32 = 0;

    let mut character = None;
    let mut realm = None;
    let mut money = None;
    let mut location = None;
    let mut free_total_slots = None;

    let mut bags_items: Vec<ExportedItem> = Vec::new();
    let mut bank_items: Vec<ExportedItem> = Vec::new();
    let mut bags_last: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut bank_last: Option<chrono::DateTime<chrono::Utc>> = None;

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if !in_export {
            if trimmed.starts_with("ItemStorage_ExportData") && trimmed.contains('{') {
                in_export = true;
                depth += trimmed.matches('{').count() as i32;
                depth -= trimmed.matches('}').count() as i32;
            }
            i += 1;
            continue;
        }

        depth += trimmed.matches('{').count() as i32;
        depth -= trimmed.matches('}').count() as i32;

        if depth <= 0 {
            break;
        }

        if character.is_none() {
            if let Some(v) = parse_lua_string_kv(trimmed, "character") {
                character = Some(v);
                i += 1;
                continue;
            }
        }
        if realm.is_none() {
            if let Some(v) = parse_lua_string_kv(trimmed, "realm") {
                realm = Some(v);
                i += 1;
                continue;
            }
        }
        if money.is_none() {
            if let Some(v) = parse_lua_number_kv(trimmed, "money") {
                money = Some(v);
                i += 1;
                continue;
            }
        }
        if location.is_none() {
            if let Some(v) = parse_lua_string_kv(trimmed, "location") {
                location = Some(v);
                i += 1;
                continue;
            }
        }
        if free_total_slots.is_none() {
            if let Some(v) = parse_lua_number_kv(trimmed, "free_total_slots") {
                free_total_slots = Some(v);
                i += 1;
                continue;
            }
        }

        // Detect sections
        if trimmed.starts_with("[\"bags\"]") && trimmed.contains('{') {
            // parse bags section until matching brace
            let mut sect_depth = trimmed.matches('{').count() as i32 - trimmed.matches('}').count() as i32;
            i += 1;
            while i < lines.len() && sect_depth > 0 {
                let l = lines[i].trim();
                if let Some(v) = parse_lua_number_kv(l, "last_update") {
                    bags_last = Some(parse_timestamp_to_utc(v));
                }
                // detect items array key (supports ["items"] or items)
                let items_key_re = Regex::new(r#"^\s*(?:\[\"items\"\]|items)\s*=\s*\{"#).unwrap();
                if items_key_re.is_match(l) {
                    // parse items array: find each item block `{ ... }` inside
                    let mut arr_depth = l.matches('{').count() as i32 - l.matches('}').count() as i32;
                    i += 1;
                    while i < lines.len() && arr_depth > 0 {
                        let inner = lines[i];
                        let trimmed_inner = inner.trim();
                        // start of an item
                        if trimmed_inner == "{" {
                            let mut item_depth = 1;
                            let mut current_item: Vec<String> = Vec::new();
                            current_item.push(inner.to_string());
                            i += 1;
                            while i < lines.len() && item_depth > 0 {
                                let it_line = lines[i];
                                let it_trim = it_line.trim();
                                current_item.push(it_line.to_string());
                                item_depth += it_trim.matches('{').count() as i32;
                                item_depth -= it_trim.matches('}').count() as i32;
                                i += 1;
                            }
                            if let Some(it) = parse_item_block(&current_item) {
                                bags_items.push(it);
                            }
                            continue;
                        }

                        arr_depth += trimmed_inner.matches('{').count() as i32;
                        arr_depth -= trimmed_inner.matches('}').count() as i32;
                        i += 1;
                    }
                    continue;
                }
                sect_depth += l.matches('{').count() as i32;
                sect_depth -= l.matches('}').count() as i32;
                i += 1;
            }
            continue;
        }

        if trimmed.starts_with("[\"bank\"]") && trimmed.contains('{') {
            let mut sect_depth = trimmed.matches('{').count() as i32 - trimmed.matches('}').count() as i32;
            i += 1;
            while i < lines.len() && sect_depth > 0 {
                let l = lines[i].trim();
                if let Some(v) = parse_lua_number_kv(l, "last_update") {
                    bank_last = Some(parse_timestamp_to_utc(v));
                }
                let items_key_re = Regex::new(r#"^\s*(?:\[\"items\"\]|items)\s*=\s*\{"#).unwrap();
                if items_key_re.is_match(l) {
                    let mut arr_depth = l.matches('{').count() as i32 - l.matches('}').count() as i32;
                    i += 1;
                    while i < lines.len() && arr_depth > 0 {
                        let inner = lines[i];
                        let trimmed_inner = inner.trim();
                        if trimmed_inner == "{" {
                            let mut item_depth = 1;
                            let mut current_item: Vec<String> = Vec::new();
                            current_item.push(inner.to_string());
                            i += 1;
                            while i < lines.len() && item_depth > 0 {
                                let it_line = lines[i];
                                let it_trim = it_line.trim();
                                current_item.push(it_line.to_string());
                                item_depth += it_trim.matches('{').count() as i32;
                                item_depth -= it_trim.matches('}').count() as i32;
                                i += 1;
                            }
                            if let Some(it) = parse_item_block(&current_item) {
                                bank_items.push(it);
                            }
                            continue;
                        }

                        arr_depth += trimmed_inner.matches('{').count() as i32;
                        arr_depth -= trimmed_inner.matches('}').count() as i32;
                        i += 1;
                    }
                    continue;
                }
                sect_depth += l.matches('{').count() as i32;
                sect_depth -= l.matches('}').count() as i32;
                i += 1;
            }
            continue;
        }

        // Fallback: old style top-level items array
        if trimmed == "{" {
            // parse a top-level item block list
            let mut item_depth = 1;
            let mut current_item: Vec<String> = Vec::new();
            current_item.push(lines[i].to_string());
            i += 1;
            while i < lines.len() && item_depth > 0 {
                let inner = lines[i];
                let trimmed_inner = inner.trim();
                current_item.push(inner.to_string());
                item_depth += trimmed_inner.matches('{').count() as i32;
                item_depth -= trimmed_inner.matches('}').count() as i32;
                if item_depth == 0 {
                    if let Some(it) = parse_item_block(&current_item) {
                        bags_items.push(it);
                    }
                }
                i += 1;
            }
            continue;
        }

        i += 1;
    }

    let consolidated_bags = consolidate_items(bags_items);
    let consolidated_bank = consolidate_items(bank_items);

    Ok(CharacterExportData {
        character: character.ok_or("Не удалось найти поле `character` в ItemStorage_ExportData")?,
        realm,
        money: money.ok_or("Не удалось найти поле `money` в ItemStorage_ExportData")?,
        location: location.ok_or("Не удалось найти поле `location` в ItemStorage_ExportData")?,
        free_total_slots: free_total_slots
            .ok_or("Не удалось найти поле `free_total_slots` в ItemStorage_ExportData")?,
        bank: SectionExport { last_update: bank_last, items: consolidated_bank },
        bags: SectionExport { last_update: bags_last, items: consolidated_bags },
        last_updated: Utc::now(), // will be overwritten by caller with file mtime
    })
 }

fn parse_timestamp_to_utc(value: u64) -> chrono::DateTime<chrono::Utc> {
    Utc.timestamp_opt(value as i64, 0)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap())
}

fn consolidate_items(items: Vec<ExportedItem>) -> Vec<ExportedItem> {
    let mut consolidated: HashMap<String, ExportedItem> = HashMap::new();

    for item in items {
        match consolidated.get_mut(&item.item_link) {
            Some(existing) => {
                // Если item_link уже существует, суммируем количество
                existing.item_count += item.item_count;
            }
            None => {
                // Если item_link новый, добавляем его в HashMap
                consolidated.insert(item.item_link.clone(), item);
            }
        }
    }

    let mut result: Vec<ExportedItem> = consolidated
        .into_values()
        .filter(|item| item.item_count > 0)
        .collect();
    result.sort_by(|a, b| a.item_link.cmp(&b.item_link));
    result
}

fn parse_lua_string_kv(line: &str, key: &str) -> Option<String> {
    // support both ["key"] = "value" and key = "value"
    let patterns = vec![
        format!(r#"^\s*\[\"{}\"\]\s*=\s*\"((?:[^\"\\]|\\.)*)\""#, regex::escape(key)),
        format!(r#"^\s*{}\s*=\s*\"((?:[^\"\\]|\\.)*)\""#, regex::escape(key)),
    ];

    for p in patterns {
        if let Ok(re) = Regex::new(&p) {
            if let Some(cap) = re.captures(line) {
                if let Some(m) = cap.get(1) {
                    return Some(m.as_str().replace("\\\"", "\"").replace("\\\\", "\\"));
                }
            }
        }
    }
    None
}

fn parse_lua_number_kv(line: &str, key: &str) -> Option<u64> {
    // support both ["key"] = 123 and key = 123
    let patterns = vec![
        format!(r#"^\s*\[\"{}\"\]\s*=\s*(\d+)"#, regex::escape(key)),
        format!(r#"^\s*{}\s*=\s*(\d+)"#, regex::escape(key)),
    ];

    for p in patterns {
        if let Ok(re) = Regex::new(&p) {
            if let Some(cap) = re.captures(line) {
                if let Some(m) = cap.get(1) {
                    if let Ok(v) = m.as_str().parse::<u64>() {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

fn parse_item_block(lines: &[String]) -> Option<ExportedItem> {
    let mut item_link = None;
    let mut item_name = None;
    let mut item_count = None;

    for line in lines {
        let trimmed = line.trim();
        if item_link.is_none() {
            item_link = parse_lua_string_kv(trimmed, "item_link").or_else(|| parse_lua_string_kv(trimmed, "link"));
            if item_link.is_some() {
                continue;
            }
        }
        if item_name.is_none() {
            item_name = parse_lua_string_kv(trimmed, "item_name").or_else(|| parse_lua_string_kv(trimmed, "name"));
            if item_name.is_some() {
                continue;
            }
        }
        if item_count.is_none() {
            item_count = parse_lua_number_kv(trimmed, "item_count").or_else(|| parse_lua_number_kv(trimmed, "count"));
            if item_count.is_some() {
                continue;
            }
        }
    }

    Some(ExportedItem {
        item_link: item_link?,
        item_name: item_name?,
        item_count: item_count?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lua_string_kv_with_quotes() {
        // Тест с экранированными кавычками
        let line = r#"			["item_name"] = "Схема: ультра-очки \"Кошачий глаз\"","#;
        let result = parse_lua_string_kv(line, "item_name");
        assert_eq!(result, Some("Схема: ультра-очки \"Кошачий глаз\"".to_string()));

        // Тест с обычной строкой
        let line2 = r#"			["item_name"] = "Обычный предмет","#;
        let result2 = parse_lua_string_kv(line2, "item_name");
        assert_eq!(result2, Some("Обычный предмет".to_string()));

        // Тест с обратными слешами
        let line3 = r#"			["item_name"] = "Предмет с \\ обратным слешем","#;
        let result3 = parse_lua_string_kv(line3, "item_name");
        assert_eq!(result3, Some("Предмет с \\ обратным слешем".to_string()));
    }
}

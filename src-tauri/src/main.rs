// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::NaiveDateTime;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug)]
struct AppConfig {
    game_path: String,
    github_token: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            game_path: String::from(r"C:\Games\World of Warcraft"),
            github_token: String::from(""),
        }
    }
}

struct CharacterInfo {
    name_realm: String,
    last_logout: NaiveDateTime,
}

fn main() -> Result<(), confy::ConfyError> {
    println!("--- ItemStorageBrowser Manager (Dev Version) ---");

    // 1. Загружаем или создаем конфиг (в Windows это будет в AppData/Roaming/item-storage-manager)
    let mut cfg: AppConfig = confy::load("item-storage-manager", None)?;

    // 2. Валидация пути к папке с игрой
    while !is_valid_wow_path(&cfg.game_path) {
        println!(
            "[!] Путь к игре не найден или некорректен: {}",
            cfg.game_path
        );
        print!("Введите полный путь к папке World of Warcraft: ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("Ошибка чтения строки");
        let new_path = input.trim().replace('"', ""); // Убираем лишние кавычки, если пользователь скопировал путь как "C:\..."

        if is_valid_wow_path(&new_path) {
            cfg.game_path = new_path;
            confy::store("item-storage-manager", None, &cfg)?;
            println!("[OK] Путь сохранен!");
            break;
        }
    }

    // 3. Определение режима
    if is_admin_mode(&cfg.game_path) {
        println!("[MODE] Режим: АДМИН");
        run_admin_cycle(&cfg.game_path);
    } else {
        println!("[MODE] Режим: ПОЛЬЗОВАТЕЛЬ");
        // Здесь будет логика пользователя
    }

    Ok(())
}

// Проверка: является ли папка папкой WoW (ищем по наличию папки WTF или исполняемого файла)
fn is_valid_wow_path(path: &str) -> bool {
    let p = Path::new(path);
    p.exists() && p.is_dir() && p.join("WTF").exists()
}

fn is_admin_mode(game_path: &str) -> bool {
    let addon_path = Path::new(game_path).join(r"Interface\AddOns\CharacterStatusLogger");
    addon_path.exists()
}

// --- Далее идет логика сканера из предыдущего шага ---
fn run_admin_cycle(game_path: &str) {
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
                    match &latest_char {
                        None => latest_char = Some(found),
                        Some(current) if found.last_logout > current.last_logout => {
                            latest_char = Some(found);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    match latest_char {
        Some(c) => println!(
            "[RESULT] Самый свежий персонаж: {} (Выход: {})",
            c.name_realm, c.last_logout
        ),
        None => println!("[!] Данные не найдены в папках HKFIRST01-04."),
    }
}

fn find_latest_in_lua(content: &str) -> Option<CharacterInfo> {
    let re =
        Regex::new(r#"\["(?P<name>.*?)"\] = \{\s+\["lastLogout"\] = "(?P<date>.*?)","#).unwrap();
    let mut latest: Option<CharacterInfo> = None;

    for cap in re.captures_iter(content) {
        let name_realm = cap["name"].to_string();
        let date_str = &cap["date"];

        if let Ok(dt) = NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
            let info = CharacterInfo {
                name_realm,
                last_logout: dt,
            };
            match &latest {
                None => latest = Some(info),
                Some(curr) if info.last_logout > curr.last_logout => latest = Some(info),
                _ => {}
            }
        }
    }
    latest
}

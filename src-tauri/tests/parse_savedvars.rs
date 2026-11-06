use std::path::Path;

#[test]
fn parse_savedvariables_file() {
    let path = Path::new(r"G:/Games/Sirus-WoW/WTF/Account/HKFIRST04/ProxyEU Sirus x5 - 3.3.5a+/Бумажная/SavedVariables/ItemStorageBrowser.lua");
    if !path.exists() {
        eprintln!("Test file not found: {:?}, skipping.", path);
        return;
    }

    let data = tauri_app_lib::scanner::read_last_character_export_data_from_file_path(path).expect("parse failed");
    let bags_count: usize = data.bags.items.len();
    let bank_count: usize = data.bank.items.len();
    println!("Parsed character: {} realm: {:?}", data.character, data.realm);
    println!("bags items: {} bank items: {}", bags_count, bank_count);
    assert!(bags_count > 0 || bank_count > 0, "Expected at least some items parsed");
}

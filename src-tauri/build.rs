#[allow(deprecated)]
use adblock::resources::resource_assembler::assemble_scriptlet_resources;
use std::{env, fs, path::Path};

// Keep this ACL list aligned with the commands registered in src/lib.rs.
const APP_COMMANDS: &[&str] = &[
    "list_tabs",
    "active_tab_id",
    "toggle_page_devtools",
    "open_page_print_dialog",
    "open_find_bar",
    "find_in_page",
    "toggle_devtools",
    "print_page",
    "save_page_as_pdf",
    "browser_window_mode",
    "open_browser_window",
    "open_private_window",
    "detach_tab_to_new_window",
    "restore_session",
    "set_page_content_visible",
    "create_tab",
    "close_tab",
    "activate_tab",
    "reorder_tabs",
    "tab_groups_list",
    "tab_group_create",
    "tab_group_update",
    "tab_group_assign",
    "tab_group_delete",
    "navigate",
    "go_back",
    "go_forward",
    "reload",
    "stop",
    "go_home",
    "resize_active_view",
    "tab_sleep_timeout_get",
    "tab_sleep_timeout_set",
    "adblock_status",
    "adblock_set_enabled",
    "adblock_set_site_allowed",
    "adblock_get_custom_filters",
    "adblock_set_custom_filters",
    "privacy_status",
    "privacy_site_permissions",
    "privacy_set_site_permission",
    "privacy_set_https_only",
    "privacy_set_block_third_party_cookies",
    "privacy_set_clear_cookies_on_exit",
    "privacy_clear_cookies_and_cache",
    "bookmarks_list",
    "bookmark_folders",
    "bookmark_create",
    "bookmark_update",
    "bookmark_delete",
    "bookmark_folder_create",
    "bookmark_folder_delete",
    "bookmark_is_saved",
    "bookmark_import",
    "history_list",
    "history_clear_before",
    "history_delete_site",
    "history_clear",
    "downloads_list",
    "downloads_clear",
    "downloads_recover",
    "download_open",
    "download_reveal",
    "download_directory",
    "set_download_directory",
    "passwords_list",
    "passwords_save",
    "passwords_reveal",
    "passwords_delete",
    "user_scripts_list",
    "user_scripts_save",
    "user_scripts_set_enabled",
    "user_scripts_delete",
    "updater_settings_get",
    "updater_settings_set",
    "updater_current_version",
    "updater_check",
    "updater_install",
    "oneko_settings_get",
    "oneko_settings_set",
    "oneko_toggle",
    "oneko_site_visibility_set",
];

fn main() {
    assert_version_sync();
    let scriptlets = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("ublock")
        .join("scriptlets.js");
    println!("cargo:rerun-if-changed={}", scriptlets.display());

    #[allow(deprecated)]
    let resources = assemble_scriptlet_resources(&scriptlets);
    let encoded = serde_json::to_vec(&resources).expect("serialize uBlock scriptlet resources");
    let output = Path::new(&env::var_os("OUT_DIR").expect("OUT_DIR is set"))
        .join("ublock-scriptlet-resources.json");
    fs::write(output, encoded).expect("write generated uBlock scriptlet resources");

    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(APP_COMMANDS)),
    )
    .expect("failed to build Tauri application metadata");
}

fn assert_version_sync() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .expect("src-tauri must be inside the project root");
    let version_path = root.join("VERSION");
    let package_path = root.join("package.json");
    let package_lock_path = root.join("package-lock.json");
    let cargo_lock_path = manifest_dir.join("Cargo.lock");
    let tauri_config_path = manifest_dir.join("tauri.conf.json");
    for path in [
        &version_path,
        &package_path,
        &package_lock_path,
        &cargo_lock_path,
        &tauri_config_path,
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let canonical = fs::read_to_string(&version_path)
        .unwrap_or_else(|error| panic!("could not read VERSION: {error}"));
    let canonical = canonical.trim();
    assert!(!canonical.is_empty(), "VERSION cannot be empty");
    assert_version_matches("Cargo.toml", env!("CARGO_PKG_VERSION"), canonical);

    let package: serde_json::Value = read_json(&package_path);
    assert_version_matches(
        "package.json",
        json_version(&package, "package.json"),
        canonical,
    );

    let package_lock: serde_json::Value = read_json(&package_lock_path);
    assert_version_matches(
        "package-lock.json",
        json_version(&package_lock, "package-lock.json"),
        canonical,
    );
    assert_version_matches(
        "package-lock.json root package",
        package_lock["packages"][""]
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
        canonical,
    );

    let tauri_config: serde_json::Value = read_json(&tauri_config_path);
    assert_version_matches(
        "tauri.conf.json",
        json_version(&tauri_config, "tauri.conf.json"),
        canonical,
    );

    let cargo_lock = fs::read_to_string(&cargo_lock_path)
        .unwrap_or_else(|error| panic!("could not read Cargo.lock: {error}"));
    let eefi_package = cargo_lock.lines().collect::<Vec<_>>();
    let lock_version = eefi_package
        .windows(3)
        .find(|lines| lines[0] == "[[package]]" && lines[1] == "name = \"eefi-browser\"")
        .and_then(|lines| lines[2].strip_prefix("version = \""))
        .and_then(|version| version.strip_suffix('"'))
        .unwrap_or("");
    assert_version_matches("Cargo.lock", lock_version, canonical);
}

fn read_json(path: &Path) -> serde_json::Value {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
    serde_json::from_str(&content)
        .unwrap_or_else(|error| panic!("could not parse {}: {error}", path.display()))
}

fn json_version<'a>(value: &'a serde_json::Value, label: &str) -> &'a str {
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("{label} has no version string"))
}

fn assert_version_matches(file: &str, actual: &str, canonical: &str) {
    assert_eq!(
        actual, canonical,
        "{file} is not synchronized with VERSION; run `npm run sync-version`."
    );
}

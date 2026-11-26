#[allow(deprecated)]
use adblock::resources::resource_assembler::assemble_scriptlet_resources;
use std::{env, fs, path::Path};

// Keep this ACL list aligned with the commands registered in src/lib.rs.
const APP_COMMANDS: &[&str] = &[
    "list_tabs",
    "active_tab_id",
    "toggle_page_devtools",
    "open_find_bar",
    "find_in_page",
    "toggle_devtools",
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
];

fn main() {
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

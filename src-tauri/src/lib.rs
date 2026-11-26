mod adblock;
mod extensions;
pub mod nav;
mod passwords;
mod privacy;
pub mod storage;
mod tabs;
mod update_apply;
mod updater;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{Manager, WebviewUrl, WindowEvent};

pub fn run() {
    let app = tauri::Builder::default()
        .manage(tabs::BrowserState::default())
        .manage(tabs::BrowserWindowStates::default())
        .manage(privacy::PrivacyService::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::Destroyed) && window.label() != "main" {
                window
                    .app_handle()
                    .state::<tabs::BrowserWindowStates>()
                    .remove(window.label());
            }
        })
        .setup(|app| {
            let adblock_service = adblock::AdblockService::new();
            let adblock_directory = app.path().app_data_dir()?.join("adblock");
            app.manage(adblock_service.clone());
            let database_path = app.path().app_data_dir()?.join("eefi-browser.sqlite3");
            let storage_service = storage::StorageService::new(database_path);
            app.manage(storage_service.clone());
            adblock::start_filter_updates(adblock_service, adblock_directory, storage_service);
            let download_directory = app.path().download_dir()?;
            std::fs::create_dir_all(&download_directory)?;
            app.manage(storage::DownloadLocation::new(download_directory));
            let window = app
                .get_window("main")
                .ok_or("main browser window is missing")?;
            let state = app.state::<tabs::BrowserState>().inner().clone();
            let app_handle = app.handle().clone();
            let storage = app.state::<storage::StorageService>().inner().clone();
            let download_location = app.state::<storage::DownloadLocation>().inner().clone();
            let adblock = app.state::<adblock::AdblockService>().inner().clone();
            let privacy = app.state::<privacy::PrivacyService>().inner().clone();
            let tab_sleep_policy = tabs::TabSleepPolicy::default();
            app.manage(tab_sleep_policy.clone());
            tabs::start_tab_sleep_monitor(
                app.handle().clone(),
                state.clone(),
                app.state::<tabs::BrowserWindowStates>().inner().clone(),
                storage.clone(),
                tab_sleep_policy,
            );
            let initial_tab = tabs::TabInfo {
                id: "1".to_string(),
                title: "New tab".to_string(),
                url: "about:blank".to_string(),
                group_id: None,
                sleeping: false,
            };
            state
                .initialize(initial_tab, Vec::new())
                .map_err(std::io::Error::other)?;
            let initial_url = "about:blank"
                .parse()
                .map_err(|error| format!("invalid initial webview URL: {error}"))?;
            let builder = tabs::page_builder(
                app_handle,
                state,
                storage,
                download_location,
                "1".to_string(),
                WebviewUrl::External(initial_url),
            );
            window.add_child(
                builder,
                tauri::LogicalPosition::new(0.0, 128.0),
                tauri::LogicalSize::new(1180.0, 652.0),
            )?;
            let initial_page = app
                .get_webview("page-1")
                .ok_or("initial page webview was not registered")?;
            adblock::attach_to_webview(&initial_page, adblock, privacy)
                .map_err(std::io::Error::other)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            tabs::list_tabs,
            tabs::active_tab_id,
            tabs::toggle_page_devtools,
            tabs::open_page_print_dialog,
            tabs::open_find_bar,
            tabs::find_in_page,
            tabs::toggle_devtools,
            tabs::print_page,
            tabs::save_page_as_pdf,
            tabs::browser_window_mode,
            tabs::open_browser_window,
            tabs::open_private_window,
            tabs::detach_tab_to_new_window,
            tabs::restore_session,
            tabs::set_page_content_visible,
            tabs::create_tab,
            tabs::close_tab,
            tabs::activate_tab,
            tabs::reorder_tabs,
            tabs::tab_groups_list,
            tabs::tab_group_create,
            tabs::tab_group_update,
            tabs::tab_group_assign,
            tabs::tab_group_delete,
            tabs::navigate,
            tabs::go_back,
            tabs::go_forward,
            tabs::reload,
            tabs::stop,
            tabs::go_home,
            tabs::resize_active_view,
            tabs::tab_sleep_timeout_get,
            tabs::tab_sleep_timeout_set,
            adblock::adblock_status,
            adblock::adblock_set_enabled,
            adblock::adblock_set_site_allowed,
            adblock::adblock_get_custom_filters,
            adblock::adblock_set_custom_filters,
            privacy::privacy_status,
            privacy::privacy_site_permissions,
            privacy::privacy_set_site_permission,
            privacy::privacy_set_https_only,
            privacy::privacy_set_block_third_party_cookies,
            privacy::privacy_set_clear_cookies_on_exit,
            privacy::privacy_clear_cookies_and_cache,
            storage::bookmarks_list,
            storage::bookmark_folders,
            storage::bookmark_create,
            storage::bookmark_update,
            storage::bookmark_delete,
            storage::bookmark_folder_create,
            storage::bookmark_folder_delete,
            storage::bookmark_is_saved,
            storage::bookmark_import,
            storage::history_list,
            storage::history_clear_before,
            storage::history_delete_site,
            storage::history_clear,
            storage::downloads_list,
            storage::downloads_clear,
            storage::downloads_recover,
            storage::download_open,
            storage::download_reveal,
            storage::download_directory,
            storage::set_download_directory,
            passwords::passwords_list,
            passwords::passwords_save,
            passwords::passwords_reveal,
            passwords::passwords_delete,
            extensions::user_scripts_list,
            extensions::user_scripts_save,
            extensions::user_scripts_set_enabled,
            extensions::user_scripts_delete,
            updater::updater_settings_get,
            updater::updater_settings_set,
            updater::updater_current_version,
            updater::updater_check,
            updater::updater_install
        ])
        .build(tauri::generate_context!())
        .expect("error while building Eefi Browser");
    let exit_cleanup_started = Arc::new(AtomicBool::new(false));
    let exit_cleanup_finished = Arc::new(AtomicBool::new(false));
    app.run(move |app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if exit_cleanup_finished.load(Ordering::Acquire) {
                return;
            }
            let privacy = app_handle
                .state::<privacy::PrivacyService>()
                .inner()
                .clone();
            if privacy.preferences_loaded() && !privacy.clear_cookies_on_exit() {
                return;
            }
            api.prevent_exit();
            if exit_cleanup_started.swap(true, Ordering::AcqRel) {
                return;
            }
            let app_handle = app_handle.clone();
            let storage = app_handle
                .state::<storage::StorageService>()
                .inner()
                .clone();
            let cleanup_finished = exit_cleanup_finished.clone();
            tauri::async_runtime::spawn(async move {
                match privacy::ensure_preferences_loaded(&storage, &privacy).await {
                    Ok(()) if privacy.clear_cookies_on_exit() => {
                        match app_handle.get_webview("main") {
                            Some(webview) => {
                                if let Err(error) =
                                    privacy::clear_profile_data(webview, true, true).await
                                {
                                    eprintln!("could not clear browsing data on exit: {error}");
                                }
                            }
                            None => eprintln!(
                                "could not clear browsing data on exit: main webview missing"
                            ),
                        }
                    }
                    Ok(()) => {}
                    Err(error) => eprintln!("could not load exit privacy settings: {error}"),
                }
                cleanup_finished.store(true, Ordering::Release);
                app_handle.exit(0);
            });
        }
    });
}

pub fn run_update_helper() -> bool {
    update_apply::run_if_requested()
}

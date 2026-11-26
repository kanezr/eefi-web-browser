# Changelog

Notable changes to Eefi Browser are documented here. This changelog follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Added the WebView2 print dialog. Use **Print** from the browser menu or press
  `Ctrl+P` while the browser chrome or a page is focused.
- Added **Save as PDF** from the browser menu. Choose a destination with the
  native save dialog; WebView2 writes the active page directly to a PDF and
  includes page backgrounds.

### Changed

- The open browser menu now uses a scrollable panel on the right, and the page
  webview resizes into the remaining space instead of being hidden behind the
  menu.

### Fixed

- Prevented printing or PDF export from stale/sleeping tabs; wake a sleeping
  tab before printing or exporting it.
- Validate the selected PDF destination and ensure the saved file uses the
  `.pdf` extension.

## [0.1.0] - 2026-11-26

The first public, early-development release of Eefi Browser for Windows.

### Added

- Added a Tauri 2 desktop shell with a Rust backend, Svelte/TypeScript browser
  interface, and Microsoft Edge WebView2 for rendering pages.
- Added an address bar that accepts web addresses or search queries, plus
  back, forward, reload, stop, home, and common keyboard shortcuts.
- Added native page tabs with drag reordering, separate browser windows,
  named/color tab groups, and main-window session restoration.
- Added configurable tab sleeping. Sleeping releases an inactive page webview;
  returning to the tab reloads its current address.
- Added SQLite-backed bookmarks and folders, searchable browsing history,
  download records, browser settings, and saved session state.
- Added bookmark-bar support, Netscape HTML bookmark import/export, history
  deletion, and controls for clearing history, cookies, and disk cache.
- Added a native WebView2 request blocker with built-in filter sources,
  per-site allowlisting, custom HTTPS filter lists/rules, and a bounded set of
  cosmetic filters.
- Added private windows, optional HTTPS-only upgrades, per-site permission
  controls, Do Not Track / Global Privacy Control request headers, and initial
  cookie controls.
- Added a Windows DPAPI-protected password vault for manually saving, viewing,
  and deleting credentials.
- Added user scripts with host/path matching, WebView2 DevTools, and
  find-in-page with previous/next match navigation.
- Added Windows CI and portable ZIP release workflows, plus issue and pull
  request templates.

### Known limitations

- This is an unsigned early-development build and may display a Windows
  SmartScreen warning. The portable ZIP requires the WebView2 Evergreen Runtime.
- Ad and tracker blocking, cosmetic filtering, and popup handling are
  incomplete and may vary across websites and WebView2 versions.
- Sleeping tabs do not retain in-page state or back/forward history.
- Downloads do not yet have byte progress, pause/resume/cancel controls, or an
  “ask where to save” prompt. Bookmark HTML import/export does not preserve
  nested folders, and only the main window is restored at startup.
- Third-party cookie control strips cookies from cross-site requests but does
  not prevent WebView2 from storing cookies received in responses. Private
  browsing does not make network activity anonymous.
- The password vault does not autofill forms or manage address/payment data.
  User scripts run in the normal page context and are not isolated from page
  JavaScript.

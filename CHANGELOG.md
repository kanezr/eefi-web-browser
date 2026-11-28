# Changelog

Notable changes to Eefi Browser are documented here. This changelog follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Add best-effort built-in filters for known YouTube ad requests, player ad
  metadata, and overlays, plus Twitch ad hosts and paths. These rules are
  available before downloaded filter lists finish updating.

### Changed

- Reuse the profile's SQLite connection and add indexed substring search for
  omnibox history suggestions; cap bookmark suggestions and combine them into
  one request.
- Parallelize independent browser startup reads and coalesce redundant page
  webview resize updates.
- Reduce lock hold time when recording blocked-request counts.
- Coalesce session persistence during bursts of page and title events, and
  flush pending session changes on graceful application exit.
- Refresh toolbar status once per completed page navigation.
- Fetch built-in and custom ad-block lists concurrently with a bounded worker
  count.

### Fixed

- Ignore stale bookmark/ad-block status responses and serialize tab activation
  with sleeping-tab transitions.
- Invalidate the compiled ad-block engine when built-in rules change, so
  existing profiles rebuild stale cached rules after restart.
- Reject invalid filter-list responses without replacing the last-known-good
  cache, recover corrupted custom-list caches, and retry failed updates sooner.
- Apply adblock engine URL rewrites and available redirect resources to
  intercepted WebView2 requests.

### Known limitations

- Ads stitched into the same media stream as the video may not be reliably
  blocked.

## [0.1.3] - 2026-11-28

### Added

- Added Reader Mode for web articles, with adjustable text size and column
  width, light/sepia/dark themes, and an in-page exit control.
- Added live download byte progress with pause, resume, and cancel controls.
- Added a persistent **Ask where to save each file** preference. Choosing a
  location opens the Windows save dialog before the download starts.

### Fixed

- Keep active downloads in the list when clearing completed download records.
- Remove a canceled download's partial file after WebView2 finishes canceling it.

## [0.1.3-beta] - 2026-11-27

### Added

- Added **Reopen closed tab** to the Browser menu and the `Ctrl+Shift+T`
  shortcut. Recently closed tabs are kept per window for the current session,
  including their URL, title, and tab group.

### Fixed

- Track single-page route changes through WebView2's native history event so
  the full URL is retained in the address bar and when reopening a closed tab.
  Hover or focus the address bar to read long URLs in a wrapped preview.

## [0.1.2] - 2026-11-27

### Added

- Added a local Home page for new and blank tabs, with a search box and
  shortcuts to bookmarks on the bookmarks bar. The Home button now opens this
  page instead of navigating to Google.
- Added automated version-format tests to CI for stable versions and arbitrary
  valid SemVer prerelease labels.

### Changed

- `VERSION` now synchronizes valid SemVer prerelease labels such as `alpha`,
  `beta`, or custom labels without requiring a specific channel name. Keep the
  `v` prefix for Git release tags only; do not put it in `VERSION`.
- Keep SQLite's schema marker at version 6 for compatibility with older browser
  builds. Future database changes must remain additive and must not advance the
  marker.
- Updated the SHA-256 dependency and updater digest encoding to keep integrity
  verification compatible with `sha2` 0.11.

### Fixed

- Databases previously marked as schema version 7 are reopened with the
  compatible version-6 marker. Existing site settings remain intact.

## [0.1.2-oneko] - 2026-11-26

### Added

- Added the optional Oneko cat overlay with a toolbar toggle, `Ctrl+Shift+K`
  shortcut, per-site hiding, speed/size settings, and reduced-motion support.
- Added Rust backend integration tests for navigation/storage workflows,
  download recovery, session persistence, and legacy database migration.
- Added a security policy with private vulnerability-reporting guidance.

## [0.1.1] - 2026-11-26

### Added

- Added an updater settings section with stable and beta release channels,
  optional automatic installation, startup checks, and a manual update check.
- Added release executable assets and GitHub pre-release tagging so the
  portable browser can download and install updates from GitHub Releases.
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

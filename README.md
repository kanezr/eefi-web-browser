# Eefi Browser

Eefi Browser is an early-development desktop browser for Windows. It uses
**Tauri 2**, a **Rust** native backend, a **Svelte/TypeScript** browser UI, and
the system **Microsoft Edge WebView2** engine for web pages. It does not include
a custom rendering or JavaScript engine.

> **Development status:** This is an experimental early-development project, not a mature
> daily-use browser. Features and compatibility can change, and some browser
> behavior is intentionally incomplete. Review the limitations below before
> using it with important data.

## What is implemented

- Native WebView2 page tabs, tab reordering, multiple windows, tab groups, and
  configurable sleeping for inactive tabs.
- An address/search bar, URL handling, navigation controls, and session restore
  for the main window.
- SQLite-backed bookmarks and folders, searchable history, download records,
  and browser settings.
- Built-in network filtering based on EasyList, EasyPrivacy, URLHaus, and
  optional user rules/lists, with a bounded set of cosmetic filters.
- Private windows, optional HTTPS-only upgrades, per-site permission controls,
  cookie request controls, and controls for clearing cookies/cache.
- A Windows DPAPI-protected password vault for manually saving, viewing, and
  deleting credentials.
- Print and Save as PDF, find-in-page (`Ctrl+F`), WebView2 DevTools (`F12`),
  user scripts, tab sleeping, and an in-app updater with stable and beta
  release channels.

See [ARCHITECTURE.md](./ARCHITECTURE.md) for implementation details and the
current roadmap.

## Important limitations

- **Ad blocking is not comprehensive.** Filter and cosmetic-rule support is
  bounded, and behavior varies by site and WebView2 version. It is not a
  guarantee that every ad, tracker, or pop-up will be blocked.
- **Sleeping tabs reload.** Sleeping discards the page webview; selecting it
  reloads the URL but does not restore page-local state or back/forward history.
- **Downloads are basic.** The app records downloads and supports open/reveal
  actions, but does not provide byte progress, pause/resume/cancel, or an
  “ask where to save” dialog.
- **Some data features are limited.** Bookmark HTML import/export is flat and
  does not preserve folder nesting. Only the main window is restored after
  restarting the app.
- **Privacy controls have boundaries.** Third-party cookie control removes
  cookies from cross-site requests; WebView2 may still store cookies received
  from responses. Clear-on-exit currently covers cookies and disk cache, not
  all site storage or history. Private mode is not anonymity: websites and
  network providers can still observe normal connection data.
- **The password vault does not autofill.** Passwords are protected with
  Windows DPAPI, but automatic form filling, address/payment profiles, a master
  password, and vault import/export are not implemented. Origin metadata is
  stored for lookup.
- **User scripts are powerful.** Enabled scripts run in the matching normal
  page context and are not isolated from that page's JavaScript. Only enable
  scripts you trust; they do not run in private windows or subframes.
- **The release is unsigned.** Windows may display a SmartScreen warning.
- **Printing uses WebView2.** The print dialog and direct PDF export require a
  WebView2 Runtime that exposes the relevant printing interfaces.

## Download and run

When a release is available, download the
`eefi-browser-<version>-windows-x64.zip` asset from GitHub Releases, extract it,
and run `Eefi Browser.exe`.

The ZIP requires the **WebView2 Evergreen Runtime**. It is present on most
current Windows 10 and Windows 11 installations. If the browser does not start,
install the runtime from the
[official WebView2 page](https://developer.microsoft.com/microsoft-edge/webview2/).

The ZIP contains the production executable and a short runtime/readme notice.
The browser UI is bundled into the application; users do not need Node.js, Rust,
or a Vite server.

The updater checks published GitHub Releases and verifies update executables
against GitHub's SHA-256 asset digest. Stable builds ignore pre-releases; the
beta channel also receives pre-releases and later stable releases. Startup
checks prompt before installing by default; automatic installation is optional
in **Settings > Updates**. Updates replace the executable in the portable
folder, so that folder must be writable. The first release containing the
updater must be downloaded manually by users of earlier versions; later
releases can be installed in-app.

## Data and privacy

Browser data such as bookmarks, history, downloads, settings, and saved session
state is stored locally in the app's data directory. These SQLite records are
not generally encrypted. Password credentials are separately protected with
Windows DPAPI. Private windows use WebView2's incognito mode and do not persist
their session tabs, history, or download metadata.

The project does not include your existing browser profile in its source or
release ZIP. Do not share your app data directory, database files, or
credentials when reporting a problem.

## Build from source

### Requirements

- Windows 10 or later, x64.
- Node.js 22 and npm.
- Rust stable with the MSVC toolchain.
- Visual Studio Build Tools with the **Desktop development with C++** workload.
- Microsoft Edge WebView2 Runtime.

Install dependencies and run the development app from the repository root:

```powershell
node scripts/sync-version.mjs
npm ci
npm run tauri -- dev
```

The root `VERSION` file is the source of truth for the application version. Use
SemVer without a leading `v` (for example `0.2.0` or `0.3.0-beta.1`). The npm
development, check, build, and Tauri commands synchronize the package manifests
and lockfiles automatically; run `node scripts/sync-version.mjs` before `npm
ci` or standalone Cargo commands after changing `VERSION`. Release tags must
still be created manually as `v<VERSION>`.

The development command starts Vite. To type-check and build the frontend:

```powershell
npm run check
npm run build
```

To run the Rust tests:

```powershell
cargo test --manifest-path src-tauri\Cargo.toml
```

To build the production Windows executable without creating an installer:

```powershell
npm run tauri -- build --no-bundle
```

Tauri runs `npm run build` automatically for production and bundles the built
frontend with the app. The executable is written to
`src-tauri\target\release\eefi-browser.exe`.

## Continuous integration and releases

The workflows are in [`.github/workflows`](./.github/workflows/):

- **Tests** runs on branch pushes and pull requests, and can also be started
  manually from the GitHub Actions tab. It checks/builds the frontend and runs
  the Rust tests on a Windows runner. A local commit alone does not trigger it;
  push the commit to GitHub.
- **Windows portable release** runs when a `v*` version tag is pushed, for
  example `v0.1.0`. It checks the frontend, builds the app, packages the ZIP,
  and creates a GitHub Release with both the ZIP and updater executable
  attached. Tags containing a prerelease suffix (for example
  `v0.2.0-beta.1`) are published as GitHub pre-releases. The tag version must
  match the root `VERSION` file; the workflow synchronizes the package metadata
  and lockfiles from it before building.

These workflows run independently: the release workflow does not wait for the
Tests workflow. Check that CI has passed before pushing a release tag.

## Changelog

See [CHANGELOG.md](./CHANGELOG.md). Add work under **Unreleased** as it is
completed, then move those notes under a version and release date when
publishing.

## Contributing and bug reports

Contributions and reproducible bug reports are welcome. Before opening a
change, run the frontend checks/build and Rust tests listed above. For bugs,
include your Windows version, WebView2 Runtime version if known, steps to
reproduce, and sanitized error messages. Do not attach browser databases,
passwords, tokens, or other private profile data.

## License

Eefi Browser is licensed under **GPL-3.0-or-later**; see [LICENSE](./LICENSE).
Third-party component notices are in [THIRD_PARTY_NOTICES.md](./THIRD_PARTY_NOTICES.md).

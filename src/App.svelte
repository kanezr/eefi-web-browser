<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { ask, save } from "@tauri-apps/plugin-dialog";
  import Library from "./Library.svelte";

  type Tab = {
    id: string;
    title: string;
    url: string;
    groupId: string | null;
    sleeping: boolean;
  };
  type TabGroup = { id: string; name: string; color: string };

  type PageEvent = {
    tabId: string;
    url: string;
    title?: string;
  };
  type FindResult = {
    tabId: string;
    query: string;
    requestId: number;
    matchCount: number;
    activeMatchIndex: number;
    error: string | null;
  };
  type LibraryView =
    | "bookmarks"
    | "history"
    | "downloads"
    | "passwords"
    | "privacy"
    | "settings"
    | "extensions"
    | "tab-groups";
  type Bookmark = { id: number; title: string; url: string; onBar: boolean };
  type Suggestion = { title: string; url: string };
  type AdblockStatus = { enabled: boolean; siteAllowed: boolean; blockedCount: number };
  type UpdaterSettings = {
    channel: "stable" | "beta";
    automaticUpdates: boolean;
    checkOnStartup: boolean;
  };
  type UpdateCheck = {
    currentVersion: string;
    latestVersion: string | null;
    updateAvailable: boolean;
  };

  let tabs: Tab[] = $state([]);
  let tabGroups: TabGroup[] = $state([]);
  let activeTabId = $state("");
  let privateMode = $state(false);
  let bookmarksBar: Bookmark[] = $state([]);
  let managerView = $state<LibraryView | null>(null);
  let browserMenuOpen = $state(false);
  let contextTabId = $state("");
  let contextMenuPosition = $state({ x: 0, y: 0 });
  let browserMenuElement: HTMLDivElement;
  let tabContextMenuElement = $state<HTMLDivElement | undefined>(undefined);
  let address = $state("");
  let addressInput: HTMLInputElement;
  let findInput = $state<HTMLInputElement>();
  let contentArea: HTMLDivElement;
  let dragTabId = "";
  let isLoading = $state(false);
  let message = $state("");
  let isBookmarked = $state(false);
  let suggestions = $state<Suggestion[]>([]);
  let adblockEnabled = $state(true);
  let currentSiteAllowed = $state(false);
  let blockedCount = $state(0);
  let findOpen = $state(false);
  let findQuery = $state("");
  let findMatchCount = $state(0);
  let findActiveMatchIndex = $state(-1);
  let findError = $state("");
  let findRequestId = 0;
  let suggestionTimer: number | undefined;
  let suggestionRequest = 0;
  let findTimer: number | undefined;

  const activeTab = $derived(tabs.find((tab) => tab.id === activeTabId));
  const isSecure = $derived(activeTab?.url.startsWith("https://") ?? false);
  const activeHost = $derived(hostForUrl(activeTab?.url ?? ""));

  function hostForUrl(url: string) {
    try {
      return new URL(url).hostname;
    } catch {
      return "";
    }
  }

  async function refreshTabGroups() {
    try {
      const [groups, currentTabs] = await Promise.all([
        invoke<TabGroup[]>("tab_groups_list"),
        invoke<Tab[]>("list_tabs"),
      ]);
      tabGroups = groups;
      tabs = currentTabs;
    } catch (error) {
      showError(error);
    }
  }

  function groupColor(groupId: string | null) {
    return tabGroups.find((group) => group.id === groupId)?.color ?? "transparent";
  }

  async function refreshAdblockStatus(url = activeTab?.url ?? "") {
    const host = hostForUrl(url);
    try {
      const status = await invoke<AdblockStatus>("adblock_status", { host });
      adblockEnabled = status.enabled;
      currentSiteAllowed = status.siteAllowed;
      blockedCount = status.blockedCount;
    } catch (error) {
      showError(error);
    }
  }

  async function toggleAdblock() {
    try {
      await invoke("adblock_set_enabled", { enabled: !adblockEnabled });
      adblockEnabled = !adblockEnabled;
    } catch (error) {
      showError(error);
    }
  }

  async function toggleSiteAdblock() {
    if (!activeHost) return;
    try {
      await invoke("adblock_set_site_allowed", {
        host: activeHost,
        allowed: !currentSiteAllowed,
      });
      await refreshAdblockStatus();
    } catch (error) {
      showError(error);
    }
  }

  async function refreshBookmarkBar() {
    bookmarksBar = await invoke<Bookmark[]>("bookmarks_list", {
      query: null,
      barOnly: true,
      folderId: null,
    });
  }

  async function refreshBookmarkStatus(url = activeTab?.url ?? "") {
    if (!/^https?:\/\//i.test(url)) {
      isBookmarked = false;
      return;
    }
    try {
      isBookmarked = await invoke<boolean>("bookmark_is_saved", { url });
    } catch (error) {
      showError(error);
    }
  }

  async function newTab() {
    try {
      if (findOpen) closeFind();
      if (managerView) await closeManager();
      const tab = await invoke<Tab>("create_tab");
      tabs = [...tabs, tab];
      activeTabId = tab.id;
      address = "";
      isLoading = false;
      await resizeContent();
      focusAddress();
    } catch (error) {
      showError(error);
    }
  }

  async function closeTab(tabId: string) {
    try {
      if (findOpen && tabId === activeTabId) closeFind();
      await invoke<Tab[]>("close_tab", { tabId });
      tabs = await invoke<Tab[]>("list_tabs");
      if (!tabs.some((tab) => tab.id === activeTabId)) {
        activeTabId = tabs[0]?.id ?? "";
      }
      address = activeTab?.url === "about:blank" ? "" : activeTab?.url ?? "";
      await resizeContent();
    } catch (error) {
      showError(error);
    }
  }

  async function selectTab(tabId: string) {
    try {
      if (findOpen && tabId !== activeTabId) closeFind();
      await invoke("activate_tab", { tabId });
      tabs = await invoke<Tab[]>("list_tabs");
      activeTabId = tabId;
      address = tabs.find((tab) => tab.id === tabId)?.url ?? "";
      isLoading = false;
      await refreshAdblockStatus(address);
      await resizeContent();
    } catch (error) {
      showError(error);
    }
  }

  async function navigate() {
    const input = address.trim();
    if (!input || !activeTabId) return;
    try {
      if (browserMenuOpen) await closeBrowserMenu();
      isLoading = true;
      message = "";
      const url = await invoke<string>("navigate", {
        tabId: activeTabId,
        input,
      });
      address = url;
    } catch (error) {
      isLoading = false;
      showError(error);
    }
  }

  function updateSuggestions() {
    if (suggestionTimer !== undefined) window.clearTimeout(suggestionTimer);
    const query = address.trim();
    const request = ++suggestionRequest;
    if (query.length < 2) {
      suggestions = [];
      return;
    }
    suggestionTimer = window.setTimeout(async () => {
      try {
        const [visited, saved] = await Promise.all([
          invoke<Array<{ title: string; url: string }>>("history_list", {
            query,
            limit: 6,
          }),
          invoke<Bookmark[]>("bookmarks_list", {
            query,
            barOnly: false,
            folderId: null,
          }),
        ]);
        if (request !== suggestionRequest) return;
        const unique = new Map<string, Suggestion>();
        for (const entry of [...saved, ...visited]) {
          if (!unique.has(entry.url)) unique.set(entry.url, { title: entry.title, url: entry.url });
        }
        suggestions = [...unique.values()].slice(0, 8);
      } catch (error) {
        if (request === suggestionRequest) showError(error);
      }
    }, 140);
  }

  async function navigateFromLibrary(url: string) {
    await closeManager();
    address = url;
    await navigate();
  }

  async function toggleBookmark() {
    const url = activeTab?.url ?? "";
    if (!/^https?:\/\//i.test(url) || !activeTab) return;
    try {
      if (isBookmarked) {
        const matches = await invoke<Bookmark[]>("bookmarks_list", {
          query: url,
          barOnly: false,
          folderId: null,
        });
        const bookmark = matches.find((entry) => entry.url === url);
        if (bookmark) await invoke("bookmark_delete", { id: bookmark.id });
      } else {
        await invoke("bookmark_create", {
          title: activeTab.title || url,
          url,
          folderId: null,
          onBar: true,
        });
      }
      await refreshBookmarkBar();
      await refreshBookmarkStatus(url);
    } catch (error) {
      showError(error);
    }
  }

  async function openManager(view: LibraryView) {
    try {
      if (findOpen) closeFind();
      await invoke("set_page_content_visible", { visible: false });
      browserMenuOpen = false;
      managerView = view;
    } catch (error) {
      showError(error);
    }
  }

  function toggleBrowserMenu() {
    browserMenuOpen = !browserMenuOpen;
  }

  function closeBrowserMenu() {
    browserMenuOpen = false;
  }

  function closeMenuFromOutside(event: PointerEvent) {
    if (browserMenuOpen && !browserMenuElement?.contains(event.target as Node)) {
      void closeBrowserMenu();
    }
    if (contextTabId && !tabContextMenuElement?.contains(event.target as Node)) {
      contextTabId = "";
    }
  }

  async function newTabFromMenu() {
    await closeBrowserMenu();
    await newTab();
  }

  async function openPrivateWindow() {
    try {
      await closeBrowserMenu();
      await invoke<string>("open_private_window");
    } catch (error) {
      showError(error);
    }
  }

  async function openBrowserWindow() {
    try {
      await closeBrowserMenu();
      await invoke<string>("open_browser_window");
    } catch (error) {
      showError(error);
    }
  }

  function showTabContextMenu(event: MouseEvent, tabId: string) {
    event.preventDefault();
    contextTabId = tabId;
    contextMenuPosition = { x: event.clientX, y: event.clientY };
  }

  async function detachTabToNewWindow() {
    const tabId = contextTabId;
    if (!tabId) return;
    contextTabId = "";
    try {
      await invoke<string>("detach_tab_to_new_window", { tabId });
      tabs = await invoke<Tab[]>("list_tabs");
      tabGroups = await invoke<TabGroup[]>("tab_groups_list");
      activeTabId = await invoke<string | null>("active_tab_id") ?? tabs[0]?.id ?? "";
      address = activeTab?.url === "about:blank" ? "" : activeTab?.url ?? "";
      isLoading = false;
      await refreshAdblockStatus(address);
      await refreshBookmarkStatus(address);
      await resizeContent();
    } catch (error) {
      showError(error);
    }
  }

  async function closeManager() {
    if (!managerView) return;
    managerView = null;
    try {
      await invoke("set_page_content_visible", { visible: true });
      await resizeContent();
    } catch (error) {
      showError(error);
    }
  }

  async function runAction(action: "go_back" | "go_forward" | "reload" | "stop" | "go_home") {
    if (!activeTabId) return;
    try {
      if (browserMenuOpen) await closeBrowserMenu();
      await invoke(action, { tabId: activeTabId });
      if (action === "reload" || action === "go_home") isLoading = true;
      if (action === "stop") isLoading = false;
      if (action === "go_home") address = "https://www.google.com/";
    } catch (error) {
      showError(error);
    }
  }

  function openFind() {
    if (managerView) return;
    findOpen = true;
    findError = "";
    requestAnimationFrame(() => {
      findInput?.focus();
      findInput?.select();
      void resizeContent();
      if (findQuery) void findInPage(0);
    });
  }

  function closeFind() {
    if (!findOpen) return;
    findOpen = false;
    findError = "";
    findMatchCount = 0;
    findActiveMatchIndex = -1;
    if (findTimer !== undefined) window.clearTimeout(findTimer);
    const tabId = activeTabId;
    if (tabId) {
      const requestId = ++findRequestId;
      void invoke("find_in_page", {
        tabId,
        query: "",
        requestId,
        direction: 0,
        startNewSearch: false,
        restorePageFocus: true,
      }).catch(showError);
    }
    requestAnimationFrame(() => void resizeContent());
  }

  function scheduleFind() {
    if (findTimer !== undefined) window.clearTimeout(findTimer);
    findError = "";
    findTimer = window.setTimeout(() => {
      findTimer = undefined;
      void findInPage(0);
    }, 100);
  }

  async function findInPage(direction: -1 | 0 | 1) {
    if (!activeTabId) return;
    const pendingInput = findTimer !== undefined;
    if (pendingInput) {
      window.clearTimeout(findTimer);
      findTimer = undefined;
    }
    const startNewSearch = direction === 0 || pendingInput;
    const requestId = ++findRequestId;
    try {
      await invoke("find_in_page", {
        tabId: activeTabId,
        query: findQuery,
        requestId,
        direction,
        startNewSearch,
        restorePageFocus: false,
      });
    } catch (error) {
      findError = error instanceof Error ? error.message : String(error);
    }
  }

  async function toggleDevtools() {
    try {
      if (browserMenuOpen) await closeBrowserMenu();
      await invoke("toggle_devtools");
    } catch (error) {
      showError(error);
    }
  }

  function pdfFilename(title: string) {
    let filename = title
      .replace(/\.pdf$/i, "")
      .replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_")
      .replace(/[. ]+$/g, "")
      .trim()
      .slice(0, 120);
    if (!filename || /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i.test(filename)) {
      filename = filename ? `_${filename}` : "webpage";
    }
    return `${filename}.pdf`;
  }

  async function printPage() {
    try {
      if (browserMenuOpen) await closeBrowserMenu();
      await invoke("print_page");
    } catch (error) {
      showError(error);
    }
  }

  async function savePageAsPdf() {
    if (!activeTab || !activeTabId) return;
    if (activeTab.sleeping) {
      showError("Wake the sleeping tab before saving it as a PDF.");
      return;
    }
    const tabId = activeTabId;
    try {
      if (browserMenuOpen) await closeBrowserMenu();
      const filePath = await save({
        title: "Save page as PDF",
        defaultPath: pdfFilename(activeTab.title || "webpage"),
        filters: [{ name: "PDF document", extensions: ["pdf"] }],
      });
      if (!filePath) return;
      const savedPath = await invoke<string>("save_page_as_pdf", { tabId, filePath });
      message = `PDF saved: ${savedPath}`;
      window.setTimeout(() => {
        if (message) message = "";
      }, 5000);
    } catch (error) {
      showError(error);
    }
  }

  async function resizeContent() {
    if (!contentArea) return;
    const rect = contentArea.getBoundingClientRect();
    try {
      await invoke("resize_active_view", {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
      });
    } catch (error) {
      // The view can briefly be absent while the last tab is closing.
      if (activeTabId) showError(error);
    }
  }

  function focusAddress() {
    requestAnimationFrame(() => {
      addressInput?.focus();
      addressInput?.select();
    });
  }

  function showError(error: unknown) {
    message = error instanceof Error ? error.message : String(error);
    window.setTimeout(() => {
      if (message) message = "";
    }, 5000);
  }

  async function checkForUpdatesAtStartup() {
    if (privateMode || getCurrentWindow().label !== "main") return;
    try {
      const settings = await invoke<UpdaterSettings>("updater_settings_get");
      if (!settings.checkOnStartup) return;
      const result = await invoke<UpdateCheck>("updater_check");
      if (!result.updateAvailable || !result.latestVersion) return;

      if (settings.automaticUpdates) {
        message = `Installing Eefi Browser v${result.latestVersion}…`;
        await invoke("updater_install", { version: result.latestVersion });
        return;
      }

      const accepted = await ask(
        `Eefi Browser v${result.latestVersion} is available. Download and install it now?`,
        { title: "Eefi Browser update", kind: "info" },
      );
      if (accepted) {
        await invoke("updater_install", { version: result.latestVersion });
      }
    } catch (error) {
      showError(error);
    }
  }

  function onTabKeydown(event: KeyboardEvent, tabId: string) {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      void selectTab(tabId);
    }
  }

  function dropTab(targetId: string) {
    if (!dragTabId || dragTabId === targetId) return;
    const from = tabs.findIndex((tab) => tab.id === dragTabId);
    const to = tabs.findIndex((tab) => tab.id === targetId);
    if (from < 0 || to < 0) return;
    const reordered = [...tabs];
    const [moved] = reordered.splice(from, 1);
    reordered.splice(to, 0, moved);
    tabs = reordered;
    dragTabId = "";
    void invoke("reorder_tabs", { order: tabs.map((tab) => tab.id) }).catch(showError);
  }

  onMount(() => {
    let unlistenNavigation: (() => void) | undefined;
    let unlistenTitle: (() => void) | undefined;
    let unlistenTabSleep: (() => void) | undefined;
    let unlistenFindOpen: (() => void) | undefined;
    let unlistenFindResult: (() => void) | undefined;
    let observer: ResizeObserver | undefined;
    let disposed = false;

    void (async () => {
      try {
        unlistenNavigation = await listen<PageEvent>("browser:navigation", ({ payload }) => {
          tabs = tabs.map((tab) =>
            tab.id === payload.tabId ? { ...tab, url: payload.url } : tab,
          );
          if (payload.tabId === activeTabId) {
            address = payload.url === "about:blank" ? "" : payload.url;
            isLoading = payload.title !== "finished";
            void refreshBookmarkStatus(payload.url);
            void refreshAdblockStatus(payload.url);
          }
        });
        unlistenTitle = await listen<PageEvent>("browser:title", ({ payload }) => {
          tabs = tabs.map((tab) =>
            tab.id === payload.tabId ? { ...tab, title: payload.title ?? tab.title } : tab,
          );
        });
        unlistenTabSleep = await listen<{ tabId: string; sleeping: boolean }>(
          "browser:tab-sleep",
          ({ payload }) => {
            tabs = tabs.map((tab) =>
              tab.id === payload.tabId ? { ...tab, sleeping: payload.sleeping } : tab,
            );
          },
        );
        unlistenFindOpen = await listen("browser:find-open", () => openFind());
        unlistenFindResult = await listen<FindResult>(
          "browser:find-result",
          ({ payload }) => {
            if (
              payload.tabId !== activeTabId ||
              payload.query !== findQuery ||
              payload.requestId !== findRequestId ||
              !findOpen
            ) {
              return;
            }
            findMatchCount = payload.matchCount;
            findActiveMatchIndex = payload.activeMatchIndex;
            findError = payload.error ?? "";
          },
        );
        if (disposed) {
          unlistenNavigation();
          unlistenTitle();
          unlistenTabSleep();
          unlistenFindOpen();
          unlistenFindResult();
          return;
        }
        privateMode = await invoke<boolean>("browser_window_mode");
        await invoke<string>("download_directory");
        if (!privateMode) await invoke("downloads_recover");
        tabs = await invoke<Tab[]>("restore_session");
        tabGroups = await invoke<TabGroup[]>("tab_groups_list");
        activeTabId = await invoke<string | null>("active_tab_id") ?? tabs[0]?.id ?? "";
        address = activeTab?.url === "about:blank" ? "" : activeTab?.url ?? "";
        await refreshAdblockStatus(address);
        await refreshBookmarkBar();
        await refreshBookmarkStatus();
        await resizeContent();
        focusAddress();
        window.setTimeout(() => void checkForUpdatesAtStartup(), 1500);
        observer = new ResizeObserver(() => void resizeContent());
        if (contentArea) observer.observe(contentArea);
      } catch (error) {
        showError(error);
      }
    })();

    const onKeydown = (event: KeyboardEvent) => {
      const modifier = event.ctrlKey || event.metaKey;
      if (modifier && event.key.toLowerCase() === "p") {
        event.preventDefault();
        void printPage();
      } else if (modifier && event.key.toLowerCase() === "f") {
        event.preventDefault();
        openFind();
      } else if (modifier && event.key.toLowerCase() === "l") {
        event.preventDefault();
        focusAddress();
      } else if (modifier && event.key.toLowerCase() === "t") {
        event.preventDefault();
        void newTab();
      } else if (modifier && event.key.toLowerCase() === "w") {
        event.preventDefault();
        if (activeTabId) void closeTab(activeTabId);
      } else if (modifier && event.key.toLowerCase() === "r") {
        event.preventDefault();
        void runAction("reload");
      } else if (modifier && event.key.toLowerCase() === "d") {
        event.preventDefault();
        void toggleBookmark();
      } else if (modifier && event.key.toLowerCase() === "h") {
        event.preventDefault();
        void openManager("history");
      } else if (modifier && event.key.toLowerCase() === "j") {
        event.preventDefault();
        void openManager("downloads");
      } else if (event.altKey && event.key === "ArrowLeft") {
        event.preventDefault();
        void runAction("go_back");
      } else if (event.altKey && event.key === "ArrowRight") {
        event.preventDefault();
        void runAction("go_forward");
      } else if (event.key === "F5") {
        event.preventDefault();
        void runAction("reload");
      } else if (event.key === "F12") {
        event.preventDefault();
        void toggleDevtools();
      } else if (event.key === "Escape" && contextTabId) {
        event.preventDefault();
        contextTabId = "";
      } else if (event.key === "Escape" && findOpen) {
        event.preventDefault();
        closeFind();
      } else if (event.key === "Escape" && browserMenuOpen) {
        event.preventDefault();
        void closeBrowserMenu();
      } else if (event.key === "Escape" && isLoading) {
        void runAction("stop");
      }
    };
    window.addEventListener("keydown", onKeydown);

    return () => {
      disposed = true;
      unlistenNavigation?.();
      unlistenTitle?.();
      unlistenTabSleep?.();
      unlistenFindOpen?.();
      unlistenFindResult?.();
      observer?.disconnect();
      window.removeEventListener("keydown", onKeydown);
      if (findTimer !== undefined) window.clearTimeout(findTimer);
    };
  });
</script>

<svelte:head>
  <title>{privateMode ? "Private - " : ""}{activeTab?.title ? `${activeTab.title} - Eefi Browser` : "Eefi Browser"}</title>
</svelte:head>

<svelte:window onpointerdown={closeMenuFromOutside} />

<main class:find-open={findOpen} class:menu-open={browserMenuOpen} class="browser">
  <div class="tab-row" role="tablist" aria-label="Browser tabs">
    {#each tabs as tab (tab.id)}
      {@const group = tabGroups.find((entry) => entry.id === tab.groupId)}
      <div
        class:active={tab.id === activeTabId}
        class:grouped={group !== undefined}
        class="tab"
        style={`--tab-group-color: ${groupColor(tab.groupId)}`}
        role="tab"
        aria-selected={tab.id === activeTabId}
        tabindex="0"
        draggable="true"
        onclick={() => void selectTab(tab.id)}
        onkeydown={(event) => onTabKeydown(event, tab.id)}
        ondragstart={() => (dragTabId = tab.id)}
        ondragover={(event) => event.preventDefault()}
        ondrop={() => dropTab(tab.id)}
        onauxclick={(event) => {
          if (event.button === 1) void closeTab(tab.id);
        }}
        oncontextmenu={(event) => showTabContextMenu(event, tab.id)}
        title={`${group ? `${group.name} · ` : ""}${tab.url || "New tab"}`}
      >
        {#if group}<span class="tab-group-mark" aria-hidden="true"></span>{/if}
        {#if tab.sleeping}<span class="tab-sleep-indicator" aria-label="Sleeping tab" title="Sleeping tab">Zz</span>{/if}
        <span class="tab-favicon">{tab.url.startsWith("https://") ? "●" : "○"}</span>
        <span class="tab-title">{tab.title || (tab.url === "about:blank" ? "New tab" : tab.url)}</span>
        <button
          class="tab-close"
          aria-label="Close tab"
          onclick={(event) => {
            event.stopPropagation();
            void closeTab(tab.id);
          }}
        >×</button>
      </div>
    {/each}
    {#if contextTabId}
      <div
        class="tab-context-menu"
        bind:this={tabContextMenuElement}
        role="menu"
        style={`left: ${contextMenuPosition.x}px; top: ${contextMenuPosition.y}px`}
      >
        <button role="menuitem" onclick={() => void detachTabToNewWindow()}>
          Move tab to new window
        </button>
      </div>
    {/if}
    <button class="new-tab" aria-label="New tab" title="New tab (Ctrl+T)" onclick={() => void newTab()}>
      +
    </button>
    <div class="window-fill"></div>
    {#if privateMode}<span class="private-mark">Private</span>{/if}
    <span class="brand-mark" aria-label="Eefi Browser">eefi</span>
  </div>

  <nav class="toolbar" aria-label="Navigation toolbar">
    <div class="navigation-controls">
      <button class="icon-button" aria-label="Back" title="Back (Alt+Left)" onclick={() => void runAction("go_back")}>←</button>
      <button class="icon-button" aria-label="Forward" title="Forward (Alt+Right)" onclick={() => void runAction("go_forward")}>→</button>
      <button
        class="icon-button"
        aria-label={isLoading ? "Stop loading" : "Reload"}
        title={isLoading ? "Stop" : "Reload (Ctrl+R)"}
        onclick={() => void runAction(isLoading ? "stop" : "reload")}
      >{isLoading ? "×" : "↻"}</button>
      <button class="icon-button home-button" aria-label="Home" title="Home" onclick={() => void runAction("go_home")}>⌂</button>
    </div>

    <form class="omnibox" onsubmit={(event) => { event.preventDefault(); void navigate(); }}>
      <span class:secure={isSecure} class="security-indicator" aria-label={isSecure ? "Secure connection" : "Connection not secure"}>
        {isSecure ? "▣" : "ⓘ"}
      </span>
      <input
        bind:this={addressInput}
        bind:value={address}
        list="omnibox-suggestions"
        oninput={updateSuggestions}
        aria-label="Search or enter address"
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        placeholder="Search or enter address"
      />
      <datalist id="omnibox-suggestions">
        {#each suggestions as suggestion (suggestion.url)}
          <option value={suggestion.url}>{suggestion.title}</option>
        {/each}
      </datalist>
      {#if isLoading}<span class="loading-indicator" aria-label="Loading"></span>{/if}
      <button class="go-button" type="submit" aria-label="Go">↵</button>
    </form>

    <button
      class:bookmarked={isBookmarked}
      class="icon-button library-action"
      aria-label={isBookmarked ? "Remove bookmark" : "Bookmark this page"}
      title={isBookmarked ? "Remove bookmark" : "Bookmark this page"}
      onclick={() => void toggleBookmark()}
    >{isBookmarked ? "★" : "☆"}</button>
    <button class="icon-button library-action" aria-label="Bookmarks" title="Bookmarks" onclick={() => void openManager("bookmarks")}>▤</button>
    <button class="icon-button library-action" aria-label="History" title="History" onclick={() => void openManager("history")}>◷</button>
    <button class="icon-button library-action" aria-label="Downloads" title="Downloads" onclick={() => void openManager("downloads")}>↓</button>
    <button class="icon-button library-action" aria-label="Passwords" title="Password vault" onclick={() => void openManager("passwords")}>⚿</button>
    <button class="icon-button library-action" aria-label="Privacy settings" title="Privacy settings" onclick={() => void openManager("privacy")}>⚙</button>
    <button
      class:site-allowed={currentSiteAllowed}
      class="icon-button adblock-button"
      aria-label={currentSiteAllowed ? `Enable ad blocking on ${activeHost}` : `Allow ads on ${activeHost}`}
      title={`${currentSiteAllowed ? "Enable" : "Disable"} ad blocking for ${activeHost || "this site"}${blockedCount ? ` · ${blockedCount} blocked` : ""}`}
      disabled={!activeHost}
      onclick={() => void toggleSiteAdblock()}
    >
      ⛨{#if blockedCount}<span class="blocked-count">{blockedCount > 99 ? "99+" : blockedCount}</span>{/if}
    </button>
    <button
      class:adblock-off={!adblockEnabled}
      class="icon-button adblock-button"
      aria-label={adblockEnabled ? "Disable ad blocker" : "Enable ad blocker"}
      title={adblockEnabled ? "Disable ad blocker" : "Enable ad blocker"}
      onclick={() => void toggleAdblock()}
    >◈</button>
    <div class="browser-menu" bind:this={browserMenuElement}>
      <button
        class="icon-button menu-button"
        aria-label="Browser menu"
        aria-haspopup="menu"
        aria-expanded={browserMenuOpen}
        title="Browser menu"
        onclick={() => void toggleBrowserMenu()}
      >⋯</button>
      {#if browserMenuOpen}
        <div class="browser-menu-panel" role="menu" aria-label="Browser menu">
          <button role="menuitem" onclick={() => void newTabFromMenu()}>New tab</button>
          {#if !privateMode}
            <button role="menuitem" onclick={() => void openBrowserWindow()}>New window</button>
            <button role="menuitem" onclick={() => void openPrivateWindow()}>New private window</button>
          {/if}
          <button role="menuitem" onclick={() => void openManager("bookmarks")}>Bookmarks</button>
          <button role="menuitem" onclick={() => void openManager("history")}>History</button>
          <button role="menuitem" onclick={() => void openManager("downloads")}>Downloads</button>
          <button role="menuitem" onclick={() => void openManager("passwords")}>Password vault</button>
          <button role="menuitem" onclick={() => void openManager("extensions")}>User scripts</button>
          <button role="menuitem" onclick={() => void openManager("tab-groups")}>Tab groups</button>
          <button role="menuitem" onclick={() => void openManager("settings")}>Settings</button>
          <hr />
          <button role="menuitem" onclick={() => void printPage()}>Print… (Ctrl+P)</button>
          <button role="menuitem" onclick={() => void savePageAsPdf()}>Save as PDF…</button>
          <button role="menuitem" onclick={() => void toggleDevtools()}>Developer tools (F12)</button>
          <hr />
          <button role="menuitem" onclick={() => void openManager("privacy")}>Privacy settings</button>
        </div>
      {/if}
    </div>
  </nav>

  <div class="bookmark-bar" aria-label="Bookmarks bar">
    {#each bookmarksBar as bookmark (bookmark.id)}
      <button title={bookmark.url} onclick={() => void navigateFromLibrary(bookmark.url)}>
        <span>◆</span>{bookmark.title}
      </button>
    {:else}
      <span class="bookmark-bar-empty">Bookmarks you add to the bar will appear here</span>
    {/each}
  </div>

  {#if findOpen}
    <div class="find-bar" role="search" aria-label="Find in page">
      <input
        bind:this={findInput}
        bind:value={findQuery}
        aria-label="Find in page"
        placeholder="Find in page"
        autocomplete="off"
        spellcheck="false"
        oninput={scheduleFind}
        onkeydown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            void findInPage(event.shiftKey ? -1 : 1);
          } else if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            closeFind();
          }
        }}
      />
      <span class:error={findError} class="find-count" role="status">
        {findError || (findQuery ? `${findActiveMatchIndex > 0 ? findActiveMatchIndex : 0} of ${findMatchCount}` : "")}
      </span>
      <button
        class="icon-button find-nav"
        aria-label="Previous match"
        title="Previous match (Shift+Enter)"
        onclick={() => void findInPage(-1)}
      >↑</button>
      <button
        class="icon-button find-nav"
        aria-label="Next match"
        title="Next match (Enter)"
        onclick={() => void findInPage(1)}
      >↓</button>
      <button class="icon-button find-close" aria-label="Close find bar" title="Close (Escape)" onclick={closeFind}>
        ×
      </button>
    </div>
  {/if}

  <div class="content" bind:this={contentArea} aria-label="Web page content">
    {#if managerView}
      <Library
        view={managerView}
        currentUrl={activeTab?.url ?? ""}
        currentTitle={activeTab?.title ?? ""}
        isPrivate={privateMode}
        tabs={tabs}
        activeTabId={activeTabId}
        tabGroups={tabGroups}
        onNavigate={(url) => void navigateFromLibrary(url)}
        onReload={() => void runAction("reload")}
        onClose={() => void closeManager()}
        onBookmarksChange={() => void refreshBookmarkBar()}
        onGroupsChange={() => void refreshTabGroups()}
      />
    {/if}
  </div>

  {#if message}
    <div class="notice" role="status">{message}</div>
  {/if}
</main>

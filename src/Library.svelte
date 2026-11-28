<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { ask } from "@tauri-apps/plugin-dialog";

  type View =
    | "bookmarks"
    | "history"
    | "downloads"
    | "privacy"
    | "settings"
    | "extensions"
    | "tab-groups";
  type BrowserTab = {
    id: string;
    title: string;
    url: string;
    groupId: string | null;
    sleeping: boolean;
  };
  type TabGroup = { id: string; name: string; color: string };
  type Bookmark = {
    id: number;
    title: string;
    url: string;
    folderId: number | null;
    folderName: string | null;
    onBar: boolean;
    createdAt: number;
  };
  type Folder = { id: number; name: string; parentId: number | null };
  type HistoryEntry = { id: number; url: string; title: string; visitedAt: number };
  type Download = {
    id: number;
    url: string;
    path: string;
    status: string;
    startedAt: number;
    finishedAt: number | null;
  };
  type DownloadProgress = {
    path: string;
    bytesReceived: number;
    totalBytes: number | null;
    paused: boolean;
  };
  type PrivacyStatus = {
    httpsOnly: boolean;
    blockThirdPartyCookies: boolean;
    clearCookiesOnExit: boolean;
  };
  type CustomFilterConfig = { listUrls: string[]; userRules: string };
  type SitePermissionStatus = { origin: string; permissions: Record<string, boolean> };
  type UserScript = {
    id: number;
    name: string;
    matchPattern: string;
    source: string;
    enabled: boolean;
    updatedAt: number;
  };
  type UpdateChannel = "stable" | "beta";
  type UpdaterSettings = {
    channel: UpdateChannel;
    automaticUpdates: boolean;
    checkOnStartup: boolean;
  };
  type OnekoSettings = {
    enabled: boolean;
    speed: "slow" | "normal" | "fast";
    size: 1 | 2;
    showBrowserPages: boolean;
    hideOnSite: boolean;
  };
  type UpdateCheck = {
    currentVersion: string;
    latestVersion: string | null;
    updateAvailable: boolean;
  };

  type Props = {
    view: View;
    currentUrl: string;
    currentTitle: string;
    isPrivate: boolean;
    tabs: BrowserTab[];
    activeTabId: string;
    tabGroups: TabGroup[];
    onNavigate: (url: string) => void;
    onReload: () => void;
    onClose: () => void;
    onBookmarksChange: () => void;
    onGroupsChange: () => void;
  };

  let {
    view,
    currentUrl,
    currentTitle,
    isPrivate,
    tabs,
    activeTabId,
    tabGroups,
    onNavigate,
    onReload,
    onClose,
    onBookmarksChange,
    onGroupsChange,
  }: Props = $props();

  let bookmarks = $state<Bookmark[]>([]);
  let folders = $state<Folder[]>([]);
  let history = $state<HistoryEntry[]>([]);
  let downloads = $state<Download[]>([]);
  let downloadProgress = $state<Record<string, DownloadProgress>>({});
  let downloadControlPending = $state<Record<number, boolean>>({});
  let userScripts = $state<UserScript[]>([]);
  let query = $state("");
  let error = $state("");
  let editId = $state<number | null>(null);
  let title = $state("");
  let url = $state("");
  let folderId = $state("");
  let onBar = $state(false);
  let newFolderName = $state("");
  let historyRange = $state("all");
  let showClearDialog = $state(false);
  let clearHistory = $state(true);
  let clearCookies = $state(true);
  let clearCache = $state(true);
  let clearingData = $state(false);
  let confirmDangerousId = $state<number | null>(null);
  let downloadDirectory = $state("");
  let askWhereToSave = $state(false);
  let importing = $state(false);
  let scriptEditId = $state<number | null>(null);
  let scriptName = $state("");
  let scriptMatchPattern = $state("");
  let scriptSource = $state("");
  let scriptEnabled = $state(false);
  let scriptSaving = $state(false);
  let tabGroupEditId = $state<string | null>(null);
  let tabGroupName = $state("");
  let tabGroupColor = $state("#4f7ff0");
  let httpsOnly = $state(false);
  let blockThirdPartyCookies = $state(false);
  let clearCookiesOnExit = $state(false);
  let customFilterListUrls = $state("");
  let customUserRules = $state("");
  let customFiltersSaving = $state(false);
  let customFiltersMessage = $state("");
  let privacyLoading = $state(true);
  let permissionOrigin = $state("");
  let sitePermissions = $state<Record<string, boolean>>({});
  let savingSitePermissions = $state<Record<string, boolean>>({});
  let privacyLoadRequest = 0;
  let tabSleepTimeout = $state(900);
  let tabSleepSettingsLoading = $state(false);
  let savingTabSleepTimeout = $state(false);
  let onekoSettings = $state<OnekoSettings>({
    enabled: false,
    speed: "normal",
    size: 1,
    showBrowserPages: false,
    hideOnSite: false,
  });
  let onekoSettingsLoading = $state(false);
  let savingOnekoSettings = $state(false);
  let updaterSettings = $state<UpdaterSettings>({
    channel: "stable",
    automaticUpdates: false,
    checkOnStartup: true,
  });
  let updaterSettingsLoading = $state(false);
  let savingUpdaterSettings = $state(false);
  let checkingForUpdates = $state(false);
  let updaterStatus = $state("");
  let appVersion = $state("");

  const isHttpUrl = (value: string) => /^https?:\/\//i.test(value);
  const dangerousExtensions = new Set([
    "bat", "cmd", "com", "exe", "jar", "js", "lnk", "msi", "ps1", "reg", "scr", "vbs",
  ]);
  const permissionOptions = [
    { key: "camera", label: "Camera" },
    { key: "microphone", label: "Microphone" },
    { key: "location", label: "Location" },
    { key: "notifications", label: "Notifications" },
    { key: "clipboard", label: "Clipboard read access" },
  ];
  const currentPageAlreadyBookmarked = $derived(
    bookmarks.some((bookmark) => bookmark.url === currentUrl),
  );

  async function loadBookmarks() {
    try {
      const selectedFolder = folderId ? Number(folderId) : null;
      bookmarks = await invoke<Bookmark[]>("bookmarks_list", {
        query: query || null,
        barOnly: false,
        folderId: selectedFolder,
      });
      folders = await invoke<Folder[]>("bookmark_folders");
      onBookmarksChange();
    } catch (reason) {
      reportError(reason);
    }
  }

  async function loadHistory() {
    try {
      history = await invoke<HistoryEntry[]>("history_list", {
        query: query || null,
        limit: 500,
      });
    } catch (reason) {
      reportError(reason);
    }
  }

  async function loadDownloads() {
    try {
      const [items, progress, directory, askPreference] = await Promise.all([
        invoke<Download[]>("downloads_list", { limit: 250 }),
        invoke<DownloadProgress[]>("downloads_progress"),
        invoke<string>("download_directory"),
        invoke<boolean>("download_ask_where_to_save"),
      ]);
      downloads = items;
      downloadProgress = Object.fromEntries(progress.map((entry) => [entry.path, entry]));
      downloadDirectory = directory;
      askWhereToSave = askPreference;
    } catch (reason) {
      reportError(reason);
    }
  }

  async function loadUserScripts() {
    try {
      userScripts = await invoke<UserScript[]>("user_scripts_list");
    } catch (reason) {
      reportError(reason);
    }
  }

  async function loadTabSleepSettings() {
    tabSleepSettingsLoading = true;
    try {
      tabSleepTimeout = await invoke<number>("tab_sleep_timeout_get");
    } catch (reason) {
      reportError(reason);
    } finally {
      tabSleepSettingsLoading = false;
    }
  }

  async function loadUpdaterSettings() {
    updaterSettingsLoading = true;
    try {
      const [settings, version] = await Promise.all([
        invoke<UpdaterSettings>("updater_settings_get"),
        invoke<string>("updater_current_version"),
      ]);
      updaterSettings = settings;
      appVersion = version;
    } catch (reason) {
      reportError(reason);
    } finally {
      updaterSettingsLoading = false;
    }
  }

  async function loadOnekoSettings() {
    onekoSettingsLoading = true;
    try {
      onekoSettings = await invoke<OnekoSettings>("oneko_settings_get", { host: null });
    } catch (reason) {
      reportError(reason);
    } finally {
      onekoSettingsLoading = false;
    }
  }

  async function saveOnekoSettings(changes: Partial<OnekoSettings>) {
    if (savingOnekoSettings || onekoSettingsLoading) return;
    const previous = onekoSettings;
    const next = { ...previous, ...changes };
    onekoSettings = next;
    savingOnekoSettings = true;
    try {
      onekoSettings = await invoke<OnekoSettings>("oneko_settings_set", {
        settings: next,
      });
    } catch (reason) {
      onekoSettings = previous;
      reportError(reason);
    } finally {
      savingOnekoSettings = false;
    }
  }

  function updateOnekoBoolean(
    key: "enabled" | "showBrowserPages",
    event: Event,
  ) {
    const checked = (event.currentTarget as HTMLInputElement).checked;
    void saveOnekoSettings({ [key]: checked });
  }

  function updateOnekoChoice(key: "speed" | "size", event: Event) {
    const value = (event.currentTarget as HTMLSelectElement).value;
    if (key === "speed") {
      if (value === "slow" || value === "normal" || value === "fast") {
        void saveOnekoSettings({ speed: value });
      }
    } else if (value === "1" || value === "2") {
      void saveOnekoSettings({ size: value === "1" ? 1 : 2 });
    }
  }

  async function saveUpdaterSettings(changes: Partial<UpdaterSettings>) {
    if (savingUpdaterSettings || updaterSettingsLoading) return;
    const previous = updaterSettings;
    const next = { ...previous, ...changes };
    updaterSettings = next;
    savingUpdaterSettings = true;
    try {
      updaterSettings = await invoke<UpdaterSettings>("updater_settings_set", {
        settings: next,
      });
    } catch (reason) {
      updaterSettings = previous;
      reportError(reason);
    } finally {
      savingUpdaterSettings = false;
    }
  }

  async function checkForUpdates() {
    checkingForUpdates = true;
    updaterStatus = "Checking for updates…";
    try {
      const result = await invoke<UpdateCheck>("updater_check");
      if (!result.updateAvailable || !result.latestVersion) {
        updaterStatus = `You're up to date (v${result.currentVersion}).`;
        return;
      }
      const accepted = await ask(
        `Version ${result.latestVersion} is available. Download and install it now?`,
        { title: "Eefi Browser update", kind: "info" },
      );
      if (!accepted) {
        updaterStatus = `Version ${result.latestVersion} is available.`;
        return;
      }
      updaterStatus = `Downloading v${result.latestVersion}… The browser will restart to finish installing.`;
      await invoke("updater_install", { version: result.latestVersion });
    } catch (reason) {
      updaterStatus = "The update check failed.";
      reportError(reason);
    } finally {
      checkingForUpdates = false;
    }
  }

  function updateUpdaterBoolean(
    key: "automaticUpdates" | "checkOnStartup",
    event: Event,
  ) {
    const checked = (event.currentTarget as HTMLInputElement).checked;
    void saveUpdaterSettings({ [key]: checked });
  }

  function updateUpdaterChannel(event: Event) {
    const channel = (event.currentTarget as HTMLSelectElement).value;
    if (channel === "stable" || channel === "beta") {
      void saveUpdaterSettings({ channel });
    }
  }

  async function updateTabSleepTimeout(event: Event) {
    const previous = tabSleepTimeout;
    const seconds = Number((event.currentTarget as HTMLSelectElement).value);
    savingTabSleepTimeout = true;
    try {
      tabSleepTimeout = await invoke<number>("tab_sleep_timeout_set", { seconds });
    } catch (reason) {
      tabSleepTimeout = previous;
      reportError(reason);
    } finally {
      savingTabSleepTimeout = false;
    }
  }

  async function loadPrivacy() {
    const request = ++privacyLoadRequest;
    let origin = "";
    try {
      const current = new URL(currentUrl);
      if (current.protocol === "http:" || current.protocol === "https:") {
        origin = current.origin;
      }
    } catch {
      origin = "";
    }
    privacyLoading = true;
    permissionOrigin = origin;
    sitePermissions = {};
    try {
      const status = await invoke<PrivacyStatus>("privacy_status");
      if (request !== privacyLoadRequest) return;
      httpsOnly = status.httpsOnly;
      blockThirdPartyCookies = status.blockThirdPartyCookies;
      clearCookiesOnExit = status.clearCookiesOnExit;
      const filters = await invoke<CustomFilterConfig>("adblock_get_custom_filters");
      if (request !== privacyLoadRequest) return;
      customFilterListUrls = filters.listUrls.join("\n");
      customUserRules = filters.userRules;
      if (origin) {
        const permissions = await invoke<SitePermissionStatus>("privacy_site_permissions", {
          origin,
        });
        if (request !== privacyLoadRequest) return;
        sitePermissions = permissions.permissions;
      }
    } catch (reason) {
      if (request === privacyLoadRequest) reportError(reason);
    } finally {
      if (request === privacyLoadRequest) privacyLoading = false;
    }
  }

  async function updateSitePermission(permission: string, event: Event) {
    if (savingSitePermissions[permission]) return;
    const allowed = (event.currentTarget as HTMLInputElement).checked;
    const previous = sitePermissions[permission] ?? false;
    const origin = permissionOrigin;
    savingSitePermissions = { ...savingSitePermissions, [permission]: true };
    sitePermissions = { ...sitePermissions, [permission]: allowed };
    try {
      await invoke("privacy_set_site_permission", {
        origin,
        permission,
        allowed,
      });
    } catch (reason) {
      if (permissionOrigin === origin) {
        sitePermissions = { ...sitePermissions, [permission]: previous };
      }
      reportError(reason);
    } finally {
      const { [permission]: _saving, ...remaining } = savingSitePermissions;
      savingSitePermissions = remaining;
    }
  }

  async function loadView() {
    if (view === "bookmarks") await loadBookmarks();
    if (view === "history") await loadHistory();
    if (view === "downloads") await loadDownloads();
    if (view === "privacy") await loadPrivacy();
    if (view === "settings") {
      await Promise.all([loadTabSleepSettings(), loadUpdaterSettings(), loadOnekoSettings()]);
    }
    if (view === "extensions") await loadUserScripts();
  }

  function resetUserScriptEditor() {
    scriptEditId = null;
    scriptName = "";
    scriptMatchPattern = "";
    scriptSource = "";
    scriptEnabled = false;
  }

  function editUserScript(script: UserScript) {
    scriptEditId = script.id;
    scriptName = script.name;
    scriptMatchPattern = script.matchPattern;
    scriptSource = script.source;
    scriptEnabled = script.enabled;
  }

  async function saveUserScript() {
    scriptSaving = true;
    error = "";
    try {
      await invoke<number>("user_scripts_save", {
        id: scriptEditId,
        name: scriptName,
        matchPattern: scriptMatchPattern,
        source: scriptSource,
        enabled: scriptEnabled,
      });
      resetUserScriptEditor();
      await loadUserScripts();
    } catch (reason) {
      reportError(reason);
    } finally {
      scriptSaving = false;
    }
  }

  async function updateUserScriptEnabled(script: UserScript, event: Event) {
    const enabled = (event.currentTarget as HTMLInputElement).checked;
    try {
      await invoke("user_scripts_set_enabled", { id: script.id, enabled });
      await loadUserScripts();
    } catch (reason) {
      await loadUserScripts();
      reportError(reason);
    }
  }

  async function deleteUserScript(id: number) {
    try {
      await invoke("user_scripts_delete", { id });
      if (scriptEditId === id) resetUserScriptEditor();
      await loadUserScripts();
    } catch (reason) {
      reportError(reason);
    }
  }

  function resetTabGroupEditor() {
    tabGroupEditId = null;
    tabGroupName = "";
    tabGroupColor = "#4f7ff0";
  }

  function editTabGroup(group: TabGroup) {
    tabGroupEditId = group.id;
    tabGroupName = group.name;
    tabGroupColor = group.color;
  }

  async function saveTabGroup() {
    error = "";
    try {
      if (tabGroupEditId) {
        await invoke("tab_group_update", {
          groupId: tabGroupEditId,
          name: tabGroupName,
          color: tabGroupColor,
        });
      } else {
        await invoke("tab_group_create", {
          name: tabGroupName,
          color: tabGroupColor,
          tabIds: activeTabId ? [activeTabId] : [],
        });
      }
      resetTabGroupEditor();
      onGroupsChange();
    } catch (reason) {
      reportError(reason);
    }
  }

  async function assignTabToGroup(group: TabGroup, tab: BrowserTab, event: Event) {
    const checked = (event.currentTarget as HTMLInputElement).checked;
    try {
      await invoke("tab_group_assign", {
        tabId: tab.id,
        groupId: checked ? group.id : null,
      });
      onGroupsChange();
    } catch (reason) {
      reportError(reason);
    }
  }

  async function deleteTabGroup(groupId: string) {
    try {
      await invoke("tab_group_delete", { groupId });
      if (tabGroupEditId === groupId) resetTabGroupEditor();
      onGroupsChange();
    } catch (reason) {
      reportError(reason);
    }
  }

  async function updateHttpsOnly(event: Event) {
    const enabled = (event.currentTarget as HTMLInputElement).checked;
    privacyLoading = true;
    try {
      await invoke("privacy_set_https_only", { enabled });
      httpsOnly = enabled;
    } catch (reason) {
      httpsOnly = !enabled;
      reportError(reason);
    } finally {
      privacyLoading = false;
    }
  }

  async function updateThirdPartyCookies(event: Event) {
    const enabled = (event.currentTarget as HTMLInputElement).checked;
    privacyLoading = true;
    try {
      await invoke("privacy_set_block_third_party_cookies", { enabled });
      blockThirdPartyCookies = enabled;
    } catch (reason) {
      blockThirdPartyCookies = !enabled;
      reportError(reason);
    } finally {
      privacyLoading = false;
    }
  }

  async function updateClearCookiesOnExit(event: Event) {
    const enabled = (event.currentTarget as HTMLInputElement).checked;
    privacyLoading = true;
    try {
      await invoke("privacy_set_clear_cookies_on_exit", { enabled });
      clearCookiesOnExit = enabled;
    } catch (reason) {
      clearCookiesOnExit = !enabled;
      reportError(reason);
    } finally {
      privacyLoading = false;
    }
  }

  async function saveCustomFilters() {
    customFiltersSaving = true;
    customFiltersMessage = "Saving and updating filter lists…";
    const listUrls = customFilterListUrls
      .split(/\r?\n/)
      .map((url) => url.trim())
      .filter(Boolean);
    try {
      await invoke("adblock_set_custom_filters", {
        listUrls,
        userRules: customUserRules,
      });
      customFiltersMessage = "Custom filters saved and applied.";
    } catch (reason) {
      reportError(reason);
    } finally {
      customFiltersSaving = false;
    }
  }

  $effect(() => {
    void view;
    void query;
    void folderId;
    if (view === "privacy") void currentUrl;
    void loadView();
  });

  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    void listen("browser:downloads-updated", () => {
      if (view === "downloads") void loadDownloads();
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    void listen<DownloadProgress>("browser:download-progress", ({ payload }) => {
      if (view === "downloads") {
        downloadProgress = { ...downloadProgress, [payload.path]: payload };
      }
    }).then((stop) => {
      if (disposed) stop();
      else unlistenProgress = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
      unlistenProgress?.();
    };
  });

  function reportError(reason: unknown) {
    error = reason instanceof Error ? reason.message : String(reason);
  }

  function resetEditor() {
    editId = null;
    title = "";
    url = "";
    folderId = "";
    onBar = false;
  }

  function editBookmark(bookmark: Bookmark) {
    editId = bookmark.id;
    title = bookmark.title;
    url = bookmark.url;
    folderId = bookmark.folderId?.toString() ?? "";
    onBar = bookmark.onBar;
  }

  async function saveBookmark() {
    try {
      const selectedFolder = folderId ? Number(folderId) : null;
      if (editId === null) {
        await invoke("bookmark_create", {
          title,
          url,
          folderId: selectedFolder,
          onBar,
        });
      } else {
        await invoke("bookmark_update", {
          id: editId,
          title,
          url,
          folderId: selectedFolder,
          onBar,
        });
      }
      resetEditor();
      await loadBookmarks();
    } catch (reason) {
      reportError(reason);
    }
  }

  async function addCurrentPage() {
    if (!isHttpUrl(currentUrl)) return;
    try {
      await invoke("bookmark_create", {
        title: currentTitle || currentUrl,
        url: currentUrl,
        folderId: null,
        onBar: true,
      });
      await loadBookmarks();
    } catch (reason) {
      reportError(reason);
    }
  }

  async function removeBookmark(id: number) {
    try {
      await invoke("bookmark_delete", { id });
      await loadBookmarks();
    } catch (reason) {
      reportError(reason);
    }
  }

  async function addFolder() {
    try {
      await invoke("bookmark_folder_create", { name: newFolderName, parentId: null });
      newFolderName = "";
      await loadBookmarks();
    } catch (reason) {
      reportError(reason);
    }
  }

  async function removeFolder(id: number) {
    try {
      await invoke("bookmark_folder_delete", { id });
      if (folderId === String(id)) folderId = "";
      await loadBookmarks();
    } catch (reason) {
      reportError(reason);
    }
  }

  function escapeHtml(value: string) {
    return value.replace(/[&<>"']/g, (character) => ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;",
    })[character]!);
  }

  async function exportBookmarks() {
    try {
      const all = await invoke<Bookmark[]>("bookmarks_list", {
        query: null,
        barOnly: false,
        folderId: null,
      });
      const anchors = all.map((bookmark) =>
        `<DT><A HREF="${escapeHtml(bookmark.url)}" ADD_DATE="${bookmark.createdAt}">${escapeHtml(bookmark.title)}</A>`,
      );
      const document = [
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>",
        '<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">',
        "<TITLE>Bookmarks</TITLE>",
        "<H1>Bookmarks</H1>",
        "<DL><p>",
        ...anchors,
        "</DL><p>",
      ].join("\n");
      const blob = new Blob([document], { type: "text/html;charset=utf-8" });
      const objectUrl = URL.createObjectURL(blob);
      const link = documentElementLink(objectUrl);
      link.download = "eefi-bookmarks.html";
      link.click();
      window.setTimeout(() => URL.revokeObjectURL(objectUrl), 1000);
    } catch (reason) {
      reportError(reason);
    }
  }

  function documentElementLink(href: string) {
    const link = window.document.createElement("a");
    link.href = href;
    link.style.display = "none";
    window.document.body.append(link);
    window.setTimeout(() => link.remove(), 0);
    return link;
  }

  async function importBookmarks(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    importing = true;
    error = "";
    try {
      if (file.size > 5 * 1024 * 1024) {
        throw new Error("Bookmark files must be smaller than 5 MB.");
      }
      const html = await file.text();
      const document = new DOMParser().parseFromString(html, "text/html");
      const anchors = Array.from(document.querySelectorAll<HTMLAnchorElement>("a[href]"));
      if (anchors.length > 5_000) {
        throw new Error("The import contains too many bookmarks (maximum 5,000).");
      }
      const entries = anchors
        .map((anchor) => ({
          title: anchor.textContent?.trim() || anchor.href,
          url: anchor.getAttribute("href") || "",
        }))
        .filter((entry) => isHttpUrl(entry.url));
      const imported = await invoke<number>("bookmark_import", { bookmarks: entries });
      error = imported ? `Imported ${imported} bookmarks.` : "No HTTP(S) bookmarks found.";
      await loadBookmarks();
    } catch (reason) {
      reportError(reason);
    } finally {
      importing = false;
      input.value = "";
    }
  }

  async function clearSelectedBrowsingData() {
    if (!clearHistory && !clearCookies && !clearCache) {
      reportError("Select at least one type of browsing data to clear.");
      return;
    }
    clearingData = true;
    try {
      if (clearCookies || clearCache) {
        await invoke("privacy_clear_cookies_and_cache", {
          clearCookies,
          clearCache,
        });
      }
      if (clearHistory) {
        if (historyRange === "all") {
          await invoke("history_clear");
        } else {
          const days = Number(historyRange);
          const cutoff = Math.floor(Date.now() / 1000) - days * 24 * 60 * 60;
          await invoke("history_clear_before", { timestamp: cutoff });
        }
      }
      showClearDialog = false;
      if (clearHistory) await loadHistory();
    } catch (reason) {
      reportError(reason);
    } finally {
      clearingData = false;
    }
  }

  async function deleteSite(urlValue: string) {
    try {
      const host = new URL(urlValue).hostname;
      await invoke("history_delete_site", { site: host });
      await loadHistory();
    } catch (reason) {
      reportError(reason);
    }
  }

  async function updateDownloadDirectory() {
    try {
      downloadDirectory = await invoke<string>("set_download_directory", {
        path: downloadDirectory,
      });
    } catch (reason) {
      reportError(reason);
    }
  }

  async function updateAskWhereToSave(event: Event) {
    const enabled = (event.currentTarget as HTMLInputElement).checked;
    const previous = askWhereToSave;
    askWhereToSave = enabled;
    try {
      await invoke("set_download_ask_where_to_save", { enabled });
    } catch (reason) {
      askWhereToSave = previous;
      reportError(reason);
    }
  }

  async function controlDownload(download: Download, action: "pause" | "resume" | "cancel") {
    if (downloadControlPending[download.id]) return;
    downloadControlPending = { ...downloadControlPending, [download.id]: true };
    try {
      await invoke("download_control", { id: download.id, action });
      if (action === "pause" || action === "resume") {
        const progress = downloadProgress[download.path];
        if (progress) {
          downloadProgress = {
            ...downloadProgress,
            [download.path]: { ...progress, paused: action === "pause" },
          };
        }
      }
    } catch (reason) {
      reportError(reason);
    } finally {
      const { [download.id]: _pending, ...remaining } = downloadControlPending;
      downloadControlPending = remaining;
    }
  }

  function fileExtension(path: string) {
    const filename = path.split(/[\\/]/).pop() ?? "";
    return filename.split(".").pop()?.toLowerCase() ?? "";
  }

  function formatBytes(bytes: number) {
    if (bytes < 1024) return `${bytes} B`;
    const units = ["KB", "MB", "GB", "TB"];
    let value = bytes / 1024;
    let unitIndex = 0;
    while (value >= 1024 && unitIndex < units.length - 1) {
      value /= 1024;
      unitIndex += 1;
    }
    return `${value.toFixed(1)} ${units[unitIndex]}`;
  }

  async function openDownload(download: Download) {
    if (dangerousExtensions.has(fileExtension(download.path))) {
      if (confirmDangerousId !== download.id) {
        confirmDangerousId = download.id;
        return;
      }
    }
    try {
      await invoke("download_open", { id: download.id });
      confirmDangerousId = null;
    } catch (reason) {
      reportError(reason);
    }
  }

  async function revealDownload(download: Download) {
    try {
      await invoke("download_reveal", { id: download.id });
    } catch (reason) {
      reportError(reason);
    }
  }

  function dateLabel(seconds: number) {
    return new Date(seconds * 1000).toLocaleString();
  }
</script>

<section class="library" aria-label={`${view} manager`}>
  <header class="library-header">
    <div>
      <div class="eyebrow">YOUR BROWSER</div>
      <h1>
        {view === "bookmarks"
          ? "Bookmarks"
          : view === "history"
            ? "History"
            : view === "downloads"
              ? "Downloads"
              : view === "extensions"
                  ? "User scripts"
                  : view === "settings"
                    ? "Settings"
                    : view === "tab-groups"
                      ? "Tab groups"
                      : "Privacy settings"}
      </h1>
    </div>
    <button class="library-close" aria-label="Close manager" onclick={onClose}>×</button>
  </header>

  {#if error}
    <div class="library-message" role="status">{error}</div>
  {/if}

  {#if view === "bookmarks"}
    <div class="library-toolbar">
      <input class="library-search" bind:value={query} placeholder="Search bookmarks" aria-label="Search bookmarks" />
      {#if isHttpUrl(currentUrl)}
        <button class="primary-button" onclick={addCurrentPage}>
          {currentPageAlreadyBookmarked ? "Save current page again" : "Add current page"}
        </button>
      {/if}
      <button onclick={exportBookmarks}>Export HTML</button>
      <label class="file-button">
        {importing ? "Importing…" : "Import HTML"}
        <input type="file" accept=".html,.htm,text/html" disabled={importing} onchange={importBookmarks} />
      </label>
    </div>

    <div class="library-columns">
      <aside class="folder-list">
        <button class:chosen={!folderId} onclick={() => (folderId = "")}>All bookmarks</button>
        {#each folders as folder (folder.id)}
          <div class="folder-item">
            <button class:chosen={folderId === String(folder.id)} onclick={() => (folderId = String(folder.id))}>
              {folder.name}
            </button>
            <button class="quiet-button" aria-label={`Delete ${folder.name} folder`} onclick={() => void removeFolder(folder.id)}>×</button>
          </div>
        {/each}
        <form class="new-folder" onsubmit={(event) => { event.preventDefault(); void addFolder(); }}>
          <input bind:value={newFolderName} placeholder="New folder" aria-label="New folder name" maxlength="120" />
          <button aria-label="Create folder">+</button>
        </form>
      </aside>

      <div class="library-main">
        <form class="bookmark-editor" onsubmit={(event) => { event.preventDefault(); void saveBookmark(); }}>
          <div class="editor-heading">{editId === null ? "Add a bookmark" : "Edit bookmark"}</div>
          <div class="editor-fields">
            <input bind:value={title} placeholder="Title" aria-label="Bookmark title" maxlength="512" required />
            <input bind:value={url} placeholder="https://example.com" aria-label="Bookmark URL" maxlength="4096" required />
            <select bind:value={folderId} aria-label="Bookmark folder">
              <option value="">No folder</option>
              {#each folders as folder (folder.id)}<option value={String(folder.id)}>{folder.name}</option>{/each}
            </select>
            <label class="bar-toggle"><input type="checkbox" bind:checked={onBar} /> Bookmarks bar</label>
            <button class="primary-button" type="submit">{editId === null ? "Save" : "Update"}</button>
            {#if editId !== null}<button type="button" onclick={resetEditor}>Cancel</button>{/if}
          </div>
        </form>

        <div class="result-list">
          {#each bookmarks as bookmark (bookmark.id)}
            <article class="result-row">
              <div class="result-copy">
                <strong>{bookmark.title}</strong>
                <span>{bookmark.url}</span>
                {#if bookmark.folderName}<small>{bookmark.folderName}</small>{/if}
              </div>
              <button onclick={() => onNavigate(bookmark.url)}>Open</button>
              <button onclick={() => editBookmark(bookmark)}>Edit</button>
              <button class="danger-button" onclick={() => void removeBookmark(bookmark.id)}>Delete</button>
            </article>
          {:else}
            <p class="empty-state">No bookmarks match this view.</p>
          {/each}
        </div>
      </div>
    </div>
  {:else if view === "history"}
    <div class="library-toolbar">
      <input class="library-search" bind:value={query} placeholder="Search visited pages" aria-label="Search history" />
      <button onclick={() => (showClearDialog = true)}>Clear browsing data</button>
    </div>
    <div class="result-list">
      {#each history as entry (entry.id)}
        <article class="result-row">
          <div class="result-copy">
            <strong>{entry.title || entry.url}</strong>
            <span>{entry.url}</span>
            <small>{dateLabel(entry.visitedAt)}</small>
          </div>
          <button onclick={() => onNavigate(entry.url)}>Open</button>
          <button class="danger-button" onclick={() => void deleteSite(entry.url)}>Delete site</button>
        </article>
      {:else}
        <p class="empty-state">No history entries match this view.</p>
      {/each}
    </div>
    {#if showClearDialog}
      <div class="dialog-backdrop" role="presentation">
        <dialog open class="clear-dialog" aria-modal="true" aria-labelledby="clear-title">
          <h2 id="clear-title">Clear browsing data</h2>
          <p>Choose which browser data to remove. Cookies and cached files are cleared for all time.</p>
          <label class="clear-data-option">
            <input type="checkbox" bind:checked={clearHistory} />
            <span>Browsing history</span>
          </label>
          <label for="history-range">Time range</label>
          <select id="history-range" bind:value={historyRange} disabled={!clearHistory}>
            <option value="all">All time</option>
            <option value="1">Last 24 hours</option>
            <option value="7">Last 7 days</option>
            <option value="30">Last 30 days</option>
          </select>
          <label class="clear-data-option">
            <input type="checkbox" bind:checked={clearCookies} />
            <span>Cookies</span>
          </label>
          <label class="clear-data-option">
            <input type="checkbox" bind:checked={clearCache} />
            <span>Cached files and images</span>
          </label>
          <div class="dialog-actions">
            <button onclick={() => (showClearDialog = false)}>Cancel</button>
            <button
              class="danger-button"
              disabled={clearingData || (!clearHistory && !clearCookies && !clearCache)}
              onclick={() => void clearSelectedBrowsingData()}
            >{clearingData ? "Clearing…" : "Clear data"}</button>
          </div>
        </dialog>
      </div>
    {/if}
  {:else if view === "downloads"}
    <div class="library-toolbar download-location">
      <label for="download-directory">Save files to</label>
      <input id="download-directory" bind:value={downloadDirectory} aria-label="Download folder" />
      <button class="primary-button" onclick={updateDownloadDirectory}>Save location</button>
      <label class="download-ask-setting">
        <input type="checkbox" checked={askWhereToSave} onchange={updateAskWhereToSave} />
        Ask where to save each file
      </label>
      <button class="danger-button" onclick={() => void invoke("downloads_clear").then(loadDownloads).catch(reportError)}>
        Clear list
      </button>
    </div>
    <div class="result-list">
      {#each downloads as download (download.id)}
        {@const filename = download.path.split(/[\\/]/).pop() ?? ""}
        {@const extension = fileExtension(download.path)}
        {@const dangerous = dangerousExtensions.has(extension)}
        {@const progress = downloadProgress[download.path]}
        <article class="result-row download-row">
          <div class="result-copy">
            <strong>{filename}</strong>
            <span>{download.url}</span>
            <small>
              {download.status === "downloading" && progress?.paused
                ? "Paused"
                : download.status}
              · {dateLabel(download.startedAt)}
              {#if download.status === "downloading" && progress}
                · {formatBytes(progress.bytesReceived)}
                {#if progress.totalBytes !== null}
                  of {formatBytes(progress.totalBytes)}
                {/if}
              {/if}
            </small>
            {#if download.status === "downloading"}
                <progress
                  class="download-progress"
                  max={progress?.totalBytes ?? undefined}
                  value={progress?.totalBytes ? progress.bytesReceived : undefined}
                  aria-label={`Download progress for ${filename}`}
                ></progress>
            {/if}
            {#if dangerous}<small class="safety-warning">This file type can run code. Only open it if you trust its source.</small>{/if}
            {#if confirmDangerousId === download.id}<small class="safety-warning">Click “Open anyway” to confirm.</small>{/if}
          </div>
          {#if download.status === "downloading"}
            {#if progress?.paused}
              <button
                disabled={downloadControlPending[download.id]}
                onclick={() => void controlDownload(download, "resume")}
              >Resume</button>
            {:else}
              <button
                disabled={downloadControlPending[download.id]}
                onclick={() => void controlDownload(download, "pause")}
              >Pause</button>
            {/if}
            <button
              class="danger-button"
              disabled={downloadControlPending[download.id]}
              onclick={() => void controlDownload(download, "cancel")}
            >Cancel</button>
          {/if}
          {#if download.status === "complete"}
            <button class:danger-button={dangerous} onclick={() => void openDownload(download)}>
              {dangerous && confirmDangerousId === download.id ? "Open anyway" : "Open"}
            </button>
            <button onclick={() => void revealDownload(download)}>Show in folder</button>
          {/if}
        </article>
      {:else}
        <p class="empty-state">No downloads yet.</p>
      {/each}
    </div>
  {:else if view === "extensions"}
    <div class="user-script-warning" role="note">
      User scripts execute as code in matching pages and can read or change page content. Add only scripts you trust.
      They run in the top-level page after it loads, do not run in private windows, and take effect after a page reload.
    </div>
    <div class="library-toolbar">
      <button
        class="primary-button"
        disabled={isPrivate || !isHttpUrl(currentUrl)}
        onclick={onReload}
      >Reload page to apply</button>
    </div>
    <form class="user-script-editor" onsubmit={(event) => { event.preventDefault(); void saveUserScript(); }}>
      <div class="editor-heading">{scriptEditId === null ? "Add a user script" : "Edit user script"}</div>
      <div class="user-script-fields">
        <label>
          <span>Name</span>
          <input bind:value={scriptName} maxlength="120" required />
        </label>
        <label>
          <span>Match pattern</span>
          <input
            bind:value={scriptMatchPattern}
            maxlength="1024"
            placeholder="https://example.com/*"
            required
          />
        </label>
        <label class="user-script-source">
          <span>JavaScript source</span>
          <textarea bind:value={scriptSource} maxlength="65536" rows="12" spellcheck="false" required></textarea>
        </label>
        <label class="user-script-enable">
          <input type="checkbox" bind:checked={scriptEnabled} />
          Enable after saving
        </label>
        <div class="user-script-actions">
          <button class="primary-button" type="submit" disabled={scriptSaving}>
            {scriptSaving ? "Saving…" : scriptEditId === null ? "Save script" : "Update script"}
          </button>
          {#if scriptEditId !== null}
            <button type="button" onclick={resetUserScriptEditor}>Cancel</button>
          {/if}
        </div>
      </div>
    </form>
    <div class="result-list user-script-list">
      {#each userScripts as script (script.id)}
        <article class="result-row">
          <div class="result-copy">
            <strong>{script.name}</strong>
            <span>{script.matchPattern}</span>
            <small>{script.enabled ? "Enabled" : "Disabled"} · Updated {dateLabel(script.updatedAt)}</small>
          </div>
          <label class="user-script-row-toggle">
            <input
              type="checkbox"
              checked={script.enabled}
              onchange={(event) => void updateUserScriptEnabled(script, event)}
            />
            Enabled
          </label>
          <button onclick={() => editUserScript(script)}>Edit</button>
          <button class="danger-button" onclick={() => void deleteUserScript(script.id)}>Delete</button>
        </article>
      {:else}
        <p class="empty-state">No user scripts yet. New scripts are disabled unless you explicitly enable them.</p>
      {/each}
    </div>
  {:else if view === "tab-groups"}
    <p class="tab-group-note">
      Groups are window-local in private windows and restored with normal tabs. Tabs stay in one strip; drag them to reorder.
    </p>
    <form class="tab-group-editor" onsubmit={(event) => { event.preventDefault(); void saveTabGroup(); }}>
      <label>
        <span>{tabGroupEditId === null ? "New group name" : "Group name"}</span>
        <input bind:value={tabGroupName} maxlength="64" required />
      </label>
      <label>
        <span>Color</span>
        <select bind:value={tabGroupColor}>
          <option value="#4f7ff0">Blue</option>
          <option value="#8d63d8">Purple</option>
          <option value="#159b9a">Teal</option>
          <option value="#2d9a65">Green</option>
          <option value="#d49a2b">Yellow</option>
          <option value="#e17a32">Orange</option>
          <option value="#d75059">Red</option>
          <option value="#788397">Gray</option>
        </select>
      </label>
      <div class="tab-group-editor-actions">
        <button class="primary-button" type="submit" disabled={!activeTabId}>
          {tabGroupEditId === null ? "Create for active tab" : "Save group"}
        </button>
        {#if tabGroupEditId !== null}
          <button type="button" onclick={resetTabGroupEditor}>Cancel</button>
        {/if}
      </div>
    </form>
    <div class="tab-group-list">
      {#each tabGroups as group (group.id)}
        <section class="tab-group-card">
          <header>
            <span class="tab-group-swatch" style={`background-color: ${group.color}`}></span>
            <strong>{group.name}</strong>
            <button onclick={() => editTabGroup(group)}>Edit</button>
            <button class="danger-button" onclick={() => void deleteTabGroup(group.id)}>Delete</button>
          </header>
          <div class="tab-group-members" aria-label={`Tabs in ${group.name}`}>
            {#each tabs as tab (tab.id)}
              {@const currentGroup = tabGroups.find((entry) => entry.id === tab.groupId)}
              <label>
                <input
                  type="checkbox"
                  checked={tab.groupId === group.id}
                  onchange={(event) => void assignTabToGroup(group, tab, event)}
                />
                <span>{tab.title || tab.url || "New tab"}</span>
                {#if currentGroup && currentGroup.id !== group.id}
                  <small>in {currentGroup.name}</small>
                {/if}
              </label>
            {:else}
              <p class="empty-state">Open a tab to add it to this group.</p>
            {/each}
          </div>
        </section>
      {:else}
        <p class="empty-state">No tab groups yet. Create one for the active tab.</p>
      {/each}
    </div>
  {:else if view === "settings"}
    <section class="privacy-settings" aria-label="Browser settings">
      <article class="privacy-card oneko-settings-card">
        <h2>Oneko cat</h2>
        <label class="privacy-toggle">
          <input
            type="checkbox"
            checked={onekoSettings.enabled}
            disabled={onekoSettingsLoading || savingOnekoSettings}
            onchange={(event) => updateOnekoBoolean("enabled", event)}
          />
          <span>
            <strong>Enable the cat</strong>
            <small>Follow your pointer on web pages. Toggle instantly with Ctrl+Shift+K.</small>
          </span>
        </label>
        <label for="oneko-speed"><strong>Speed</strong></label>
        <select
          id="oneko-speed"
          value={onekoSettings.speed}
          disabled={onekoSettingsLoading || savingOnekoSettings}
          onchange={(event) => updateOnekoChoice("speed", event)}
        >
          <option value="slow">Slow</option>
          <option value="normal">Normal</option>
          <option value="fast">Fast</option>
        </select>
        <label for="oneko-size"><strong>Size</strong></label>
        <select
          id="oneko-size"
          value={onekoSettings.size}
          disabled={onekoSettingsLoading || savingOnekoSettings}
          onchange={(event) => updateOnekoChoice("size", event)}
        >
          <option value={1}>1x</option>
          <option value={2}>2x</option>
        </select>
        <label class="privacy-toggle">
          <input
            type="checkbox"
            checked={onekoSettings.showBrowserPages}
            disabled={onekoSettingsLoading || savingOnekoSettings}
            onchange={(event) => updateOnekoBoolean("showBrowserPages", event)}
          />
          <span>
            <strong>Show cat on browser pages</strong>
            <small>Also show it on blank/new-tab pages; browser controls are never covered.</small>
          </span>
        </label>
        <p>Reduced-motion preferences display a still sitting cat instead of animations.</p>
      </article>
      <article class="privacy-card performance-card">
        <h2>Performance</h2>
        <label for="tab-sleep-timeout"><strong>Discard inactive tabs after</strong></label>
        <select
          id="tab-sleep-timeout"
          value={tabSleepTimeout}
          disabled={tabSleepSettingsLoading || savingTabSleepTimeout}
          onchange={(event) => void updateTabSleepTimeout(event)}
        >
          <option value={0}>Never</option>
          <option value={60}>1 minute</option>
          <option value={300}>5 minutes</option>
          <option value={900}>15 minutes</option>
          <option value={1800}>30 minutes</option>
          <option value={3600}>1 hour</option>
        </select>
        <p>
          Inactive tabs are unloaded to reduce memory use. Selecting one reloads its current page; unsaved page state
          and that tab's back/forward history are not preserved.
        </p>
      </article>
      <article class="privacy-card updater-card">
        <h2>Updates</h2>
        <p>Current version: <strong>v{appVersion || "…"}</strong></p>
        <label for="update-channel"><strong>Build channel</strong></label>
        <select
          id="update-channel"
          value={updaterSettings.channel}
          disabled={updaterSettingsLoading || savingUpdaterSettings}
          onchange={updateUpdaterChannel}
        >
          <option value="stable">Stable releases</option>
          <option value="beta">Beta (pre-releases and stable releases)</option>
        </select>
        <label class="privacy-toggle">
          <input
            type="checkbox"
            checked={updaterSettings.automaticUpdates}
            disabled={updaterSettingsLoading || savingUpdaterSettings}
            onchange={(event) => updateUpdaterBoolean("automaticUpdates", event)}
          />
          <span>
            <strong>Install updates automatically</strong>
            <small>When a startup check finds an update, install it and restart without asking.</small>
          </span>
        </label>
        <label class="privacy-toggle">
          <input
            type="checkbox"
            checked={updaterSettings.checkOnStartup}
            disabled={updaterSettingsLoading || savingUpdaterSettings}
            onchange={(event) => updateUpdaterBoolean("checkOnStartup", event)}
          />
          <span>
            <strong>Check for updates at startup</strong>
            <small>When automatic installation is off, ask before downloading an available update.</small>
          </span>
        </label>
        <div class="updater-actions">
          <button
            class="primary-button"
            disabled={updaterSettingsLoading || checkingForUpdates}
            onclick={() => void checkForUpdates()}
          >{checkingForUpdates ? "Checking…" : "Check for updates"}</button>
          {#if updaterStatus}<small role="status">{updaterStatus}</small>{/if}
        </div>
        <p>
          Updates are downloaded from Eefi Browser GitHub releases. The browser restarts to replace its executable;
          keep the portable folder writable.
        </p>
      </article>
    </section>
  {:else}
    <section class="privacy-settings" aria-label="Privacy settings">
      <article class="privacy-card">
        <label class="privacy-toggle">
          <input type="checkbox" checked={httpsOnly} disabled={privacyLoading} onchange={updateHttpsOnly} />
          <span>
            <strong>HTTPS-only mode</strong>
            <small>Upgrade intercepted HTTP requests to HTTPS.</small>
          </span>
        </label>
        <p>
          When enabled, insecure requests are upgraded and are not retried over HTTP. Sites that do not support HTTPS
          may fail to load; turn this setting off to visit them.
        </p>
      </article>
      <article class="privacy-card">
        <label class="privacy-toggle">
          <input
            type="checkbox"
            checked={blockThirdPartyCookies}
            disabled={privacyLoading}
            onchange={updateThirdPartyCookies}
          />
          <span>
            <strong>Block third-party request cookies</strong>
            <small>Remove cookies from cross-site requests.</small>
          </span>
        </label>
        <p>
          This prevents existing cookies from being sent to other sites in cross-site requests. WebView2 may still store
          cookies received from those sites; use Clear browsing data to remove stored cookies.
        </p>
      </article>
      <article class="privacy-card">
        <label class="privacy-toggle">
          <input
            type="checkbox"
            checked={clearCookiesOnExit}
            disabled={privacyLoading}
            onchange={updateClearCookiesOnExit}
          />
          <span>
            <strong>Clear cookies and cache on exit</strong>
            <small>Remove WebView2 cookies and disk cache when the browser closes.</small>
          </span>
        </label>
      </article>
      <article class="privacy-card custom-filters-card">
        <h2>Custom ad-block filters</h2>
        <p>Use HTTPS filter-list URLs and standard Adblock Plus rules. Up to 8 lists and 64 KB of custom rules are supported.</p>
        <label>
          <strong>Filter-list URLs</strong>
          <textarea
            bind:value={customFilterListUrls}
            rows="4"
            disabled={customFiltersSaving}
            placeholder="https://filters.example/list.txt"
            aria-label="Custom filter-list URLs"
          ></textarea>
        </label>
        <label>
          <strong>Custom rules</strong>
          <textarea
            bind:value={customUserRules}
            rows="6"
            disabled={customFiltersSaving}
            placeholder="||ads.example^"
            aria-label="Custom ad-block rules"
          ></textarea>
        </label>
        <button class="primary-button" disabled={customFiltersSaving} onclick={() => void saveCustomFilters()}>
          {customFiltersSaving ? "Saving…" : "Save custom filters"}
        </button>
        {#if customFiltersMessage}<p role="status">{customFiltersMessage}</p>{/if}
      </article>
      <article class="privacy-card site-permissions-card">
        <h2>Site permissions</h2>
        <p>
          {permissionOrigin
            ? `Permissions for ${permissionOrigin}. Requests are denied unless you allow them here.`
            : "Open an HTTP(S) site to manage its permissions."}
        </p>
        {#each permissionOptions as permission (permission.key)}
          <label class="site-permission-option">
            <input
              type="checkbox"
              checked={sitePermissions[permission.key] ?? false}
              disabled={!permissionOrigin || privacyLoading || savingSitePermissions[permission.key]}
              onchange={(event) => void updateSitePermission(permission.key, event)}
            />
            <span>{permission.label}</span>
          </label>
        {/each}
      </article>
    </section>
  {/if}
</section>

<style>
  .library {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    padding: 21px clamp(18px, 4vw, 56px) 25px;
    overflow: auto;
    color: #242937;
    background: #f5f6fa;
    font: 13px "Segoe UI", sans-serif;
  }
  button, input, select { font: inherit; }
  button, .file-button {
    min-height: 34px;
    padding: 0 12px;
    color: #3b4250;
    background: #fff;
    border: 1px solid #dce0e8;
    border-radius: 8px;
    cursor: pointer;
  }
  button:hover, .file-button:hover { background: #f3f5fa; }
  input, select {
    min-width: 0;
    height: 36px;
    padding: 0 10px;
    color: #242937;
    background: #fff;
    border: 1px solid #dce0e8;
    border-radius: 8px;
  }
  .library-header { display: flex; align-items: flex-start; justify-content: space-between; margin-bottom: 20px; }
  .eyebrow { color: #7b8496; font-size: 10px; font-weight: 700; letter-spacing: .12em; }
  h1 { margin: 4px 0 0; font-size: 25px; font-weight: 650; }
  .library-close { width: 36px; padding: 0; font-size: 22px; }
  .library-toolbar { display: flex; align-items: center; gap: 8px; margin-bottom: 14px; }
  .library-search { flex: 1; }
  .primary-button { color: #fff; background: #536fce; border-color: #536fce; }
  .primary-button:hover { background: #435db8; }
  .danger-button { color: #a13542; }
  .file-button { display: inline-flex; align-items: center; position: relative; }
  .file-button input { position: absolute; inset: 0; width: 100%; height: 100%; padding: 0; opacity: 0; cursor: pointer; }
  .library-columns { display: grid; grid-template-columns: minmax(160px, 220px) minmax(0, 1fr); gap: 15px; min-height: 0; flex: 1; }
  .folder-list, .bookmark-editor, .result-list { background: #fff; border: 1px solid #e0e3e9; border-radius: 11px; }
  .folder-list { align-self: start; padding: 9px; }
  .folder-list > button, .folder-item > button:first-child { width: 100%; text-align: left; border-color: transparent; background: transparent; }
  .folder-list button.chosen { color: #435db8; background: #edf1ff; }
  .folder-item { display: flex; align-items: center; }
  .folder-item > button:first-child { flex: 1; }
  .quiet-button { min-width: 30px; padding: 0 5px; border: 0; background: transparent; }
  .new-folder { display: flex; gap: 5px; padding: 9px 3px 2px; border-top: 1px solid #edf0f4; }
  .new-folder input { flex: 1; width: 100px; }
  .new-folder button { width: 34px; padding: 0; font-size: 19px; }
  .library-main { min-width: 0; }
  .bookmark-editor { padding: 13px; margin-bottom: 11px; }
  .editor-heading { margin-bottom: 9px; font-weight: 650; }
  .editor-fields { display: flex; flex-wrap: wrap; gap: 7px; }
  .editor-fields > input:first-child { flex: 1 1 150px; }
  .editor-fields > input:nth-child(2) { flex: 2 1 250px; }
  .editor-fields select { flex: 1 1 130px; }
  .bar-toggle { display: flex; align-items: center; gap: 6px; padding: 0 4px; white-space: nowrap; }
  .bar-toggle input { width: 15px; height: 15px; }
  .result-list { overflow: auto; }
  .result-row { display: flex; align-items: center; gap: 7px; padding: 10px 12px; border-bottom: 1px solid #edf0f4; }
  .result-row:last-child { border-bottom: 0; }
  .result-row > button { flex: 0 0 auto; min-height: 30px; padding: 0 9px; }
  .result-copy { display: flex; flex: 1; flex-direction: column; min-width: 80px; gap: 3px; overflow: hidden; }
  .result-copy strong, .result-copy span { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .result-copy span, .result-copy small { color: #758094; font-size: 11px; }
  .safety-warning { color: #a3482f !important; }
  .empty-state { padding: 20px; margin: 0; color: #778092; text-align: center; }
  .library-message { padding: 9px 12px; margin: 0 0 12px; color: #713e19; background: #fff2dd; border-radius: 8px; }
  .dialog-backdrop { position: fixed; inset: 0; display: grid; place-items: center; padding: 20px; background: rgb(18 22 31 / 38%); }
  .clear-dialog { position: relative; width: min(420px, 100%); margin: 0; padding: 22px; background: white; border: 1px solid #e1e4eb; border-radius: 14px; box-shadow: 0 20px 70px #171a2440; }
  .clear-dialog h2 { margin: 0 0 9px; font-size: 20px; }
  .clear-dialog p { color: #697386; line-height: 1.5; }
  .clear-dialog label { display: block; margin: 18px 0 6px; font-weight: 600; }
  .clear-dialog select { width: 100%; }
  .clear-data-option { display: flex !important; align-items: center; gap: 9px; margin: 14px 0 !important; }
  .clear-data-option input { width: 16px; height: 16px; margin: 0; accent-color: #4459e8; }
  .dialog-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 20px; }
  .download-location label { color: #4a5261; font-weight: 600; }
  .download-location #download-directory { flex: 1; }
  .download-ask-setting { display: flex; align-items: center; gap: 7px; white-space: nowrap; }
  .download-ask-setting input { width: 15px; height: 15px; margin: 0; accent-color: #536fce; }
  .download-progress { width: min(420px, 100%); height: 8px; accent-color: #536fce; }
  .download-row .result-copy { gap: 6px; }
  .privacy-settings { max-width: 720px; }
  .privacy-card { padding: 18px; border: 1px solid #dfe4ec; border-radius: 12px; background: #fff; }
  .performance-card { display: grid; justify-items: start; gap: 10px; }
  .performance-card h2 { margin: 0; font-size: 15px; }
  .performance-card p { margin: 0; }
  .performance-card select { min-width: 190px; }
  .updater-card { display: grid; justify-items: start; gap: 11px; }
  .updater-card h2 { margin: 0; font-size: 15px; }
  .updater-card > p { margin: 0; }
  .updater-card select { min-width: min(320px, 100%); }
  .updater-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; }
  .updater-actions small { color: #586174; }
  .privacy-card + .privacy-card { margin-top: 12px; }
  .privacy-toggle { display: flex; align-items: flex-start; gap: 12px; cursor: pointer; }
  .privacy-toggle input { width: 18px; height: 18px; margin: 1px 0 0; accent-color: #4459e8; }
  .privacy-toggle span { display: grid; gap: 5px; }
  .privacy-toggle small, .privacy-card p { color: #586174; }
  .privacy-card p { margin: 14px 0 0 30px; line-height: 1.5; }
  .site-permissions-card h2 { margin: 0; font-size: 15px; }
  .site-permissions-card > p { margin: 8px 0 12px; }
  .site-permission-option { display: flex; align-items: center; gap: 9px; padding: 8px 0; }
  .site-permission-option input { width: 16px; height: 16px; margin: 0; accent-color: #4459e8; }
  .custom-filters-card h2 { margin: 0; font-size: 15px; }
  .custom-filters-card > p { margin: 8px 0 14px; }
  .custom-filters-card label { display: grid; gap: 7px; margin: 12px 0; }
  .custom-filters-card textarea { width: 100%; resize: vertical; font: 12px/1.5 ui-monospace, Consolas, monospace; }
  .custom-filters-card button + p { margin: 10px 0 0; }
  .user-script-warning { padding: 11px 13px; margin-bottom: 12px; color: #713e19; background: #fff2dd; border-radius: 8px; line-height: 1.45; }
  .user-script-editor { padding: 13px; margin-bottom: 12px; background: #fff; border: 1px solid #e0e3e9; border-radius: 11px; }
  .user-script-fields { display: grid; grid-template-columns: minmax(180px, 1fr) minmax(220px, 2fr); gap: 9px; }
  .user-script-fields > label { display: grid; align-content: start; gap: 5px; color: #4a5261; font-size: 12px; font-weight: 600; }
  .user-script-source { grid-column: 1 / -1; }
  .user-script-fields textarea { width: 100%; padding: 10px; resize: vertical; color: #242937; background: #fff; border: 1px solid #dce0e8; border-radius: 8px; font: 12px/1.5 ui-monospace, Consolas, monospace; }
  .user-script-enable, .user-script-row-toggle { display: flex !important; align-items: center; gap: 7px !important; }
  .user-script-enable input, .user-script-row-toggle input { width: 16px; height: 16px; margin: 0; accent-color: #4459e8; }
  .user-script-actions { display: flex; justify-content: flex-end; gap: 8px; }
  .user-script-list { flex: 1; }
  .tab-group-note { margin: 0 0 12px; color: #586174; }
  .tab-group-editor { display: flex; align-items: flex-end; flex-wrap: wrap; gap: 9px; padding: 13px; margin-bottom: 12px; background: #fff; border: 1px solid #e0e3e9; border-radius: 11px; }
  .tab-group-editor > label { display: grid; flex: 1 1 180px; gap: 5px; color: #4a5261; font-size: 12px; font-weight: 600; }
  .tab-group-editor-actions { display: flex; gap: 8px; }
  .tab-group-list { display: grid; gap: 10px; overflow: auto; }
  .tab-group-card { overflow: hidden; background: #fff; border: 1px solid #e0e3e9; border-radius: 11px; }
  .tab-group-card > header { display: flex; align-items: center; gap: 8px; padding: 9px 11px; border-bottom: 1px solid #edf0f4; }
  .tab-group-card > header strong { flex: 1; }
  .tab-group-card > header button { min-height: 30px; padding: 0 9px; }
  .tab-group-swatch { width: 12px; height: 12px; flex: 0 0 auto; border-radius: 50%; }
  .tab-group-members { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 4px 12px; padding: 9px 12px; }
  .tab-group-members label { display: flex; min-width: 0; align-items: center; gap: 7px; color: #384052; }
  .tab-group-members label span { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .tab-group-members label small { flex: 0 0 auto; color: #758094; }
  .tab-group-members input { width: 15px; height: 15px; flex: 0 0 auto; margin: 0; accent-color: #4459e8; }
  @media (max-width: 900px) {
    .library { padding: 16px; }
    .library-columns { grid-template-columns: minmax(130px, 180px) minmax(0, 1fr); }
    .user-script-fields { grid-template-columns: 1fr; }
    .user-script-source { grid-column: auto; }
    .user-script-actions { justify-content: flex-start; }
    .result-row { flex-wrap: wrap; }
    .result-copy { flex-basis: 100%; }
  }
</style>

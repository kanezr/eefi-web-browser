export async function loadBrowserBootstrap(invoke) {
  const [privateMode] = await Promise.all([
    invoke("browser_window_mode"),
    invoke("download_directory"),
    invoke("download_ask_where_to_save"),
  ]);
  const [tabs] = await Promise.all([
    invoke("restore_session"),
    privateMode ? Promise.resolve() : invoke("downloads_recover"),
  ]);
  const [groups, activeTabId] = await Promise.all([
    invoke("tab_groups_list"),
    invoke("active_tab_id"),
  ]);
  return { privateMode, tabs, groups, activeTabId };
}

export function createResizeScheduler({
  requestFrame,
  cancelFrame,
  getBounds,
  getActiveTabId,
  resize,
  onError,
}) {
  let frame;
  let inFlight = false;
  let pending = false;
  let lastTabId;
  let lastBounds;

  async function flush() {
    if (inFlight) {
      pending = true;
      return;
    }
    inFlight = true;
    try {
      do {
        pending = false;
        const bounds = getBounds();
        if (!bounds) continue;
        const tabId = getActiveTabId();
        if (
          lastBounds &&
          lastTabId === tabId &&
          lastBounds.x === bounds.x &&
          lastBounds.y === bounds.y &&
          lastBounds.width === bounds.width &&
          lastBounds.height === bounds.height
        ) {
          continue;
        }
        try {
          await resize(bounds);
          lastBounds = { ...bounds };
          lastTabId = tabId;
        } catch (error) {
          onError(error);
        }
      } while (pending);
    } finally {
      inFlight = false;
    }
  }

  function schedule() {
    if (frame !== undefined) return;
    frame = requestFrame(() => {
      frame = undefined;
      void flush();
    });
  }

  function invalidate() {
    lastBounds = undefined;
    lastTabId = undefined;
  }

  function cancel() {
    if (frame === undefined) return;
    cancelFrame(frame);
    frame = undefined;
  }

  return { flush, schedule, invalidate, cancel };
}

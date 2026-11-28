export type BrowserTab = {
  id: string;
  title: string;
  url: string;
  groupId: string | null;
  sleeping: boolean;
};

export type BrowserTabGroup = { id: string; name: string; color: string };
export type WebviewBounds = { x: number; y: number; width: number; height: number };
export type BrowserInvoke = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export function loadBrowserBootstrap(
  invoke: BrowserInvoke,
): Promise<{
  privateMode: boolean;
  tabs: BrowserTab[];
  groups: BrowserTabGroup[];
  activeTabId: string | null;
}>;

export function createResizeScheduler(options: {
  requestFrame: (callback: () => void) => number;
  cancelFrame: (frame: number) => void;
  getBounds: () => WebviewBounds | null;
  getActiveTabId: () => string;
  resize: (bounds: WebviewBounds) => Promise<unknown>;
  onError: (error: unknown) => void;
}): {
  flush: () => Promise<void>;
  schedule: () => void;
  invalidate: () => void;
  cancel: () => void;
};

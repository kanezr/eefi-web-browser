import assert from "node:assert/strict";
import test from "node:test";
import { createResizeScheduler, loadBrowserBootstrap } from "../src/browser-runtime.mjs";

function deferred() {
  let resolve;
  const promise = new Promise((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

test("startup reads independent browser state in parallel phases", async () => {
  const calls = [];
  let activeCalls = 0;
  let maxActiveCalls = 0;
  const values = {
    browser_window_mode: false,
    download_directory: "C:\\Downloads",
    download_ask_where_to_save: true,
    restore_session: [{ id: "1", title: "Home", url: "about:blank" }],
    downloads_recover: 0,
    tab_groups_list: [],
    active_tab_id: "1",
  };
  const invoke = async (command) => {
    calls.push(command);
    activeCalls += 1;
    maxActiveCalls = Math.max(maxActiveCalls, activeCalls);
    await new Promise((resolve) => setImmediate(resolve));
    activeCalls -= 1;
    return values[command];
  };

  const bootstrap = await loadBrowserBootstrap(invoke);

  assert.equal(maxActiveCalls, 3);
  assert.deepEqual(new Set(calls.slice(0, 3)), new Set([
    "browser_window_mode",
    "download_directory",
    "download_ask_where_to_save",
  ]));
  assert.deepEqual(bootstrap, {
    privateMode: false,
    tabs: values.restore_session,
    groups: [],
    activeTabId: "1",
  });
});

test("private-window startup skips persistent download recovery", async () => {
  const calls = [];
  const values = {
    browser_window_mode: true,
    download_directory: "C:\\Downloads",
    download_ask_where_to_save: true,
    restore_session: [],
    tab_groups_list: [],
    active_tab_id: null,
  };
  const bootstrap = await loadBrowserBootstrap(async (command) => {
    calls.push(command);
    return values[command];
  });

  assert.equal(bootstrap.privateMode, true);
  assert.equal(calls.includes("downloads_recover"), false);
});

test("startup propagates command failures", async () => {
  await assert.rejects(
    loadBrowserBootstrap(async (command) => {
      if (command === "download_directory") throw new Error("profile unavailable");
      return undefined;
    }),
    /profile unavailable/,
  );
});

function createFrameHarness() {
  let nextFrame = 0;
  const frames = new Map();
  return {
    requestFrame(callback) {
      nextFrame += 1;
      frames.set(nextFrame, callback);
      return nextFrame;
    },
    cancelFrame(frame) {
      frames.delete(frame);
    },
    runNext() {
      const [frame, callback] = frames.entries().next().value ?? [];
      if (callback) {
        frames.delete(frame);
        callback();
      }
    },
    get size() {
      return frames.size;
    },
  };
}

test("resize scheduler coalesces frames and skips unchanged bounds", async () => {
  const harness = createFrameHarness();
  const bounds = { x: 0, y: 128, width: 1180, height: 652 };
  let activeTabId = "tab-1";
  const calls = [];
  const scheduler = createResizeScheduler({
    ...harness,
    getBounds: () => bounds,
    getActiveTabId: () => activeTabId,
    resize: async (value) => calls.push(value),
    onError: (error) => {
      throw error;
    },
  });

  scheduler.schedule();
  scheduler.schedule();
  scheduler.schedule();
  assert.equal(harness.size, 1);
  harness.runNext();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls.length, 1);

  scheduler.schedule();
  harness.runNext();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls.length, 1);

  bounds.width = 900;
  scheduler.schedule();
  harness.runNext();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls.length, 2);

  activeTabId = "tab-2";
  scheduler.schedule();
  harness.runNext();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls.length, 3);
});

test("resize scheduler applies the latest bounds after an in-flight resize", async () => {
  const first = deferred();
  const second = deferred();
  const bounds = { x: 0, y: 128, width: 1180, height: 652 };
  const calls = [];
  const scheduler = createResizeScheduler({
    requestFrame: () => 1,
    cancelFrame: () => {},
    getBounds: () => ({ ...bounds }),
    getActiveTabId: () => "tab-1",
    resize: (value) => {
      calls.push(value);
      return calls.length === 1 ? first.promise : second.promise;
    },
    onError: (error) => {
      throw error;
    },
  });

  const firstFlush = scheduler.flush();
  bounds.width = 900;
  await scheduler.flush();
  first.resolve();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls.length, 2);
  assert.equal(calls[1].width, 900);
  second.resolve();
  await firstFlush;
});

test("resize scheduler invalidation and cancellation force a fresh update", async () => {
  const harness = createFrameHarness();
  let resized = 0;
  const scheduler = createResizeScheduler({
    ...harness,
    getBounds: () => ({ x: 0, y: 0, width: 800, height: 600 }),
    getActiveTabId: () => "tab-1",
    resize: async () => {
      resized += 1;
    },
    onError: (error) => {
      throw error;
    },
  });

  scheduler.schedule();
  scheduler.cancel();
  assert.equal(harness.size, 0);
  scheduler.schedule();
  harness.runNext();
  await new Promise((resolve) => setImmediate(resolve));
  scheduler.invalidate();
  await scheduler.flush();
  assert.equal(resized, 2);
});

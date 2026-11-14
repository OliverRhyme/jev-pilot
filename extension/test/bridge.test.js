// The bridge core's behaviour that the recorded conversations do not show:
// what it does with Chrome on its own account.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createBridge, listen } from "../bridge.js";
import { fakeChrome, settle } from "./fake-chrome.js";

const SCRIPTS = JSON.parse(readFileSync(new URL("../scripts.json", import.meta.url), "utf8"));
const UA = "Mozilla/5.0 (Macintosh) AppleWebKit/537.36 Chrome/158.0.0.0 Safari/537.36";

function bridged({ policy = { sites: [], owner_tabs: "group" }, timers = [] } = {}) {
  const fake = fakeChrome();
  const posted = [];
  const bridge = createBridge({
    chrome: fake.chrome,
    post: (m) => posted.push(m),
    loadPolicy: async () => policy,
    scripts: SCRIPTS,
    userAgent: UA,
    setTimer: (fn, ms) => timers.push({ fn, ms }),
  });
  listen(fake.chrome, bridge);
  return { fake, posted, bridge, timers };
}

const started = { type: "run_started", pid: 4242, goal: "Star the repository", sites: ["https://github.com"] };

async function attachedTo(fake, bridge, id, url = "https://github.com/") {
  fake.state.tabs.set(id, { id, url, title: "", groupId: 31, windowId: 1 });
  fake.state.targets.set(id, `T${id}`);
  await bridge.receive({ type: "request", id: 90 + id, op: "attach", tab: id });
  await settle();
}

test("a run starting raises a notice naming its sites and goal", async () => {
  const { fake, bridge } = bridged();
  await bridge.receive(started);
  await bridge.receive({ ...started, pid: 5, sites: [], goal: "x".repeat(100) });
  const notices = fake.calls.filter(([call]) => call === "notifications.create");
  assert.equal(notices[0][2].message, "jev-pilot is driving https://github.com: Star the repository");
  assert.equal(notices[0][2].title, "jev-pilot");
  assert.equal(notices[1][2].message, `jev-pilot is driving any site: ${"x".repeat(79)}…`);
});

test("release lets go of every tab the run holds, and closes none", async () => {
  const { fake, bridge } = bridged();
  await bridge.receive(started);
  await attachedTo(fake, bridge, 7);
  await attachedTo(fake, bridge, 8);
  await bridge.receive({ type: "release" });
  const detached = fake.calls.filter(([call]) => call === "debugger.detach").map(([, d]) => d.tabId);
  assert.deepEqual(detached.sort(), [7, 8]);
  assert.equal(fake.calls.some(([call]) => call === "tabs.remove"), false);
});

test("losing the connection lets go of every tab, as release does", async () => {
  const { fake, bridge } = bridged();
  await bridge.receive(started);
  await attachedTo(fake, bridge, 7);
  await bridge.portLost();
  assert.deepEqual(
    fake.calls.filter(([call]) => call === "debugger.detach").map(([, d]) => d.tabId),
    [7],
  );
});

test("a held tab that arrives off the allowed sites is let go of, saying where it went", async () => {
  const { fake, bridge, posted } = bridged({ policy: { sites: ["https://github.com"], owner_tabs: "group" } });
  await bridge.receive(started);
  await attachedTo(fake, bridge, 7);
  posted.length = 0;
  await fake.emit("debugger.onEvent", { tabId: 7 }, "Page.frameNavigated", {
    frame: { id: "F", url: "https://github.com/next" },
  });
  await fake.emit("debugger.onEvent", { tabId: 7 }, "Page.frameNavigated", {
    frame: { id: "C", parentId: "F", url: "https://ads.test/" },
  });
  await fake.emit("debugger.onEvent", { tabId: 7 }, "Page.frameNavigated", {
    frame: { id: "F", url: "https://elsewhere.test/x" },
  });
  await settle();
  assert.deepEqual(posted.map((m) => m.type), ["cdp", "cdp", "detached"]);
  assert.deepEqual(posted[2], {
    type: "detached",
    tab: 7,
    reason: "left_allowed_sites",
    url: "https://elsewhere.test/x",
  });
  assert.ok(fake.calls.some(([call, d]) => call === "debugger.detach" && d.tabId === 7));
});

test("a tab with no opener is claimed by the navigation that opened it, or reported without one", async () => {
  const { fake, bridge, posted, timers } = bridged();
  await bridge.receive(started);
  await attachedTo(fake, bridge, 7);
  posted.length = 0;
  for (const id of [11, 12]) {
    fake.state.tabs.set(id, { id, url: "", pendingUrl: "https://github.com/x", title: "", groupId: -1, windowId: 1 });
    fake.state.targets.set(id, `T${id}`);
    await fake.emit("tabs.onCreated", fake.state.tabs.get(id));
  }
  await fake.emit("webNavigation.onCreatedNavigationTarget", { sourceTabId: 7, tabId: 11 });
  for (const timer of timers) {
    assert.equal(timer.ms, 500);
    timer.fn();
  }
  await settle();
  assert.deepEqual(posted, [
    { type: "tab_created", tab: 11, target: "T11", opener: 7, url: "https://github.com/x", title: "" },
    { type: "tab_created", tab: 12, target: "T12", url: "https://github.com/x", title: "" },
  ]);
});

test("tabs opened while no run is connected are not reported", async () => {
  const { fake, posted } = bridged();
  await fake.emit("tabs.onCreated", { id: 3, openerTabId: 1, url: "u", title: "t" });
  await settle();
  assert.deepEqual(posted, []);
});

test("a new tab opens in a new window when no normal window is open, in the jev-pilot group", async () => {
  const { fake, bridge, posted } = bridged();
  await bridge.receive(started);
  fake.state.groups = [];
  fake.when("windows.getLastFocused", () => {
    throw new Error("No last-focused window");
  });
  await bridge.receive({ type: "request", id: 1, op: "create", url: "https://github.com/" });
  await settle();
  const created = fake.calls.find(([call]) => call === "tabs.create")[1];
  assert.equal(created.windowId, 2, "the window just made");
  assert.ok(fake.calls.some(([call, , p]) => call === "tabGroups.update" && p.title === "jev-pilot"));
  assert.equal(posted.at(-1).type, "reply");
});

test("a reload is honoured once per version", async () => {
  const { fake, bridge } = bridged();
  await bridge.receive({ type: "reload" });
  await bridge.receive({ type: "reload" });
  assert.equal(fake.calls.filter(([call]) => call === "runtime.reload").length, 1);
  assert.deepEqual(fake.state.session, { reloaded: "0.1.3" });
});

test("a command to a tab the run does not hold is refused", async () => {
  const { bridge, posted, fake } = bridged();
  await bridge.receive({ type: "request", id: 4, op: "send", tab: 7, method: "Page.enable", params: {} });
  assert.deepEqual(posted.at(-1), {
    type: "reply",
    id: 4,
    error: { code: "no_tab", message: "the run does not hold tab 7" },
  });
  assert.equal(fake.calls.some(([call]) => call === "debugger.sendCommand"), false);
});

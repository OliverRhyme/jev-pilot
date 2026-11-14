// The bridge core's behaviour that the recorded conversations do not show:
// what it does with Chrome on its own account.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createBridge, listen } from "../bridge.js";
import { fakeChrome, settle } from "./fake-chrome.js";

const SCRIPTS = JSON.parse(readFileSync(new URL("../scripts.json", import.meta.url), "utf8"));
const UA = "Mozilla/5.0 (Macintosh) AppleWebKit/537.36 Chrome/158.0.0.0 Safari/537.36";

function bridged({ policy = { sites: [], owner_tabs: "group" }, timers = [], loadPolicy } = {}) {
  const fake = fakeChrome();
  const posted = [];
  const bridge = createBridge({
    chrome: fake.chrome,
    post: (m) => posted.push(m),
    loadPolicy: loadPolicy ?? (async () => policy),
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

test("a reload is honoured once per version, though a reload empties session storage", async () => {
  const { fake, bridge } = bridged();
  await bridge.receive({ type: "reload" });
  await bridge.receive({ type: "reload" });
  assert.equal(fake.calls.filter(([call]) => call === "runtime.reload").length, 1);
  assert.equal(fake.state.storage.reloadedFrom, "0.1.3");
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

test("the bridge says what run it serves and which tabs it holds, for the popup", async () => {
  const { fake, bridge } = bridged();
  assert.deepEqual(bridge.state(), { running: false, goal: "", sites: [], tabs: [] });
  await bridge.receive(started);
  await attachedTo(fake, bridge, 7);
  assert.deepEqual(bridge.state(), {
    running: true,
    goal: "Star the repository",
    sites: ["https://github.com"],
    tabs: [7],
  });
  await bridge.receive({ type: "release" });
  assert.deepEqual(bridge.state(), { running: false, goal: "", sites: [], tabs: [] });
});

/** A policy that is read only when the test lets it be. */
function slowPolicy(policy) {
  const waiting = [];
  return {
    loadPolicy: () => new Promise((resolve) => waiting.push(() => resolve(policy))),
    release: () => {
      for (const go of waiting.splice(0)) {
        go();
      }
    },
  };
}

test("a command sent while a navigation off the sites is checked is not carried out", async () => {
  const policy = { sites: ["https://github.com"], owner_tabs: "group" };
  const slow = slowPolicy(policy);
  const { fake, bridge, posted } = bridged({ loadPolicy: slow.loadPolicy });
  await bridge.receive(started);
  const attaching = attachedTo(fake, bridge, 7);
  await settle();
  slow.release();
  await attaching;
  posted.length = 0;
  fake.state.tabs.get(7).url = "https://elsewhere.test/x";
  const navigated = fake.emit("debugger.onEvent", { tabId: 7 }, "Page.frameNavigated", {
    frame: { id: "F", url: "https://elsewhere.test/x" },
  });
  const sent = bridge.receive({
    type: "request",
    id: 2,
    op: "send",
    tab: 7,
    method: "DOMSnapshot.captureSnapshot",
    params: {},
  });
  for (let i = 0; i < 5; i += 1) {
    await settle();
    slow.release();
  }
  await Promise.all([navigated, sent]);
  assert.equal(
    fake.calls.some(([call, , method]) => call === "debugger.sendCommand" && method === "DOMSnapshot.captureSnapshot"),
    false,
  );
  assert.deepEqual(posted.map((m) => m.type), ["detached", "reply"]);
  assert.equal(posted[0].reason, "left_allowed_sites");
  assert.equal(posted[1].error.code, "no_tab");
});

test("each command checks where its tab is now, and lets go of a tab off the sites", async () => {
  const { fake, bridge, posted } = bridged({ policy: { sites: ["https://github.com"], owner_tabs: "group" } });
  await bridge.receive(started);
  await attachedTo(fake, bridge, 7);
  posted.length = 0;
  fake.state.tabs.get(7).url = "https://elsewhere.test/x";
  await bridge.receive({ type: "request", id: 3, op: "send", tab: 7, method: "Page.getFrameTree", params: {} });
  await settle();
  assert.equal(fake.calls.some(([call]) => call === "debugger.sendCommand"), false);
  assert.deepEqual(posted[0], {
    type: "detached",
    tab: 7,
    reason: "left_allowed_sites",
    url: "https://elsewhere.test/x",
  });
  assert.equal(posted[1].error.code, "no_tab");
  assert.ok(fake.calls.some(([call, d]) => call === "debugger.detach" && d.tabId === 7));
});

test("a committed navigation off the sites lets go of the tab, whatever DevTools says", async () => {
  const { fake, bridge, posted } = bridged({ policy: { sites: ["https://github.com"], owner_tabs: "group" } });
  await bridge.receive(started);
  await attachedTo(fake, bridge, 7);
  posted.length = 0;
  await fake.emit("webNavigation.onCommitted", { tabId: 7, frameId: 3, url: "https://ads.test/" });
  await fake.emit("webNavigation.onCommitted", { tabId: 9, frameId: 0, url: "https://ads.test/" });
  await settle();
  assert.deepEqual(posted, [], "a frame, or a tab the run does not hold, is not the run's business");
  await fake.emit("webNavigation.onCommitted", { tabId: 7, frameId: 0, url: "https://elsewhere.test/x" });
  await settle();
  const alone = [...posted];
  await fake.emit("debugger.onEvent", { tabId: 7 }, "Page.frameNavigated", {
    frame: { id: "F", url: "https://elsewhere.test/x" },
  });
  await settle();
  assert.deepEqual(alone, posted, "said once");
  assert.deepEqual(posted, [
    { type: "detached", tab: 7, reason: "left_allowed_sites", url: "https://elsewhere.test/x" },
  ]);
});

test("every request is answered, even when the policy file cannot be read", async () => {
  const { fake, bridge, posted } = bridged({
    loadPolicy: async () => {
      throw new SyntaxError("Unexpected token");
    },
  });
  await bridge.receive(started);
  await bridge.receive({ type: "request", id: 5, op: "create", url: "https://github.com/" });
  await bridge.receive({ type: "request", id: 6, op: "close" });
  await settle();
  assert.deepEqual(
    posted.map((m) => [m.id, m.error?.code]),
    [
      [5, "refused_site"],
      [6, "refused_method"],
    ],
  );
  assert.equal(fake.calls.some(([call]) => call === "tabs.create"), false);
});

test("going back looks up the page it goes back to, and refuses one off the sites", async () => {
  const { fake, bridge, posted } = bridged({ policy: { sites: ["https://github.com"], owner_tabs: "group" } });
  await bridge.receive(started);
  await attachedTo(fake, bridge, 7);
  const history = {
    currentIndex: 1,
    entries: [
      { id: 3, url: "https://elsewhere.test/" },
      { id: 4, url: "https://github.com/" },
    ],
  };
  fake.when("debugger.sendCommand", () => history);
  posted.length = 0;
  await bridge.receive({
    type: "request",
    id: 8,
    op: "send",
    tab: 7,
    method: "Page.navigateToHistoryEntry",
    params: { entryId: 3 },
  });
  await settle();
  assert.equal(posted.at(-1).error.code, "refused_site");
  assert.deepEqual(
    fake.calls.filter(([call]) => call === "debugger.sendCommand").map(([, , method]) => method),
    ["Page.getNavigationHistory"],
  );
});

test("on starting, the extension lets go of tabs an earlier worker left attached", async () => {
  const { fake, bridge } = bridged();
  fake.state.targets.set(7, "T7");
  fake.state.targets.set(8, "T8");
  fake.state.attached = new Set([7]);
  await bridge.forgetEarlierRun();
  assert.deepEqual(
    fake.calls.filter(([call]) => call === "debugger.detach").map(([, d]) => d.tabId),
    [7],
  );
});

test("an attach a managed Chrome's policy forbids says so, naming the site", async () => {
  for (const chromeSaid of [
    "Host access is restricted by policy.",
    "This page cannot be scripted due to an ExtensionsSettings policy.",
    "Screenshot capture is restricted by policy.",
    "Screenshot capture is restricted on this target.",
  ]) {
    const { fake, bridge, posted } = bridged();
    await bridge.receive(started);
    fake.state.tabs.set(7, { id: 7, url: "https://bank.test/account", title: "", groupId: 31, windowId: 1 });
    fake.when("debugger.attach", () => {
      throw new Error(chromeSaid);
    });
    await bridge.receive({ type: "request", id: 4, op: "attach", tab: 7 });
    await settle();
    assert.deepEqual(
      posted.at(-1).error,
      { code: "attach_failed", message: "this Chrome's policy does not let extensions debug https://bank.test" },
      chromeSaid,
    );
  }
});

test("any other failure to attach is passed on in Chrome's words", async () => {
  const { fake, bridge, posted } = bridged();
  await bridge.receive(started);
  fake.state.tabs.set(7, { id: 7, url: "https://github.com/", title: "", groupId: 31, windowId: 1 });
  fake.when("debugger.attach", () => {
    throw new Error("Another debugger is already attached to the tab with id: 7.");
  });
  await bridge.receive({ type: "request", id: 4, op: "attach", tab: 7 });
  await settle();
  assert.deepEqual(posted.at(-1).error, {
    code: "attach_failed",
    message: "Another debugger is already attached to the tab with id: 7.",
  });
});

test("a page snapshot leaves Chrome without the value of any password field", async () => {
  const { fake, bridge, posted } = bridged();
  await bridge.receive(started);
  await attachedTo(fake, bridge, 7);
  const strings = ["#document", "INPUT", "type", "password", "hunter2", "text", "alice", "PassWord"];
  fake.when("debugger.sendCommand", () => ({
    strings,
    documents: [
      {
        nodes: {
          nodeName: [0, 1, 1, 1],
          attributes: [[], [2, 3], [2, 5], [2, 7]],
          inputValue: { index: [1, 2, 3], value: [4, 6, 4] },
        },
      },
    ],
  }));
  await bridge.receive({
    type: "request",
    id: 9,
    op: "send",
    tab: 7,
    method: "DOMSnapshot.captureSnapshot",
    params: { computedStyles: [] },
  });
  await settle();
  const reply = posted.at(-1);
  assert.equal(reply.id, 9);
  assert.deepEqual(reply.result.documents[0].nodes.inputValue, { index: [2], value: [6] });
  const values = reply.result.documents[0].nodes.inputValue.value.map((i) => reply.result.strings[i]);
  assert.deepEqual(values, ["alice"]);
  assert.equal(JSON.stringify(reply).includes("hunter2"), false, "the words themselves stay behind");
});

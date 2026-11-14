// The bridge's messages as the extension builds and reads them. The shapes
// are the run's (jev_pilot::browser::bridge::Message), so these are checked
// against literal JSON.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as protocol from "../protocol.js";

test("the extension says hello with the protocol, its version, the browser, the pause and the policy", () => {
  assert.deepEqual(
    protocol.hello({
      version: "0.1.3",
      userAgent: "Mozilla/5.0 (Macintosh) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/158.0.0.0 Safari/537.36",
      paused: false,
      policy: { sites: [], owner_tabs: "group" },
    }),
    {
      type: "hello",
      protocol: 1,
      extension_version: "0.1.3",
      product: "Chrome/158.0.0.0",
      paused: false,
      policy: { sites: [], owner_tabs: "group" },
    },
  );
});

test("the browser is named as Browser.getVersion names it", () => {
  const edge =
    "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/158.0.0.0 Safari/537.36 Edg/158.0.0.0";
  assert.equal(protocol.product(edge), "Edg/158.0.0.0");
  assert.equal(protocol.product("Mozilla/5.0 Chrome/158.0.7400.12 Safari/537.36"), "Chrome/158.0.7400.12");
  assert.equal(protocol.product("Mozilla/5.0 Gecko/20100101 Firefox/140.0"), "unknown");
});

test("a reply carries a result or a coded error", () => {
  assert.deepEqual(protocol.reply(3, { tab: 9, target: "T9" }), {
    type: "reply",
    id: 3,
    result: { tab: 9, target: "T9" },
  });
  assert.deepEqual(protocol.refused(4, { code: "refused_site", message: "why" }), {
    type: "reply",
    id: 4,
    error: { code: "refused_site", message: "why" },
  });
});

test("events say which tab they came from", () => {
  assert.deepEqual(protocol.cdp(7, undefined, "Page.loadEventFired", { timestamp: 1 }), {
    type: "cdp",
    tab: 7,
    method: "Page.loadEventFired",
    params: { timestamp: 1 },
  });
  assert.deepEqual(protocol.cdp(7, "C1", "Page.loadEventFired", {}), {
    type: "cdp",
    tab: 7,
    session: "C1",
    method: "Page.loadEventFired",
    params: {},
  });
  assert.deepEqual(
    protocol.tabCreated({ tab: 8, target: "T8", opener: 7, url: "https://example.test/", title: "Example" }),
    { type: "tab_created", tab: 8, target: "T8", opener: 7, url: "https://example.test/", title: "Example" },
  );
  assert.deepEqual(
    protocol.tabCreated({ tab: 8, target: "T8", opener: undefined, url: "u", title: "t" }),
    { type: "tab_created", tab: 8, target: "T8", url: "u", title: "t" },
    "no opener is left out",
  );
  assert.deepEqual(protocol.tabRemoved(8), { type: "tab_removed", tab: 8 });
  assert.deepEqual(protocol.detached(7, "canceled_by_user"), {
    type: "detached",
    tab: 7,
    reason: "canceled_by_user",
  });
  assert.deepEqual(protocol.detached(7, "left_allowed_sites", "https://elsewhere.test/"), {
    type: "detached",
    tab: 7,
    reason: "left_allowed_sites",
    url: "https://elsewhere.test/",
  });
});

test("only well-formed messages from the host are read", () => {
  assert.deepEqual(protocol.read({ type: "request", id: 1, op: "tabs" }), {
    type: "request",
    id: 1,
    op: "tabs",
  });
  assert.deepEqual(protocol.read({ type: "release" }), { type: "release" });
  assert.deepEqual(protocol.read({ type: "reload" }), { type: "reload" });
  assert.deepEqual(protocol.read({ type: "run_started", pid: 1, goal: "g", sites: [] }), {
    type: "run_started",
    pid: 1,
    goal: "g",
    sites: [],
  });
  for (const bad of [
    null,
    "release",
    { type: "hello" },
    { type: "request", op: "tabs" },
    { type: "request", id: 1, op: "close" },
    { type: "request", id: 1, op: "send", tab: 7 },
    { type: "request", id: 1, op: "attach" },
    { type: "request", id: 1, op: "create" },
  ]) {
    assert.equal(protocol.read(bad), null, JSON.stringify(bad));
  }
});

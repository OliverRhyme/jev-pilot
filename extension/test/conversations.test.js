// The recorded bridge conversations (tests/fixtures/extension/), replayed
// from the extension's side. The run's frames are given to the extension;
// what Chrome does is read off the recording too, through a fake `chrome`;
// and the frames the extension sends must be the recorded ones, in order.
// The run's side replays the same files in tests/cdp.rs.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { createBridge, listen } from "../bridge.js";
import { fakeChrome, settle } from "./fake-chrome.js";

const FOLDER = new URL("../../tests/fixtures/extension/", import.meta.url);
const SCRIPTS = JSON.parse(readFileSync(new URL("../scripts.json", import.meta.url), "utf8"));

/** Refusals the policy gives, before Chrome is asked anything. */
const POLICY_CODES = ["refused_method", "refused_script", "refused_site", "refused_page", "refused_tab", "paused"];

const isEvent = (frame) => ["cdp", "tab_created", "tab_removed", "detached", "status"].includes(frame.type);

/** Make Chrome do what the recording says it did. */
async function fire(fake, event) {
  switch (event.type) {
    case "cdp": {
      const source = event.session === undefined ? { tabId: event.tab } : { tabId: event.tab, sessionId: event.session };
      await fake.emit("debugger.onEvent", source, event.method, event.params);
      break;
    }
    case "tab_created": {
      const tab = {
        id: event.tab,
        url: event.url,
        title: event.title,
        groupId: -1,
        windowId: 1,
        openerTabId: event.opener,
      };
      fake.state.tabs.set(tab.id, tab);
      fake.state.targets.set(tab.id, event.target);
      await fake.emit("tabs.onCreated", tab);
      break;
    }
    case "tab_removed":
      await fake.emit("tabs.onRemoved", event.tab, {});
      break;
    case "detached":
      await fake.emit("debugger.onDetach", { tabId: event.tab }, event.reason);
      break;
    case "status":
      fake.state.storage.paused = event.paused;
      await fake.emit("storage.onChanged", { paused: { newValue: event.paused } }, "local");
      break;
    default:
      throw new Error(`no way to make Chrome send ${event.type}`);
  }
  await settle();
}

/**
 * Set Chrome up to answer `request` as recorded in `answers` (the frames the
 * extension sent before the run's next one). Returns the events to fire
 * before the request is given, and those to fire after its reply.
 */
function prepare(fake, request, answers) {
  const reply = answers.find((a) => a.type === "reply" && a.id === request.id);
  const at = reply ? answers.indexOf(reply) : answers.length;
  const before = answers.slice(0, at).filter(isEvent);
  const after = answers.slice(at + 1).filter(isEvent);
  const first = before.filter((e) => e.type === "status");
  const during = before.filter((e) => e.type !== "status");
  const failure = reply?.error && !POLICY_CODES.includes(reply.error.code) ? reply.error : undefined;
  const fail = () => {
    throw new Error(failure.message);
  };

  switch (request.op) {
    case "tabs": {
      const tabs = reply.result.tabs.map((t) => ({
        id: t.tab,
        url: t.url,
        title: t.title,
        groupId: fake.state.tabs.get(t.tab)?.groupId ?? 31,
        windowId: 1,
        openerTabId: t.opener,
      }));
      for (const [i, t] of reply.result.tabs.entries()) {
        fake.state.tabs.set(t.tab, fake.state.tabs.get(t.tab) ?? tabs[i]);
        fake.state.targets.set(t.tab, t.target);
      }
      fake.when("tabs.query", () => tabs);
      break;
    }
    case "create":
      if (reply.result) {
        fake.when("tabs.create", () => {
          const tab = { id: reply.result.tab, url: request.url, title: "", groupId: -1, windowId: 1 };
          fake.state.tabs.set(tab.id, tab);
          fake.state.targets.set(tab.id, reply.result.target);
          return tab;
        });
      }
      break;
    case "attach":
    case "activate":
      if (reply.error?.code === "refused_tab") {
        fake.state.tabs.get(request.tab).groupId = -1;
      } else if (reply.error?.code === "no_tab") {
        fake.when("tabs.get", fail);
      } else if (reply.error?.code === "attach_failed") {
        fake.when("debugger.attach", fail);
      }
      break;
    case "send":
      if (reply?.error && POLICY_CODES.includes(reply.error.code)) {
        break;
      }
      fake.when("debugger.sendCommand", async () => {
        for (const event of during) {
          await fire(fake, event);
        }
        if (!reply) {
          throw new Error("Detached while handling command.");
        }
        return failure ? fail() : reply.result;
      });
      break;
    default:
      break;
  }
  return { first, after: reply ? after : [] };
}

for (const name of readdirSync(FOLDER).filter((f) => f.endsWith(".json")).sort()) {
  test(`${name} replays from the extension's side`, async () => {
    const frames = JSON.parse(readFileSync(new URL(name, FOLDER), "utf8"));
    const hello = frames[0].frame;
    assert.equal(hello.type, "hello", "a conversation starts with the extension's hello");
    const fake = fakeChrome({ version: hello.extension_version, paused: hello.paused });
    const posted = [];
    const bridge = createBridge({
      chrome: fake.chrome,
      post: (message) => posted.push(message),
      loadPolicy: async () => hello.policy,
      scripts: SCRIPTS,
      userAgent: `Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 ${hello.product} Safari/537.36`,
    });
    listen(fake.chrome, bridge);
    await bridge.connect();
    await settle();

    for (const [i, { to, frame }] of frames.entries()) {
      if (to !== "extension") {
        continue;
      }
      const next = frames.findIndex((f, j) => j > i && f.to === "extension");
      const answers = frames
        .slice(i + 1, next < 0 ? frames.length : next)
        .filter((f) => f.from === "extension")
        .map((f) => f.frame);
      const { first, after } = frame.type === "request" ? prepare(fake, frame, answers) : { first: [], after: [] };
      for (const event of first) {
        await fire(fake, event);
      }
      const chromeCalls = fake.calls.length;
      await bridge.receive(frame);
      await settle();
      const refusal = answers.find((a) => a.type === "reply" && a.id === frame.id)?.error;
      if (refusal && POLICY_CODES.includes(refusal.code)) {
        const asked = fake.calls.slice(chromeCalls).filter(([call]) =>
          ["debugger.attach", "debugger.sendCommand", "tabs.create", "tabs.update"].includes(call),
        );
        assert.deepEqual(asked, [], `${refusal.code} is the policy's, so Chrome was not asked`);
      }
      for (const event of after) {
        await fire(fake, event);
      }
    }

    assert.deepEqual(
      posted,
      frames.filter((f) => f.from === "extension").map((f) => f.frame),
    );
  });
}

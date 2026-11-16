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
 * The Chrome the conversations were recorded against, from the same page
 * targets the run's side lists (tests/fixtures/cdp/signup/targets.json):
 * each page a tab, numbered from 1. The owner has put the sign-up page in
 * the jev-pilot tab group; the blank tab is theirs alone.
 */
function chromeAsRecorded(fake) {
  const targets = JSON.parse(readFileSync(new URL("../../tests/fixtures/cdp/signup/targets.json", import.meta.url), "utf8"));
  const pages = targets.targetInfos.filter((t) => t.type === "page");
  for (const [i, page] of pages.entries()) {
    const id = i + 1;
    const grouped = page.url.endsWith("/signup");
    fake.state.tabs.set(id, { id, url: page.url, title: page.title, groupId: grouped ? 31 : -1, windowId: 1 });
    fake.state.targets.set(id, page.targetId);
  }
}

/** What Chrome says when a command fails, by method, in its own words. */
const CHROME_FAILS = { "DOM.resolveNode": "No node with given id found" };

/**
 * Set Chrome up for `request`, given the frames the extension sent before
 * the run's next one (`answers`). Chrome's state is its own, never the
 * recorded reply: the recording only says when Chrome did something the
 * fake cannot know of, such as the owner closing a tab, and the reply the
 * extension makes of it must match. Returns the events to fire before the
 * request is given, and those to fire after its reply.
 */
function prepare(fake, request, answers) {
  const reply = answers.find((a) => a.type === "reply" && a.id === request.id);
  const at = reply ? answers.indexOf(reply) : answers.length;
  const before = answers.slice(0, at).filter(isEvent);
  const after = answers.slice(at + 1).filter(isEvent);
  const first = before.filter((e) => e.type === "status");
  const during = before.filter((e) => e.type !== "status");

  if ((request.op === "attach" || request.op === "activate") && reply?.error?.code === "no_tab") {
    // The owner closed the tab, and Chrome has not yet said so.
    fake.state.tabs.delete(request.tab);
  }
  if (request.op === "send" && !(reply?.error && POLICY_CODES.includes(reply.error.code))) {
    const fails = reply?.error?.code === "chrome";
    fake.when("debugger.sendCommand", async () => {
      for (const event of during) {
        await fire(fake, event);
      }
      if (!reply) {
        throw new Error("Detached while handling command.");
      }
      if (fails) {
        throw new Error(CHROME_FAILS[request.method] ?? `no failure of ${request.method} is modelled`);
      }
      return reply.result;
    });
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
    chromeAsRecorded(fake);
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

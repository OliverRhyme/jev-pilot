// The service worker: Chrome's events wired to the bridge, and the bridge to
// the host. Everything it decides lives in the modules it imports.

import { createBridge, listen } from "./bridge.js";
import { createConnection } from "./connection.js";
import { product } from "./protocol.js";
import scripts from "./scripts.json" with { type: "json" };

/** The admin-owned policy file, read afresh for every request it decides. */
const loadPolicy = async () => (await fetch(chrome.runtime.getURL("policy.json"), { cache: "no-store" })).json();

let connection;
const bridge = createBridge({
  chrome,
  post: (message) => connection.post(message),
  loadPolicy,
  scripts,
  userAgent: navigator.userAgent,
});
connection = createConnection({ chrome, bridge });

// Listeners are registered at the top level, so Chrome finds them when it
// starts the worker for an event.
listen(chrome, bridge);
chrome.runtime.onStartup.addListener(() => connection.open());
chrome.runtime.onInstalled.addListener(() => connection.open());

// The popup asks for the state, and shows it.
chrome.runtime.onMessage.addListener((message, _sender, respond) => {
  if (message?.type !== "state") {
    return false;
  }
  (async () => {
    const policy = await loadPolicy().catch(() => null);
    const { paused } = await chrome.storage.local.get("paused");
    respond({
      status: connection.status(),
      browser: product(navigator.userAgent),
      install: policy?.install ?? "unknown",
      run: bridge.state(),
      policy,
      paused: paused === true,
    });
  })();
  return true;
});

// Any wake-up of the worker is a chance to be connected.
connection.open();

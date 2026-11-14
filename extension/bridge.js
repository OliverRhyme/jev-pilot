// The extension's side of the bridge: one connection to the jev-pilot host,
// the run it serves, and every chrome.* call made for it. `chrome` is passed
// in, so the same code runs in the service worker and in the tests.

import * as protocol from "./protocol.js";
import { leftAllowedSites, refusal, requestRefusal } from "./policy.js";

/** The tab group the run's tabs go in, and owner's tabs it may use. */
export const GROUP = "jev-pilot";

/** How long a tab with no opener waits for Chrome to say what opened it. */
const OPENER_WAIT_MS = 500;

/** The debugger protocol version the extension asks Chrome for. */
const DEBUGGER_VERSION = "1.3";

/** A failure in Chrome, as the run is told it. */
const chromeFailure = (code, error) => refusal(code, String(error?.message ?? error));

/**
 * The bridge for one connection. `post` sends a message to the host,
 * `loadPolicy` reads the admin-owned policy file, and `scripts` are the
 * engine's page scripts.
 */
export function createBridge({ chrome, post, loadPolicy, scripts, userAgent, setTimer = setTimeout }) {
  const version = chrome.runtime.getManifest().version;
  /** The tabs the run opened, and those they opened. */
  const owned = new Set();
  /** The tabs the run holds with the debugger. */
  const held = new Set();
  /** Tabs opened with no opener yet, waiting to hear what opened them. */
  const unclaimed = new Map();
  let running = false;
  /** The run being served: its goal and its sites. */
  let run = { goal: "", sites: [] };

  const paused = async () => (await chrome.storage.local.get("paused")).paused === true;

  const say = async (type) =>
    post(protocol.hello({ version, userAgent, paused: await paused(), policy: await loadPolicy() }, type));

  const targetOf = async (tabId) =>
    (await chrome.debugger.getTargets()).find((t) => t.tabId === tabId && t.type === "page")?.id;

  const groups = async () => new Set((await chrome.tabGroups.query({ title: GROUP })).map((g) => g.id));

  /** Let go of every tab the run holds. The extension never closes a tab. */
  const releaseAll = async () => {
    const tabs = [...held];
    held.clear();
    owned.clear();
    running = false;
    run = { goal: "", sites: [] };
    for (const tabId of tabs) {
      await chrome.debugger.detach({ tabId }).catch(() => {});
    }
  };

  /** Put a tab the run opened in the jev-pilot group of its window. */
  const groupTab = async (tab) => {
    const [existing] = await chrome.tabGroups.query({ title: GROUP, windowId: tab.windowId });
    if (existing) {
      await chrome.tabs.group({ tabIds: [tab.id], groupId: existing.id });
      return;
    }
    const groupId = await chrome.tabs.group({ tabIds: [tab.id], createProperties: { windowId: tab.windowId } });
    await chrome.tabGroups.update(groupId, { title: GROUP });
  };

  /** The last focused normal window, or a new one when there is none. */
  const window = async () => {
    try {
      return await chrome.windows.getLastFocused({ windowTypes: ["normal"] });
    } catch {
      return chrome.windows.create({ focused: true });
    }
  };

  /** Carry out a request the policy allowed. */
  const carryOut = async (request, tab) => {
    switch (request.op) {
      case "tabs": {
        const tabs = await chrome.tabs.query({});
        const targets = await chrome.debugger.getTargets();
        const listed = [];
        for (const t of tabs) {
          const target = targets.find((g) => g.tabId === t.id && g.type === "page")?.id;
          if (target === undefined) {
            continue;
          }
          const entry = { tab: t.id, target, url: t.url, title: t.title };
          if (t.openerTabId !== undefined) {
            entry.opener = t.openerTabId;
          }
          listed.push(entry);
        }
        return { tabs: listed };
      }
      case "create": {
        const into = await window();
        const created = await chrome.tabs.create({ windowId: into.id, url: request.url, active: true });
        owned.add(created.id);
        await groupTab(created);
        return { tab: created.id, target: await targetOf(created.id) };
      }
      case "attach":
        try {
          await chrome.debugger.attach({ tabId: request.tab }, DEBUGGER_VERSION);
        } catch (error) {
          throw chromeFailure("attach_failed", error);
        }
        held.add(request.tab);
        return {};
      case "detach":
        if (held.delete(request.tab)) {
          await chrome.debugger.detach({ tabId: request.tab }).catch(() => {});
        }
        return {};
      case "activate":
        await chrome.tabs.update(tab.id, { active: true });
        await chrome.windows.update(tab.windowId, { focused: true });
        return {};
      case "send": {
        const debuggee =
          request.session === undefined ? { tabId: request.tab } : { tabId: request.tab, sessionId: request.session };
        try {
          return await chrome.debugger.sendCommand(debuggee, request.method, request.params ?? {});
        } catch (error) {
          throw chromeFailure("chrome", error);
        }
      }
      default:
        return {};
    }
  };

  /** Answer one request from the run. */
  const answer = async (request) => {
    let tab;
    if (request.op === "attach" || request.op === "activate") {
      try {
        tab = await chrome.tabs.get(request.tab);
      } catch (error) {
        post(protocol.refused(request.id, chromeFailure("no_tab", error)));
        return;
      }
    }
    if (request.op === "send" && !held.has(request.tab)) {
      post(protocol.refused(request.id, refusal("no_tab", `the run does not hold tab ${request.tab}`)));
      return;
    }
    const refused = requestRefusal(request, {
      policy: await loadPolicy(),
      paused: await paused(),
      scripts,
      run: { owned, groups: await groups() },
      tab,
    });
    if (refused) {
      post(protocol.refused(request.id, refused));
      return;
    }
    try {
      const result = await carryOut(request, tab);
      post(protocol.reply(request.id, result));
    } catch (failure) {
      // A command cut short by its tab being let go of has no answer: the
      // run has already been told the tab is gone.
      if (request.op === "send" && !held.has(request.tab)) {
        return;
      }
      post(protocol.refused(request.id, failure?.code ? failure : chromeFailure("chrome", failure)));
    }
  };

  /** Tell the run about a tab opened while it is connected. */
  const report = async (tab, opener) => {
    if (opener !== undefined && (owned.has(opener) || held.has(opener))) {
      owned.add(tab.id);
    }
    post(
      protocol.tabCreated({
        tab: tab.id,
        target: await targetOf(tab.id),
        opener,
        url: tab.pendingUrl || tab.url || "",
        title: tab.title ?? "",
      }),
    );
  };

  return {
    /** The run being served and the tabs it holds, for the popup. */
    state: () => ({ running, goal: run.goal, sites: run.sites, tabs: [...held] }),

    /** Say hello to the host, on connecting. */
    connect: () => say("hello"),

    /** A message from the host. */
    async receive(message) {
      const read = protocol.read(message);
      if (!read) {
        return;
      }
      switch (read.type) {
        case "request":
          return answer(read);
        case "run_started": {
          await releaseAll();
          running = true;
          run = { goal: read.goal, sites: read.sites };
          const where = read.sites.length > 0 ? read.sites.join(", ") : "any site";
          const goal = read.goal.length > 80 ? `${read.goal.slice(0, 79)}…` : read.goal;
          await chrome.notifications.create(`jev-pilot-run-${read.pid}`, {
            type: "basic",
            iconUrl: chrome.runtime.getURL("icons/icon128.png"),
            title: "jev-pilot",
            message: `jev-pilot is driving ${where}: ${goal}`,
          });
          return;
        }
        case "release":
          return releaseAll();
        case "reload": {
          const { reloaded } = await chrome.storage.session.get("reloaded");
          if (reloaded !== version) {
            await chrome.storage.session.set({ reloaded: version });
            chrome.runtime.reload();
          }
          return;
        }
        default:
      }
    },

    /** A DevTools event from an attached tab. */
    async debuggerEvent(source, method, params) {
      if (!held.has(source.tabId)) {
        return;
      }
      const frame = params?.frame;
      if (method === "Page.frameNavigated" && frame && frame.parentId === undefined) {
        const policy = await loadPolicy();
        if (leftAllowedSites(frame.url, policy)) {
          held.delete(source.tabId);
          await chrome.debugger.detach({ tabId: source.tabId }).catch(() => {});
          post(protocol.detached(source.tabId, "left_allowed_sites", frame.url));
          return;
        }
      }
      post(protocol.cdp(source.tabId, source.sessionId, method, params));
    },

    /** Chrome let go of an attached tab. */
    detachedByChrome(source, reason) {
      if (held.delete(source.tabId)) {
        post(protocol.detached(source.tabId, reason));
      }
    },

    /** A tab was opened. */
    async tabCreated(tab) {
      if (!running) {
        return;
      }
      if (tab.openerTabId !== undefined) {
        await report(tab, tab.openerTabId);
        return;
      }
      unclaimed.set(tab.id, tab);
      setTimer(() => {
        if (unclaimed.delete(tab.id)) {
          report(tab, undefined);
        }
      }, OPENER_WAIT_MS);
    },

    /** Chrome says which tab opened a new one. */
    async navigationTarget({ sourceTabId, tabId }) {
      const tab = unclaimed.get(tabId);
      if (tab && unclaimed.delete(tabId)) {
        await report(tab, sourceTabId);
      }
    },

    /** A tab was closed. */
    tabRemoved(tabId) {
      owned.delete(tabId);
      const wasHeld = held.delete(tabId);
      if (running || wasHeld) {
        post(protocol.tabRemoved(tabId));
      }
    },

    /** The pause switch, or another stored setting, changed. */
    async storageChanged(changes, area) {
      if (area === "local" && "paused" in changes) {
        await say("status");
      }
    },

    /** The connection to the host was lost: let go of everything. */
    portLost: () => releaseAll(),
  };
}

/** Send `chrome`'s events to `bridge`. */
export function listen(chrome, bridge) {
  chrome.debugger.onEvent.addListener((source, method, params) => bridge.debuggerEvent(source, method, params));
  chrome.debugger.onDetach.addListener((source, reason) => bridge.detachedByChrome(source, reason));
  chrome.tabs.onCreated.addListener((tab) => bridge.tabCreated(tab));
  chrome.tabs.onRemoved.addListener((tabId) => bridge.tabRemoved(tabId));
  chrome.webNavigation.onCreatedNavigationTarget.addListener((details) => bridge.navigationTarget(details));
  chrome.storage.onChanged.addListener((changes, area) => bridge.storageChanged(changes, area));
}

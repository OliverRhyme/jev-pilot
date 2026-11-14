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

/**
 * What Chrome says when a managed browser's policy forbids debugging a
 * page: `runtime_blocked_hosts` in ExtensionSettings, or a screenshot
 * restriction, which since Chrome 155 can also stop an attach.
 */
const POLICY_FORBIDS = [
  "Host access is restricted by policy.",
  "This page cannot be scripted due to an ExtensionsSettings policy.",
  "Screenshot capture is restricted by policy.",
  "Screenshot capture is restricted on this target.",
];

/** The origin of `url`, or `url` itself when it has none. */
const originOf = (url) => {
  try {
    return new URL(url).origin;
  } catch {
    return url;
  }
};

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
  /**
   * For a held tab whose page has just changed, the check of where it went.
   * Set before anything is awaited, so that no command or event for the tab
   * passes until the check has said whether the run may still have it.
   */
  const checks = new Map();
  /** For each held tab, its events not yet passed on, in Chrome's order. */
  const queues = new Map();
  let running = false;
  /** The run being served: its goal and its sites. */
  let run = { goal: "", sites: [] };

  /** The policy file, or `null` when it cannot be read, which refuses. */
  const policyNow = () => loadPolicy().catch(() => null);

  const paused = async () => (await chrome.storage.local.get("paused")).paused === true;

  const say = async (type) =>
    post(protocol.hello({ version, userAgent, paused: await paused(), policy: await policyNow() }, type));

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

  /**
   * Check that a held tab arriving at `url` is still on the allowed sites,
   * and let go of it, telling the run, when it is not. Checks of one tab run
   * in turn, and whatever waits on the tab waits for the last of them.
   */
  const checkArrival = (tabId, url, policy) => {
    const check = (checks.get(tabId) ?? Promise.resolve()).then(async () => {
      if (!held.has(tabId) || !leftAllowedSites(url, policy ?? (await policyNow()))) {
        return;
      }
      if (held.delete(tabId)) {
        await chrome.debugger.detach({ tabId }).catch(() => {});
        post(protocol.detached(tabId, "left_allowed_sites", url));
      }
    });
    checks.set(tabId, check);
    check.finally(() => {
      if (checks.get(tabId) === check) {
        checks.delete(tabId);
      }
    });
    return check;
  };

  /** Wait for any check of where `tabId` went. */
  const settled = async (tabId) => {
    while (checks.has(tabId)) {
      await checks.get(tabId).catch(() => {});
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
          if (POLICY_FORBIDS.includes(String(error?.message ?? error))) {
            throw refusal(
              "attach_failed",
              `this Chrome's policy does not let extensions debug ${originOf(tab.url)}`,
            );
          }
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
      default:
        return {};
    }
  };

  /**
   * Check where a held tab is now, whatever DevTools has or has not said
   * yet, and wait for every check of where it went.
   */
  const whereNow = async (tabId, policy) => {
    if (held.has(tabId)) {
      const now = await chrome.tabs.get(tabId).catch(() => undefined);
      if (now?.url) {
        checkArrival(tabId, now.url, policy);
      }
    }
    await settled(tabId);
  };

  /**
   * Send a command the policy allowed to a held tab. The last check that
   * the run still holds the tab comes after every check of where it went,
   * with nothing awaited between it and the command; the reply is passed on
   * only if the run still holds the tab once the command is done.
   */
  const send = async (request, policy) => {
    const tabId = request.tab;
    await whereNow(tabId, policy);
    if (!held.has(tabId)) {
      post(protocol.refused(request.id, refusal("no_tab", `the run does not hold tab ${tabId}`)));
      return;
    }
    const debuggee = request.session === undefined ? { tabId } : { tabId, sessionId: request.session };
    let result;
    try {
      result = await chrome.debugger.sendCommand(debuggee, request.method, request.params ?? {});
    } catch (error) {
      await settled(tabId);
      // A command cut short by its tab being let go of has no answer: the
      // run has already been told the tab is gone.
      if (held.has(tabId)) {
        post(protocol.refused(request.id, chromeFailure("chrome", error)));
      }
      return;
    }
    await whereNow(tabId, policy);
    if (!held.has(tabId)) {
      return;
    }
    const passed = request.method === "DOMSnapshot.captureSnapshot" ? protocol.withoutPasswords(result) : result;
    post(protocol.reply(request.id, passed));
  };

  /** The address of the history entry a tab would go back to, if any. */
  const historyUrl = async (request) => {
    if (request.method !== "Page.navigateToHistoryEntry") {
      return undefined;
    }
    try {
      const history = await chrome.debugger.sendCommand({ tabId: request.tab }, "Page.getNavigationHistory", {});
      return history?.entries?.find((entry) => entry.id === request.params?.entryId)?.url;
    } catch {
      return undefined;
    }
  };

  /** Answer one request from the run, always: a failure is a coded refusal. */
  const answer = async (request) => {
    try {
      await answerOrThrow(request);
    } catch (failure) {
      post(protocol.refused(request.id, failure?.code ? failure : chromeFailure("chrome", failure)));
    }
  };

  const answerOrThrow = async (request) => {
    // Read once, and used for every check of this request.
    const policy = await policyNow();
    let tab;
    if (request.op === "attach" || request.op === "activate") {
      try {
        tab = await chrome.tabs.get(request.tab);
      } catch (error) {
        post(protocol.refused(request.id, chromeFailure("no_tab", error)));
        return;
      }
    }
    if (request.op === "send") {
      await settled(request.tab);
      if (!held.has(request.tab)) {
        post(protocol.refused(request.id, refusal("no_tab", `the run does not hold tab ${request.tab}`)));
        return;
      }
    }
    const refused = requestRefusal(request, {
      historyUrl: request.op === "send" ? await historyUrl(request) : undefined,
      policy,
      paused: await paused(),
      scripts,
      run: { owned, groups: await groups() },
      tab,
    });
    if (refused) {
      post(protocol.refused(request.id, refused));
      return;
    }
    if (request.op === "send") {
      await send(request, policy);
      return;
    }
    try {
      const result = await carryOut(request, tab);
      post(protocol.reply(request.id, result));
    } catch (failure) {
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

    /**
     * Let go of tabs an earlier service worker left attached: its run's
     * state went with it, so nothing would ever let go of them.
     */
    async forgetEarlierRun() {
      for (const target of await chrome.debugger.getTargets()) {
        if (target.attached && target.tabId !== undefined && !held.has(target.tabId)) {
          await chrome.debugger.detach({ tabId: target.tabId }).catch(() => {});
        }
      }
    },

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
          // Kept in local storage, which a reload leaves alone: if Chrome
          // comes back with this same version, it is not reloaded again.
          const { reloadedFrom } = await chrome.storage.local.get("reloadedFrom");
          if (reloadedFrom !== version) {
            await chrome.storage.local.set({ reloadedFrom: version });
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
        checkArrival(source.tabId, frame.url);
      }
      // Nothing from a tab passes while where it went is being checked, and
      // its events pass in the order Chrome sent them.
      const tabId = source.tabId;
      const passed = (queues.get(tabId) ?? Promise.resolve())
        .then(() => settled(tabId))
        .then(() => {
          if (held.has(tabId)) {
            post(protocol.cdp(tabId, source.sessionId, method, params));
          }
        });
      queues.set(tabId, passed);
      passed.finally(() => {
        if (queues.get(tabId) === passed) {
          queues.delete(tabId);
        }
      });
      await passed;
    },

    /** A tab's page changed, as Chrome's navigation events say it. */
    async committed({ tabId, frameId, url }) {
      if (frameId === 0 && held.has(tabId)) {
        await checkArrival(tabId, url);
      }
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
  chrome.webNavigation.onCommitted.addListener((details) => bridge.committed(details));
  chrome.storage.onChanged.addListener((changes, area) => bridge.storageChanged(changes, area));
}

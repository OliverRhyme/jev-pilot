// A stand-in for the parts of `chrome` the extension uses. Calls are
// recorded, and each can be told how to answer, so a test decides what
// Chrome does and checks what the extension makes of it.

export function fakeChrome({ version = "0.1.3", paused = false } = {}) {
  const listeners = new Map();
  const event = (name) => ({
    addListener(listener) {
      listeners.set(name, [...(listeners.get(name) ?? []), listener]);
    },
  });
  const calls = [];
  const state = {
    tabs: new Map(),
    targets: new Map(),
    groups: [{ id: 31, title: "jev-pilot", windowId: 1 }],
    nextTab: 100,
    storage: { paused },
    session: {},
    /** How the next call of each kind answers, set by a test. */
    answers: new Map(),
  };
  const answer = async (kind, otherwise) => {
    const queued = state.answers.get(kind);
    if (queued && queued.length > 0) {
      return queued.shift()();
    }
    return otherwise();
  };
  const tabOf = (id) => {
    const tab = state.tabs.get(id);
    if (!tab) {
      throw new Error(`No tab with id: ${id}.`);
    }
    return tab;
  };

  /** A native messaging port, as connectNative gives one. */
  const port = () => {
    const portListeners = { message: [], disconnect: [] };
    const made = {
      sent: [],
      disconnected: false,
      postMessage: (message) => made.sent.push(message),
      disconnect: () => {
        made.disconnected = true;
      },
      onMessage: { addListener: (fn) => portListeners.message.push(fn) },
      onDisconnect: { addListener: (fn) => portListeners.disconnect.push(fn) },
      /** The host sends `message`. */
      receive: async (message) => {
        for (const fn of portListeners.message) {
          await fn(message);
        }
      },
      /** The host goes, with Chrome's reason in `lastError`. */
      drop: async (why) => {
        chrome.runtime.lastError = why === undefined ? undefined : { message: why };
        for (const fn of portListeners.disconnect) {
          await fn(made);
        }
        chrome.runtime.lastError = undefined;
      },
    };
    state.ports.push(made);
    return made;
  };
  state.ports = [];

  const chrome = {
    runtime: {
      getManifest: () => ({ version }),
      getURL: (path) => `chrome-extension://eckgjcpdiehapkphgcflmcikippoegcp/${path}`,
      reload: () => calls.push(["runtime.reload"]),
      connectNative: (name) => {
        calls.push(["runtime.connectNative", name]);
        return port();
      },
      lastError: undefined,
      onStartup: event("runtime.onStartup"),
      onInstalled: event("runtime.onInstalled"),
      onMessage: event("runtime.onMessage"),
    },
    tabs: {
      query: async (query) => {
        calls.push(["tabs.query", query]);
        return answer("tabs.query", () => [...state.tabs.values()]);
      },
      get: async (id) => {
        calls.push(["tabs.get", id]);
        return answer("tabs.get", () => tabOf(id));
      },
      create: async (properties) => {
        calls.push(["tabs.create", properties]);
        return answer("tabs.create", () => {
          const tab = { id: state.nextTab++, url: properties.url, title: "", groupId: -1, windowId: properties.windowId ?? 1 };
          state.tabs.set(tab.id, tab);
          return tab;
        });
      },
      update: async (id, properties) => {
        calls.push(["tabs.update", id, properties]);
        return tabOf(id);
      },
      group: async (options) => {
        calls.push(["tabs.group", options]);
        const groupId = options.groupId ?? 31;
        for (const id of options.tabIds) {
          if (state.tabs.has(id)) {
            state.tabs.get(id).groupId = groupId;
          }
        }
        return groupId;
      },
      onCreated: event("tabs.onCreated"),
      onRemoved: event("tabs.onRemoved"),
    },
    tabGroups: {
      query: async (query) => {
        calls.push(["tabGroups.query", query]);
        return state.groups.filter(
          (g) => g.title === query.title && (query.windowId === undefined || g.windowId === query.windowId),
        );
      },
      update: async (id, properties) => {
        calls.push(["tabGroups.update", id, properties]);
        return { id, ...properties };
      },
    },
    windows: {
      getLastFocused: async (options) => {
        calls.push(["windows.getLastFocused", options]);
        return answer("windows.getLastFocused", () => ({ id: 1, type: "normal" }));
      },
      create: async (properties) => {
        calls.push(["windows.create", properties]);
        return { id: 2, type: "normal" };
      },
      update: async (id, properties) => {
        calls.push(["windows.update", id, properties]);
        return { id };
      },
    },
    debugger: {
      attach: async (debuggee, protocolVersion) => {
        calls.push(["debugger.attach", debuggee, protocolVersion]);
        return answer("debugger.attach", () => undefined);
      },
      detach: async (debuggee) => {
        calls.push(["debugger.detach", debuggee]);
        return answer("debugger.detach", () => undefined);
      },
      sendCommand: async (debuggee, method, params) => {
        calls.push(["debugger.sendCommand", debuggee, method, params]);
        return answer("debugger.sendCommand", () => ({}));
      },
      getTargets: async () => {
        calls.push(["debugger.getTargets"]);
        return [...state.targets].map(([tabId, id]) => ({ id, tabId, type: "page", attached: false }));
      },
      onEvent: event("debugger.onEvent"),
      onDetach: event("debugger.onDetach"),
    },
    webNavigation: {
      onCreatedNavigationTarget: event("webNavigation.onCreatedNavigationTarget"),
    },
    storage: {
      local: {
        get: async (key) => ({ [key]: state.storage[key] }),
        set: async (values) => Object.assign(state.storage, values),
      },
      session: {
        get: async (key) => ({ [key]: state.session[key] }),
        set: async (values) => {
          calls.push(["storage.session.set", values]);
          Object.assign(state.session, values);
        },
      },
      onChanged: event("storage.onChanged"),
    },
    notifications: {
      create: async (id, options) => {
        calls.push(["notifications.create", id, options]);
        return id;
      },
    },
    alarms: {
      create: (name, info) => calls.push(["alarms.create", name, info]),
      onAlarm: event("alarms.onAlarm"),
    },
    action: {
      setBadgeText: async (details) => calls.push(["action.setBadgeText", details]),
      setBadgeBackgroundColor: async (details) => calls.push(["action.setBadgeBackgroundColor", details]),
    },
  };

  /** Fire a `chrome` event, as Chrome would. */
  const emit = async (name, ...args) => {
    for (const listener of listeners.get(name) ?? []) {
      await listener(...args);
    }
  };

  /** Have the next call of `kind` answer with `fn`. */
  const when = (kind, fn) => {
    state.answers.set(kind, [...(state.answers.get(kind) ?? []), fn]);
  };

  return { chrome, emit, when, calls, state, listeners };
}

/** Let every pending promise settle. */
export async function settle() {
  for (let i = 0; i < 20; i += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

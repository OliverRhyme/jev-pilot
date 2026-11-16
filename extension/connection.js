// The extension's connection to the jev-pilot host, which Chrome starts on
// connectNative. An open native port keeps the service worker alive; when the
// host goes, the extension lets go of the run's tabs and tries again.

/** The name Chrome looks the host up by. */
export const HOST = "dev.jevpilot.bridge";

/** The waits before each try after a lost connection; then every 30 s. */
const QUICK_RETRIES_MS = [1000, 2000, 5000];
const SLOW_RETRY_MS = 30_000;

/** How long a connection must have lasted to start the waits over. */
const LASTED_MS = 30_000;

/** The backstop alarm, should a timer be lost with the service worker. */
const ALARM = "jev-pilot-reconnect";

/** How long to wait before try number `attempt` (from 0). */
export function retryDelay(attempt) {
  return attempt < QUICK_RETRIES_MS.length ? QUICK_RETRIES_MS[attempt] : SLOW_RETRY_MS;
}

/**
 * The connection to the host, for `bridge`. `status()` is `connecting`,
 * `connected`, `absent` (the host could not be reached or went) or
 * `elsewhere` (another browser already serves jev-pilot).
 */
export function createConnection({ chrome, bridge, setTimer = setTimeout, now = Date.now }) {
  let port = null;
  let status = "connecting";
  let attempt = 0;
  let openedAt = 0;
  let elsewhere = false;

  const mark = async (text) => {
    await chrome.action.setBadgeText({ text });
    if (text) {
      await chrome.action.setBadgeBackgroundColor({ color: "#b3261e" });
    }
  };

  const lost = async () => {
    port = null;
    await bridge.portLost();
    if (now() - openedAt >= LASTED_MS) {
      attempt = 0;
    }
    status = elsewhere ? "elsewhere" : "absent";
    await mark("!");
    const wait = elsewhere ? SLOW_RETRY_MS : retryDelay(attempt);
    attempt += 1;
    setTimer(() => open(), wait);
  };

  const open = async () => {
    if (port) {
      return;
    }
    elsewhere = false;
    status = "connecting";
    openedAt = now();
    port = chrome.runtime.connectNative(HOST);
    port.onMessage.addListener(async (message) => {
      status = "connected";
      if (message?.type === "elsewhere") {
        elsewhere = true;
        return;
      }
      await bridge.receive(message);
    });
    const opened = port;
    port.onDisconnect.addListener(() => lost());
    await mark("");
    await bridge.connect();
    // Hello is on its way; until the port is lost, the bridge is up.
    if (port === opened && status === "connecting") {
      status = "connected";
    }
  };

  chrome.alarms.create(ALARM, { periodInMinutes: 1 });
  chrome.alarms.onAlarm.addListener(async (alarm) => {
    if (alarm.name === ALARM && !port) {
      await open();
    }
  });

  return {
    open,
    /** Send a message to the host. */
    post(message) {
      port?.postMessage(message);
    },
    status: () => status,
  };
}

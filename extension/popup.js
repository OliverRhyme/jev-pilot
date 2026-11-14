// The popup: the bridge's state, the run, and the policy, read-only; and the
// pause switch, which is the owner's own and not a security control.

const STATUS = {
  connected: "connected to jev-pilot",
  connecting: "connecting…",
  absent: "jev-pilot is not installed, or not running: run the install",
  elsewhere: "another browser is already serving jev-pilot",
};

const INSTALL = {
  protected: "protected (installed by an administrator)",
  development: "development install: not protected against other programs running as you",
};

const show = (id, text, warn = false) => {
  const element = document.getElementById(id);
  element.textContent = text;
  element.classList.toggle("warn", warn);
};

const state = await chrome.runtime.sendMessage({ type: "state" });
show("status", STATUS[state.status] ?? state.status, state.status !== "connected");
show("browser", state.browser);
show("install", INSTALL[state.install] ?? "unknown", state.install !== "protected");
show(
  "run",
  state.run.running
    ? `${state.run.goal} (${state.run.tabs.length} tab${state.run.tabs.length === 1 ? "" : "s"} held)`
    : "none",
);
show("sites", state.policy ? (state.policy.sites.length > 0 ? state.policy.sites.join(", ") : "any site") : "no policy");
show(
  "owner-tabs",
  state.policy?.owner_tabs === "never" ? "never" : "only those in the jev-pilot tab group",
);

const paused = document.getElementById("paused");
paused.checked = state.paused;
paused.addEventListener("change", () => chrome.storage.local.set({ paused: paused.checked }));

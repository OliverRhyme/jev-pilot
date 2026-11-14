// The popup: the bridge's state, the run, and the policy, read-only; and the
// pause switch, which is the owner's own and not a security control.

import { describe } from "./popup-view.js";

const state = await chrome.runtime.sendMessage({ type: "state" });
const lines = describe(state);
for (const [id, key] of [
  ["status", "status"],
  ["browser", "browser"],
  ["install", "install"],
  ["run", "run"],
  ["sites", "sites"],
  ["owner-tabs", "ownerTabs"],
]) {
  const element = document.getElementById(id);
  element.textContent = lines[key].text;
  element.classList.toggle("warn", lines[key].warn);
}

const paused = document.getElementById("paused");
paused.checked = state.paused;
paused.addEventListener("change", () => chrome.storage.local.set({ paused: paused.checked }));

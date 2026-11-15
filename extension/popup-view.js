// What the popup says, from the state the service worker reports: pure, so
// that each line is tested without a browser.

const STATUS = {
  connected: "connected to jev-pilot",
  connecting: "connecting…",
  absent: "jev-pilot is not installed, or not running: run the install",
  elsewhere: "another browser is already serving jev-pilot",
};

/** Each line of the popup: its text, and whether it is a warning. */
export function describe(state) {
  const held = state.run.tabs.length;
  return {
    status: { text: STATUS[state.status] ?? state.status, warn: state.status !== "connected" },
    browser: { text: state.browser, warn: false },
    run: {
      text: state.run.running ? `${state.run.goal} (${held} tab${held === 1 ? "" : "s"} held)` : "none",
      warn: false,
    },
    sites: {
      text: state.policy ? (state.policy.sites.length > 0 ? state.policy.sites.join(", ") : "any site") : "no policy",
      warn: false,
    },
    ownerTabs: {
      text: state.policy?.owner_tabs === "never" ? "never" : "only those in the jev-pilot tab group",
      warn: false,
    },
  };
}

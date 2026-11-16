// What the popup says for each state of the bridge and the run.

import { test } from "node:test";
import assert from "node:assert/strict";
import { describe } from "../popup-view.js";

const state = (overrides = {}) => ({
  status: "connected",
  browser: "Chrome/158.0.7400.12",
  run: { running: false, goal: "", sites: [], tabs: [] },
  policy: { sites: [], owner_tabs: "group" },
  paused: false,
  ...overrides,
});

test("a second browser's popup says another browser is already serving jev-pilot", () => {
  assert.deepEqual(describe(state({ status: "elsewhere" })).status, {
    text: "another browser is already serving jev-pilot",
    warn: true,
  });
});

test("the popup says how the bridge, the run and the policy stand", () => {
  const said = describe(
    state({
      run: { running: true, goal: "Star the repository", sites: [], tabs: [7] },
      policy: { sites: ["https://github.com"], owner_tabs: "never" },
    }),
  );
  assert.deepEqual(said.status, { text: "connected to jev-pilot", warn: false });
  assert.equal(said.install, undefined, "there is one kind of install, so no line for it");
  assert.equal(said.run.text, "Star the repository (1 tab held)");
  assert.equal(said.sites.text, "https://github.com");
  assert.equal(said.ownerTabs.text, "never");
  assert.deepEqual(describe(state({ status: "absent", policy: null })).status, {
    text: "jev-pilot is not installed, or not running: run the install",
    warn: true,
  });
  assert.equal(describe(state({ policy: null })).sites.text, "no policy");
});

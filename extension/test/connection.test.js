// The connection to the jev-pilot host: connecting, losing it, and trying
// again with growing waits.

import { test } from "node:test";
import assert from "node:assert/strict";
import { createConnection, HOST, retryDelay } from "../connection.js";
import { fakeChrome, settle } from "./fake-chrome.js";

/** A connection over a fake chrome, with a bridge that records what it is told. */
function connected({ now = () => 0 } = {}) {
  const fake = fakeChrome();
  const timers = [];
  const told = [];
  const bridge = {
    connect: async () => told.push("connect"),
    receive: async (message) => told.push(message),
    portLost: async () => told.push("lost"),
  };
  const connection = createConnection({
    chrome: fake.chrome,
    bridge,
    setTimer: (fn, ms) => timers.push({ fn, ms }),
    now,
  });
  return { fake, timers, told, connection };
}

test("the waits between tries grow from 1 s to 2 s and 5 s, then stay at 30 s", () => {
  assert.deepEqual(
    [0, 1, 2, 3, 4, 9].map(retryDelay),
    [1000, 2000, 5000, 30000, 30000, 30000],
  );
});

test("connecting opens the host by name and says hello on it", async () => {
  const { fake, told, connection } = connected();
  await connection.open();
  assert.deepEqual(
    fake.calls.filter(([call]) => call === "runtime.connectNative"),
    [["runtime.connectNative", "dev.jevpilot.bridge"]],
  );
  assert.equal(HOST, "dev.jevpilot.bridge");
  assert.deepEqual(told, ["connect"]);
  await fake.state.ports[0].receive({ type: "release" });
  assert.deepEqual(told, ["connect", { type: "release" }]);
  connection.post({ type: "reply", id: 1, result: {} });
  assert.deepEqual(fake.state.ports[0].sent, [{ type: "reply", id: 1, result: {} }]);
});

test("losing the host lets go of the run's tabs, shows a mark, and tries again with growing waits", async () => {
  const { fake, told, connection, timers } = connected();
  await connection.open();
  await fake.state.ports[0].drop("Specified native messaging host not found.");
  assert.ok(told.includes("lost"));
  assert.ok(fake.calls.some(([call, d]) => call === "action.setBadgeText" && d.text === "!"));
  assert.equal(connection.status(), "absent");
  for (const wait of [1000, 2000, 5000, 30000]) {
    assert.equal(timers.at(-1).ms, wait);
    await timers.at(-1).fn();
    await fake.state.ports.at(-1).drop("Specified native messaging host not found.");
  }
  assert.equal(fake.calls.filter(([call]) => call === "runtime.connectNative").length, 5);
});

test("a connection that lasted starts the waits over", async () => {
  let clock = 0;
  const { fake, connection, timers } = connected({ now: () => clock });
  await connection.open();
  await fake.state.ports[0].drop();
  await timers.at(-1).fn();
  await fake.state.ports[1].drop();
  assert.equal(timers.at(-1).ms, 2000);
  await timers.at(-1).fn();
  clock += 60_000;
  await fake.state.ports[2].drop();
  assert.equal(timers.at(-1).ms, 1000, "a minute connected is a fresh start");
});

test("the one-minute alarm reconnects a connection that has gone, and leaves a live one alone", async () => {
  const { fake, connection } = connected();
  await connection.open();
  assert.ok(fake.calls.some(([call, name, info]) => call === "alarms.create" && info.periodInMinutes === 1));
  await fake.emit("alarms.onAlarm", { name: "jev-pilot-reconnect" });
  assert.equal(fake.state.ports.length, 1, "connected already");
  await fake.state.ports[0].drop();
  await fake.emit("alarms.onAlarm", { name: "jev-pilot-reconnect" });
  await settle();
  assert.equal(fake.state.ports.length, 2);
});

test("when another browser already serves jev-pilot, the extension says so, lets go, and tries again slowly", async () => {
  const { fake, told, connection, timers } = connected();
  await connection.open();
  await fake.state.ports[0].receive({ type: "elsewhere" });
  await fake.state.ports[0].drop();
  assert.equal(connection.status(), "elsewhere");
  assert.ok(told.includes("lost"));
  assert.equal(timers.at(-1).ms, 30000, "no quick retries against a browser that is serving");
  assert.ok(fake.calls.some(([call, d]) => call === "action.setBadgeText" && d.text === "!"));
});

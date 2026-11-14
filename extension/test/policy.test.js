// The extension's policy: what a run may ask of Chrome, decided from the
// request, the run's tabs and the admin-owned policy file alone.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as policy from "../policy.js";

// --- methods ---

test("only the DevTools methods the engine uses are passed on", () => {
  for (const method of ["Page.navigate", "DOM.getNodeForLocation", "Runtime.callFunctionOn"]) {
    assert.equal(policy.methodRefusal(method), null, method);
  }
  for (const method of [
    "Runtime.evaluate",
    "Network.getCookies",
    "Storage.getCookies",
    "Fetch.enable",
    "Page.setDownloadBehavior",
  ]) {
    assert.deepEqual(policy.methodRefusal(method), {
      code: "refused_method",
      message: `${method} is not one jev-pilot uses`,
    });
  }
});

// --- scripts ---

import { readFileSync } from "node:fs";

const SCRIPTS = JSON.parse(
  readFileSync(new URL("../scripts.json", import.meta.url), "utf8"),
);

test("a page script passes only when it is, byte for byte, one of the engine's", () => {
  const call = (functionDeclaration) => ({ functionDeclaration, arguments: [] });
  for (const script of SCRIPTS) {
    assert.equal(policy.scriptRefusal("Runtime.callFunctionOn", call(script), SCRIPTS), null);
  }
  const refused = {
    code: "refused_script",
    message: "that page script is not one of jev-pilot's own",
  };
  assert.deepEqual(
    policy.scriptRefusal("Runtime.callFunctionOn", call("function () { return document.cookie; }"), SCRIPTS),
    refused,
  );
  assert.deepEqual(
    policy.scriptRefusal("Runtime.callFunctionOn", call(SCRIPTS[0] + " "), SCRIPTS),
    refused,
    "a single added space is another script",
  );
  assert.deepEqual(policy.scriptRefusal("Runtime.callFunctionOn", {}, SCRIPTS), refused);
  assert.equal(policy.scriptRefusal("Page.navigate", { url: "https://example.test/" }, SCRIPTS), null);
});

// --- pages ---

test("only web pages and a blank page may be driven", () => {
  for (const url of ["https://example.test/a?b#c", "http://127.0.0.1:8080/", "about:blank"]) {
    assert.equal(policy.pageRefusal(url), null, url);
  }
  for (const url of [
    "chrome://settings",
    "chrome-extension://abcdefghijklmnopabcdefghijklmnop/popup.html",
    "devtools://devtools/bundled/inspector.html",
    "file:///etc/passwd",
    "view-source:https://example.test/",
    "about:settings",
    "data:text/html,<p>hi</p>",
    "javascript:alert(1)",
    "https://chromewebstore.google.com/detail/x",
    "https://chrome.google.com/webstore/detail/x",
    "not a url",
  ]) {
    assert.deepEqual(
      policy.pageRefusal(url),
      { code: "refused_page", message: `Chrome does not let extensions drive ${url}` },
      url,
    );
  }
});

// --- sites ---

const sites = (list) => ({ sites: list, owner_tabs: "group" });

test("an empty site list allows every web site", () => {
  assert.equal(policy.siteRefusal("https://anywhere.test/x", sites([])), null);
  assert.equal(policy.siteRefusal("http://127.0.0.1:52578/signup", sites([])), null);
});

test("a site is an origin: scheme, host and port all count", () => {
  const github = sites(["https://github.com"]);
  assert.equal(policy.siteRefusal("https://github.com/OliverRhyme/jev-pilot", github), null);
  for (const url of [
    "http://github.com/",
    "https://gist.github.com/",
    "https://github.com:8443/",
    "https://github.com.evil.test/",
  ]) {
    const origin = new URL(url).origin;
    assert.deepEqual(
      policy.siteRefusal(url, github),
      {
        code: "refused_site",
        message: `${origin} is not on jev-pilot's allowed sites (\`sudo jev-pilot extension policy sites add ${origin}\`)`,
      },
      url,
    );
  }
});

test("a leading *. allows the site's subdomains, not the site itself", () => {
  const subdomains = sites(["https://*.github.com"]);
  assert.equal(policy.siteRefusal("https://gist.github.com/x", subdomains), null);
  assert.equal(policy.siteRefusal("https://a.b.github.com/", subdomains), null);
  assert.notEqual(policy.siteRefusal("https://github.com/", subdomains), null);
  assert.notEqual(policy.siteRefusal("https://evilgithub.com/", subdomains), null);
  assert.notEqual(policy.siteRefusal("http://gist.github.com/", subdomains), null);
});

test("a blank page is no site, and always allowed", () => {
  assert.equal(policy.siteRefusal("about:blank", sites(["https://github.com"])), null);
});

// --- tabs ---

const run = { owned: new Set([7]), groups: new Set([31]) };

test("a run may use the tabs it opened, and the tabs those open", () => {
  const group = sites([]);
  assert.equal(policy.tabRefusal({ id: 7, groupId: -1 }, run, group), null);
  assert.equal(policy.tabRefusal({ id: 9, groupId: -1, openerTabId: 7 }, run, group), null);
});

test("an owner's tab is the run's only when the owner put it in the jev-pilot group", () => {
  const group = { sites: [], owner_tabs: "group" };
  assert.equal(policy.tabRefusal({ id: 4, groupId: 31 }, run, group), null);
  assert.deepEqual(policy.tabRefusal({ id: 4, groupId: 12 }, run, group), {
    code: "refused_tab",
    message: "put that tab in the 'jev-pilot' tab group first, so the run may use it",
  });
  assert.deepEqual(policy.tabRefusal({ id: 4, groupId: -1 }, run, group), {
    code: "refused_tab",
    message: "put that tab in the 'jev-pilot' tab group first, so the run may use it",
  });
});

test("with owner_tabs never, no tab of the owner's is the run's, grouped or not", () => {
  const never = { sites: [], owner_tabs: "never" };
  assert.deepEqual(policy.tabRefusal({ id: 4, groupId: 31 }, run, never), {
    code: "refused_tab",
    message: "this run may use only the tabs it opened (owner_tabs is never)",
  });
  assert.equal(policy.tabRefusal({ id: 7, groupId: -1 }, run, never), null);
});

// --- pause ---

test("a paused extension refuses, saying how to resume", () => {
  assert.equal(policy.pausedRefusal(false), null);
  assert.deepEqual(policy.pausedRefusal(true), {
    code: "paused",
    message: "jev-pilot is paused in Chrome: resume it from the extension's toolbar button",
  });
});

// --- requests ---

const ctx = (overrides = {}) => ({
  policy: { sites: ["https://example.test"], owner_tabs: "group" },
  paused: false,
  scripts: SCRIPTS,
  run,
  tab: { id: 7, groupId: -1, url: "https://example.test/" },
  ...overrides,
});
const codeOf = (request, context) => policy.requestRefusal(request, context)?.code ?? null;

test("creating a tab is checked for pause, the page and the site", () => {
  assert.equal(codeOf({ op: "create", url: "https://example.test/signup" }, ctx()), null);
  assert.equal(codeOf({ op: "create", url: "about:blank" }, ctx()), null);
  assert.equal(codeOf({ op: "create", url: "https://example.test/" }, ctx({ paused: true })), "paused");
  assert.equal(codeOf({ op: "create", url: "chrome://settings" }, ctx()), "refused_page");
  assert.equal(codeOf({ op: "create", url: "https://elsewhere.test/" }, ctx()), "refused_site");
});

test("attaching is checked for pause, the tab, its page and its site", () => {
  assert.equal(codeOf({ op: "attach", tab: 7 }, ctx()), null);
  assert.equal(codeOf({ op: "attach", tab: 7 }, ctx({ paused: true })), "paused");
  assert.equal(
    codeOf({ op: "attach", tab: 4 }, ctx({ tab: { id: 4, groupId: -1, url: "https://example.test/" } })),
    "refused_tab",
  );
  assert.equal(
    codeOf({ op: "attach", tab: 7 }, ctx({ tab: { id: 7, groupId: -1, url: "file:///etc/passwd" } })),
    "refused_page",
  );
  assert.equal(
    codeOf({ op: "attach", tab: 7 }, ctx({ tab: { id: 7, groupId: -1, url: "https://elsewhere.test/" } })),
    "refused_site",
  );
});

test("bringing a tab forward is checked for the tab only", () => {
  assert.equal(codeOf({ op: "activate", tab: 7 }, ctx({ paused: true })), null);
  assert.equal(
    codeOf({ op: "activate", tab: 4 }, ctx({ tab: { id: 4, groupId: -1, url: "https://example.test/" } })),
    "refused_tab",
  );
});

test("listing tabs and letting go of one are never refused", () => {
  assert.equal(codeOf({ op: "tabs" }, ctx({ paused: true })), null);
  assert.equal(codeOf({ op: "detach", tab: 7 }, ctx({ paused: true })), null);
});

test("a command is checked for pause, its method, its script, and where it navigates", () => {
  const send = (method, params = {}) => ({ op: "send", tab: 7, method, params });
  assert.equal(codeOf(send("Page.getFrameTree"), ctx()), null);
  assert.equal(codeOf(send("Page.getFrameTree"), ctx({ paused: true })), "paused");
  assert.equal(codeOf(send("Runtime.evaluate", { expression: "1" }), ctx()), "refused_method");
  assert.equal(
    codeOf(send("Runtime.callFunctionOn", { functionDeclaration: "function () {}" }), ctx()),
    "refused_script",
  );
  assert.equal(
    codeOf(send("Runtime.callFunctionOn", { functionDeclaration: SCRIPTS[3], executionContextId: 1 }), ctx()),
    null,
  );
  assert.equal(codeOf(send("Page.navigate", { url: "chrome://settings" }), ctx()), "refused_page");
  assert.equal(codeOf(send("Page.navigate", { url: "https://elsewhere.test/" }), ctx()), "refused_site");
  assert.equal(codeOf(send("Page.navigate", { url: "https://example.test/next" }), ctx()), null);
});

test("a held tab that arrives off the allowed sites is to be let go of", () => {
  const listed = { sites: ["https://example.test"], owner_tabs: "group" };
  assert.equal(policy.leftAllowedSites("https://elsewhere.test/", listed), true);
  assert.equal(policy.leftAllowedSites("https://example.test/next", listed), false);
  assert.equal(policy.leftAllowedSites("about:blank", listed), false);
  assert.equal(policy.leftAllowedSites("https://elsewhere.test/", { sites: [], owner_tabs: "group" }), false);
});

// --- the policy file's shape ---

test("a policy file not in the install's shape refuses everything that drives a page", () => {
  for (const broken of [
    null,
    "sites",
    [],
    {},
    { owner_tabs: "group" },
    { sites: null, owner_tabs: "group" },
    { sites: "https://example.test", owner_tabs: "group" },
    { sites: [7], owner_tabs: "group" },
    { sites: [] },
    { sites: [], owner_tabs: "always" },
  ]) {
    const context = ctx({ policy: broken, tab: { id: 7, groupId: 31, url: "https://example.test/" } });
    for (const request of [
      { op: "create", url: "https://example.test/" },
      { op: "attach", tab: 7 },
      { op: "activate", tab: 7 },
      { op: "send", tab: 7, method: "Page.enable" },
    ]) {
      const refused = policy.requestRefusal(request, context);
      assert.equal(refused?.code, "refused_site", `${JSON.stringify(broken)} ${request.op}`);
      assert.match(refused.message, /run the install again/);
    }
    for (const request of [{ op: "tabs" }, { op: "detach", tab: 7 }]) {
      assert.equal(codeOf(request, context), null, "listing tabs and letting go of one still work");
    }
    assert.equal(policy.leftAllowedSites("https://example.test/", broken), true, JSON.stringify(broken));
  }
  assert.equal(policy.policyProblem({ sites: [], owner_tabs: "never", install: "protected" }), null);
});

test("a request of a kind the run does not make is refused", () => {
  for (const op of ["close", "constructor", "toString", undefined]) {
    assert.equal(codeOf({ op, tab: 7 }, ctx()), "refused_method", String(op));
  }
});

test("going back is checked against the site of the page it goes back to", () => {
  const back = { op: "send", tab: 7, method: "Page.navigateToHistoryEntry", params: { entryId: 3 } };
  assert.equal(codeOf(back, ctx({ historyUrl: "https://example.test/before" })), null);
  assert.equal(codeOf(back, ctx({ historyUrl: "https://elsewhere.test/" })), "refused_site");
  assert.equal(codeOf(back, ctx({ historyUrl: "chrome://settings/" })), "refused_page");
  assert.equal(codeOf(back, ctx()), "refused_site", "an entry that cannot be found is refused");
});

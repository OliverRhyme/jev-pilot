// The engine's page scripts (scripts.json, which equals the engine's own),
// run against a few stand-in elements.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const SCRIPTS = JSON.parse(readFileSync(new URL("../scripts.json", import.meta.url), "utf8"));
const CONTAINS = SCRIPTS.find((s) => s.includes("this.contains"));

/** Run a page script on `self` with `args`, as Runtime.callFunctionOn does. */
const run = (script, self, ...args) => new Function(`return (${script})`)().apply(self, args);

/** An element with just what the scripts use. */
function element(tag, { parent = null, href, label, text = "" } = {}) {
  return {
    nodeType: 1,
    tagName: tag.toUpperCase(),
    parentElement: parent,
    href,
    innerText: text,
    textContent: text,
    getAttribute: (name) => (name === "aria-label" ? (label ?? null) : null),
    contains(other) {
      for (let node = other; node; node = node.parentElement) {
        if (node === this) {
          return true;
        }
      }
      return false;
    },
    closest(selector) {
      assert.equal(selector, "a[href]");
      for (let node = this; node; node = node.parentElement) {
        if (node.tagName === "A" && node.href) {
          return node;
        }
      }
      return null;
    },
  };
}

const VIDEO = "https://www.youtube.com/watch?v=ZUW1PjzE5vE";

test("a hit inside the row is the row", () => {
  const row = element("a", { href: VIDEO });
  const image = element("img", { parent: row });
  assert.equal(run(CONTAINS, row, image).inside, true);
});

test("a hit in an overlay link that goes where the row's link goes is the row", () => {
  // YouTube's hover preview: drawn over the thumbnail from outside its
  // result, with its video inside a link to the same video.
  const row = element("a", { href: VIDEO });
  const preview = element("a", { href: VIDEO });
  const player = element("div", { parent: element("div", { parent: preview }) });
  assert.equal(run(CONTAINS, row, player).inside, true);
});

test("a hit in a link that goes elsewhere covers the row, and is named", () => {
  const row = element("a", { href: VIDEO });
  const other = element("a", { href: "https://ads.example/", label: "Sponsored" });
  assert.deepEqual(run(CONTAINS, row, other), { inside: false, what: "a “Sponsored”" });
});

test("a row in no link is not matched by a hit in no link", () => {
  const row = element("button", { text: "Accept" });
  const banner = element("div", { text: "Cookies" });
  assert.deepEqual(run(CONTAINS, row, banner), { inside: false, what: "div “Cookies”" });
});

test("a row inside a link is matched by a hit in another link to the same place", () => {
  const link = element("a", { href: VIDEO });
  const row = element("span", { parent: link, text: "Na Para Bang" });
  const preview = element("a", { href: VIDEO });
  assert.equal(run(CONTAINS, row, element("video", { parent: preview })).inside, true);
});

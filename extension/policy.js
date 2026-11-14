// What a run may ask of Chrome. Every rule here is pure: it decides from the
// request, the run's tabs and the admin-owned policy file, and the service
// worker carries out only what passes.

/** The DevTools methods the engine uses, and so the only ones passed on. */
export const METHODS = [
  "Accessibility.getFullAXTree",
  "Accessibility.getPartialAXTree",
  "DOM.describeNode",
  "DOM.getBoxModel",
  "DOM.getNodeForLocation",
  "DOM.resolveNode",
  "DOM.scrollIntoViewIfNeeded",
  "DOMSnapshot.captureSnapshot",
  "Input.dispatchKeyEvent",
  "Input.dispatchMouseEvent",
  "Input.insertText",
  "Page.createIsolatedWorld",
  "Page.enable",
  "Page.getFrameTree",
  "Page.getLayoutMetrics",
  "Page.getNavigationHistory",
  "Page.handleJavaScriptDialog",
  "Page.navigate",
  "Page.navigateToHistoryEntry",
  "Runtime.callFunctionOn",
];

/** Why a request was not carried out. */
export function refusal(code, message) {
  return { code, message };
}

/** A DevTools method the engine does not use is refused. */
export function methodRefusal(method) {
  return METHODS.includes(method)
    ? null
    : refusal("refused_method", `${method} is not one jev-pilot uses`);
}

/**
 * A page script passes only when it is exactly one of the engine's own: the
 * engine sends nothing else, so anything else did not come from it.
 */
export function scriptRefusal(method, params, scripts) {
  if (method !== "Runtime.callFunctionOn") {
    return null;
  }
  const declaration = params?.functionDeclaration;
  return typeof declaration === "string" && scripts.includes(declaration)
    ? null
    : refusal("refused_script", "that page script is not one of jev-pilot's own");
}

/** The Web Store, which no extension may drive. */
const WEB_STORE = [
  { host: "chromewebstore.google.com", path: "/" },
  { host: "chrome.google.com", path: "/webstore" },
];

/**
 * Only web pages, and a blank page, may be driven: never Chrome's own pages,
 * another extension's, DevTools, a local file, page source or the Web Store.
 */
export function pageRefusal(url) {
  const refused = refusal("refused_page", `Chrome does not let extensions drive ${url}`);
  if (url === "about:blank") {
    return null;
  }
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return refused;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return refused;
  }
  const store = WEB_STORE.some(
    (s) => parsed.hostname === s.host && parsed.pathname.startsWith(s.path),
  );
  return store ? refused : null;
}

/**
 * Whether `url` is on the allowed sites. Each entry is an origin; a leading
 * `*.` in its host allows that host's subdomains, not the host itself. An
 * empty list allows every site, and a blank page is no site at all.
 */
export function siteAllowed(url, sites) {
  if (url === "about:blank" || sites.length === 0) {
    return true;
  }
  let target;
  try {
    target = new URL(url);
  } catch {
    return false;
  }
  return sites.some((entry) => {
    const wildcard = entry.match(/^([a-z][a-z0-9+.-]*:)\/\/\*\.(.+)$/i);
    if (!wildcard) {
      try {
        return new URL(entry).origin === target.origin;
      } catch {
        return false;
      }
    }
    const [, scheme, rest] = wildcard;
    let base;
    try {
      base = new URL(`${scheme}//${rest}`);
    } catch {
      return false;
    }
    return (
      target.protocol === base.protocol &&
      target.port === base.port &&
      target.hostname.endsWith(`.${base.hostname}`)
    );
  });
}

/**
 * What is wrong with a policy file, or `null` when it is in the shape the
 * install writes: an object whose `sites` is a list of origins and whose
 * `owner_tabs` is "group" or "never". Anything else is not guessed at.
 */
export function policyProblem(policy) {
  if (policy === null || typeof policy !== "object" || Array.isArray(policy)) {
    return "it is not a policy";
  }
  if (!Array.isArray(policy.sites) || !policy.sites.every((site) => typeof site === "string")) {
    return "its sites are not a list of origins";
  }
  if (policy.owner_tabs !== "group" && policy.owner_tabs !== "never") {
    return "its owner_tabs is neither group nor never";
  }
  return null;
}

/** A policy file that cannot be used refuses, rather than allowing more. */
function brokenPolicyRefusal(policy) {
  const why = policyProblem(policy);
  return why === null
    ? null
    : refusal(
        "refused_site",
        `jev-pilot's policy.json cannot be used (${why}): set it with \`sudo jev-pilot extension policy\`, or run the install again`,
      );
}

/** A site off the allowed list is refused, naming the command that adds it. */
export function siteRefusal(url, policy) {
  if (siteAllowed(url, policy.sites ?? [])) {
    return null;
  }
  let origin = url;
  try {
    origin = new URL(url).origin;
  } catch {
    // Named as given.
  }
  return refusal(
    "refused_site",
    `${origin} is not on jev-pilot's allowed sites (\`sudo jev-pilot extension policy sites add ${origin}\`)`,
  );
}

/**
 * A run may use the tabs it opened and the tabs those open, and an owner's
 * tab only when the owner has put it in the "jev-pilot" group and the policy
 * allows that (`owner_tabs: "group"`). `run.owned` holds the run's tab ids,
 * and `run.groups` the ids of the groups titled "jev-pilot".
 */
export function tabRefusal(tab, run, policy) {
  if (run.owned.has(tab.id) || (tab.openerTabId !== undefined && run.owned.has(tab.openerTabId))) {
    return null;
  }
  if (policy.owner_tabs === "never") {
    return refusal("refused_tab", "this run may use only the tabs it opened (owner_tabs is never)");
  }
  return run.groups.has(tab.groupId)
    ? null
    : refusal("refused_tab", "put that tab in the 'jev-pilot' tab group first, so the run may use it");
}

/** While the owner has paused the extension, nothing is driven. */
export function pausedRefusal(paused) {
  return paused
    ? refusal("paused", "jev-pilot is paused in Chrome: resume it from the extension's toolbar button")
    : null;
}

/** The first refusal among `checks`, each a function that may give one. */
function first(...checks) {
  for (const check of checks) {
    const refused = check();
    if (refused) {
      return refused;
    }
  }
  return null;
}

/**
 * Why a run's request may not be carried out, or `null` when it may.
 *
 * `context` holds the policy file (`policy`), whether the owner has paused
 * the extension (`paused`), the engine's scripts (`scripts`), the run's tabs
 * (`run`), and for a request about a tab, that tab as Chrome describes it
 * (`tab`). Pause stops anything that drives a page; listing tabs and letting
 * go of one are always allowed.
 */
export function requestRefusal(request, context) {
  const { policy, paused, scripts, run, tab, historyUrl } = context;
  if (!REQUESTS.includes(request.op)) {
    return refusal("refused_method", `${String(request.op)} is not a request jev-pilot makes`);
  }
  if (request.op === "tabs" || request.op === "detach") {
    return null;
  }
  const broken = brokenPolicyRefusal(policy);
  if (broken) {
    return broken;
  }
  switch (request.op) {
    case "create":
      return first(
        () => pausedRefusal(paused),
        () => pageRefusal(request.url),
        () => siteRefusal(request.url, policy),
      );
    case "attach":
      return first(
        () => pausedRefusal(paused),
        () => tabRefusal(tab, run, policy),
        () => pageRefusal(tab.url),
        () => siteRefusal(tab.url, policy),
      );
    case "activate":
      return tabRefusal(tab, run, policy);
    case "send": {
      const back = request.method === "Page.navigateToHistoryEntry";
      const navigating = request.method === "Page.navigate" ? request.params?.url : back ? historyUrl : undefined;
      return first(
        () => pausedRefusal(paused),
        () => methodRefusal(request.method),
        () => scriptRefusal(request.method, request.params, scripts),
        () =>
          back && typeof historyUrl !== "string"
            ? refusal("refused_site", "the page to go back to is not in the tab's history")
            : null,
        () => (navigating === undefined ? null : pageRefusal(navigating)),
        () => (navigating === undefined ? null : siteRefusal(navigating, policy)),
      );
    }
    default:
      return null;
  }
}

/** The requests a run makes. */
const REQUESTS = ["tabs", "create", "attach", "detach", "activate", "send"];

/** Whether a held tab that has arrived at `url` has left the allowed sites. */
export function leftAllowedSites(url, policy) {
  return policyProblem(policy) !== null || !siteAllowed(url, policy.sites);
}

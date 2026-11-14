// The bridge's messages, as the extension builds and reads them. The shapes
// are the run's (jev_pilot::browser::bridge::Message); Chrome's native
// messaging does the framing.

/** The version of the bridge protocol this extension speaks. */
export const PROTOCOL = 1;

/** The operations a run may ask for, and the fields each needs. */
const OPERATIONS = {
  tabs: [],
  create: ["url"],
  attach: ["tab"],
  detach: ["tab"],
  activate: ["tab"],
  send: ["tab", "method"],
};

/** The browser's product, as `Browser.getVersion` would name it. */
export function product(userAgent) {
  const edge = userAgent.match(/\bEdg\/[\d.]+/);
  if (edge) {
    return edge[0];
  }
  const chrome = userAgent.match(/\bChrome\/[\d.]+/);
  return chrome ? chrome[0] : "unknown";
}

/** What the extension says on connecting, and again when any of it changes. */
export function hello({ version, userAgent, paused, policy }, type = "hello") {
  return {
    type,
    protocol: PROTOCOL,
    extension_version: version,
    product: product(userAgent),
    paused,
    policy,
  };
}

/** A request carried out. */
export function reply(id, result) {
  return { type: "reply", id, result };
}

/** A request refused, or failed in Chrome. */
export function refused(id, { code, message }) {
  return { type: "reply", id, error: { code, message } };
}

/** A DevTools event from a held tab, or one of its child sessions. */
export function cdp(tab, session, method, params) {
  return session === undefined
    ? { type: "cdp", tab, method, params }
    : { type: "cdp", tab, session, method, params };
}

/** A tab opened while a run is connected. */
export function tabCreated({ tab, target, opener, url, title }) {
  const created = { type: "tab_created", tab, target };
  if (opener !== undefined) {
    created.opener = opener;
  }
  return { ...created, url, title };
}

/** A tab closed. */
export function tabRemoved(tab) {
  return { type: "tab_removed", tab };
}

/** A held tab let go of, why, and for a tab that left the sites, where it went. */
export function detached(tab, reason, url) {
  return url === undefined ? { type: "detached", tab, reason } : { type: "detached", tab, reason, url };
}

/**
 * A message from the host, when it is one the extension reads: a run's
 * request with its id and every field its operation needs (or of a kind not
 * listed, to be refused), or the host's own `run_started`, `release` or
 * `reload`. Anything else is `null`.
 */
export function read(message) {
  if (message === null || typeof message !== "object") {
    return null;
  }
  switch (message.type) {
    case "release":
    case "reload":
    case "run_started":
      return message;
    case "request": {
      if (!Number.isInteger(message.id)) {
        return null;
      }
      // A request of a kind not listed is read as it is, for the policy to
      // refuse: it has an id, so it gets an answer.
      if (typeof message.op !== "string" || !Object.hasOwn(OPERATIONS, message.op)) {
        return message;
      }
      return OPERATIONS[message.op].every((field) => message[field] !== undefined) ? message : null;
    }
    default:
      return null;
  }
}

/**
 * A DOMSnapshot without the value of any password field: each `<input
 * type=password>` loses its `inputValue` entry, and the words it held are
 * blanked in the snapshot's string table. The run never reads a password's
 * value; this keeps it from leaving Chrome at all. A string the page also
 * uses elsewhere is blanked there too, which costs at most a word of the
 * page and never a secret.
 */
export function withoutPasswords(snapshot) {
  const strings = snapshot?.strings;
  if (!Array.isArray(strings) || !Array.isArray(snapshot.documents)) {
    return snapshot;
  }
  const text = (index) => (typeof strings[index] === "string" ? strings[index] : "");
  for (const document of snapshot.documents) {
    const nodes = document?.nodes;
    const values = nodes?.inputValue;
    if (!values || !Array.isArray(values.index) || !Array.isArray(values.value)) {
      continue;
    }
    const isPassword = (node) => {
      if (text(nodes.nodeName?.[node]).toUpperCase() !== "INPUT") {
        return false;
      }
      const attributes = nodes.attributes?.[node] ?? [];
      for (let i = 0; i + 1 < attributes.length; i += 2) {
        if (text(attributes[i]).toLowerCase() === "type") {
          return text(attributes[i + 1]).toLowerCase() === "password";
        }
      }
      return false;
    };
    const index = [];
    const value = [];
    values.index.forEach((node, k) => {
      if (isPassword(node)) {
        if (Number.isInteger(values.value[k]) && values.value[k] >= 0) {
          strings[values.value[k]] = "";
        }
      } else {
        index.push(node);
        value.push(values.value[k]);
      }
    });
    nodes.inputValue = { index, value };
  }
  return snapshot;
}

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
 * A DOMSnapshot without the value of any password field. Every node of
 * every document is classified first, from the snapshot as Chrome sent it;
 * then each `<input type=password>` loses its `inputValue` entry, and the
 * words it held are blanked in the snapshot's string table unless the page
 * uses that string for something else too, where it is already in plain
 * sight and blanking it would change what the page reads as.
 */
export function withoutPasswords(snapshot) {
  const strings = snapshot?.strings;
  if (!Array.isArray(strings) || !Array.isArray(snapshot.documents)) {
    return snapshot;
  }
  const text = (index) => (typeof strings[index] === "string" ? strings[index] : "");
  const isPassword = (nodes, node) => {
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

  // First, from the snapshot untouched: which values are passwords, and
  // which strings anything else uses.
  const secret = new Set();
  const used = new Set();
  const use = (index) => {
    if (Number.isInteger(index)) {
      used.add(index);
    }
  };
  const useAll = (value) => {
    if (Array.isArray(value)) {
      value.forEach(useAll);
    } else {
      use(value);
    }
  };
  const kept = snapshot.documents.map((document) => {
    const nodes = document?.nodes;
    const values = nodes?.inputValue;
    for (const key of ["documentURL", "title", "baseURL", "contentLanguage", "encodingName", "publicId", "systemId", "frameId"]) {
      use(document?.[key]);
    }
    if (nodes) {
      useAll(nodes.nodeName);
      useAll(nodes.nodeValue);
      useAll(nodes.attributes);
      useAll(nodes.pseudoIdentifier);
      for (const rare of ["textValue", "currentSourceURL", "originURL"]) {
        useAll(nodes[rare]?.value);
      }
    }
    useAll(document?.layout?.text);
    useAll(document?.layout?.styles);
    if (!values || !Array.isArray(values.index) || !Array.isArray(values.value)) {
      return null;
    }
    const index = [];
    const value = [];
    values.index.forEach((node, k) => {
      if (isPassword(nodes, node)) {
        if (Number.isInteger(values.value[k])) {
          secret.add(values.value[k]);
        }
      } else {
        index.push(node);
        value.push(values.value[k]);
        use(values.value[k]);
      }
    });
    return { index, value };
  });

  // Then the changes.
  snapshot.documents.forEach((document, d) => {
    if (kept[d]) {
      document.nodes.inputValue = kept[d];
    }
  });
  for (const index of secret) {
    if (!used.has(index) && index >= 0 && index < strings.length) {
      strings[index] = "";
    }
  }
  return snapshot;
}

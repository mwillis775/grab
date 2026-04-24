// GrabNet Resolver — background service worker (MV3)
//
// Responsibilities:
//   • Maintain the user's gateway URL in chrome.storage.sync.
//   • Provide an omnibox keyword (`grab`) that routes queries to the gateway:
//       - "grab <site-id>" or "grab <site-id>/<path>"        -> /site/<id>/<path>
//       - "grab <hostname>" (contains a dot)                 -> gateway root with Host alias
//       - "grab ens <name>.eth"                              -> /ens/<name>.eth (gateway-side)
//   • Rewrite navigations to grab://<host-or-id>/<path> via declarativeNetRequest.

const DEFAULT_GATEWAY = "http://127.0.0.1:8080";

async function getGateway() {
  const { gatewayUrl } = await chrome.storage.sync.get({ gatewayUrl: DEFAULT_GATEWAY });
  return (gatewayUrl || DEFAULT_GATEWAY).replace(/\/+$/, "");
}

function looksLikeBase58SiteId(s) {
  // Base58 is 32-bytes encoded → typically 43-44 chars, no 0/O/I/l.
  return /^[1-9A-HJ-NP-Za-km-z]{40,50}$/.test(s);
}

function isHostname(s) {
  return /\./.test(s) && !/\s/.test(s);
}

async function buildUrlForInput(input) {
  const gw = await getGateway();
  const trimmed = input.trim();
  if (!trimmed) return gw + "/";

  // ENS pass-through: `ens vitalik.eth`
  const ensMatch = trimmed.match(/^ens\s+([^\s/]+\.eth)(\/.*)?$/i);
  if (ensMatch) {
    return `${gw}/ens/${encodeURIComponent(ensMatch[1])}${ensMatch[2] || "/"}`;
  }

  // grab://host/path
  const grabUri = trimmed.match(/^grab:\/\/([^/]+)(\/.*)?$/i);
  if (grabUri) {
    return rewriteGrabUri(gw, grabUri[1], grabUri[2] || "/");
  }

  // bare site-id [/path]
  const sitePart = trimmed.split("/")[0];
  const pathPart = trimmed.slice(sitePart.length) || "/";
  if (looksLikeBase58SiteId(sitePart)) {
    return `${gw}/site/${sitePart}${pathPart}`;
  }

  // hostname (has a dot) — let the gateway alias/DNS resolve it
  if (isHostname(sitePart)) {
    return `${gw}${pathPart === "/" ? "/" : pathPart}#__grab_host=${encodeURIComponent(sitePart)}`;
  }

  // fallback: treat as a search/path on the gateway root
  return `${gw}/${trimmed.replace(/^\/+/, "")}`;
}

function rewriteGrabUri(gw, host, path) {
  if (looksLikeBase58SiteId(host)) {
    return `${gw}/site/${host}${path}`;
  }
  if (host.endsWith(".eth")) {
    return `${gw}/ens/${encodeURIComponent(host)}${path}`;
  }
  // Host alias / DNS path — gateway needs the Host header. We can't set
  // arbitrary Host headers from a background page, but we can hit the gateway
  // root and rely on the gateway's --alias or --dns-aliases resolution when
  // the gateway is reverse-proxied at that hostname. As a fallback we expose
  // the host via a fragment so the user (or the gateway operator) can see it.
  return `${gw}${path}#__grab_host=${encodeURIComponent(host)}`;
}

// ---------------------------------------------------------------------------
// Omnibox
// ---------------------------------------------------------------------------

chrome.omnibox.onInputEntered.addListener(async (text) => {
  const url = await buildUrlForInput(text);
  chrome.tabs.update({ url });
});

chrome.omnibox.onInputChanged.addListener(async (text, suggest) => {
  const gw = await getGateway();
  suggest([
    {
      content: text,
      description: `Open via GrabNet gateway <url>${gw}</url>`,
    },
  ]);
});

// ---------------------------------------------------------------------------
// grab:// rewriter via declarativeNetRequest (best-effort).
// MV3 does not let extensions register custom URL schemes, but we can rewrite
// any navigation that ended up as http(s)://*/grab%3A%2F%2F... fallbacks, and
// the content script handles in-page <a href="grab://..."> clicks.
// ---------------------------------------------------------------------------

chrome.runtime.onInstalled.addListener(async () => {
  // Seed default gateway on first install
  const cur = await chrome.storage.sync.get(["gatewayUrl"]);
  if (!cur.gatewayUrl) {
    await chrome.storage.sync.set({ gatewayUrl: DEFAULT_GATEWAY });
  }
});

// Message API for content scripts.
chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (msg && msg.type === "grab:resolve") {
    buildUrlForInput(msg.input).then((url) => sendResponse({ url }));
    return true; // async
  }
  if (msg && msg.type === "grab:get-gateway") {
    getGateway().then((url) => sendResponse({ url }));
    return true;
  }
  return false;
});

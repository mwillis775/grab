// GrabNet Resolver — content script
//
// Rewrites in-page links of the form `grab://<host-or-id>/<path>` to point at
// the configured gateway, so they're clickable in any browser.

(async function () {
  const { url: gateway } = await chrome.runtime.sendMessage({ type: "grab:get-gateway" });
  if (!gateway) return;

  const SITE_ID_RE = /^[1-9A-HJ-NP-Za-km-z]{40,50}$/;

  function rewrite(href) {
    const m = href.match(/^grab:\/\/([^/]+)(\/.*)?$/i);
    if (!m) return null;
    const host = m[1];
    const path = m[2] || "/";
    if (SITE_ID_RE.test(host)) return `${gateway}/site/${host}${path}`;
    if (host.endsWith(".eth")) return `${gateway}/ens/${encodeURIComponent(host)}${path}`;
    return `${gateway}${path}#__grab_host=${encodeURIComponent(host)}`;
  }

  function tagAnchor(a) {
    if (!a || a.dataset.grabRewritten === "1") return;
    const href = a.getAttribute("href");
    if (!href || !href.toLowerCase().startsWith("grab://")) return;
    const dest = rewrite(href);
    if (dest) {
      a.dataset.grabOriginalHref = href;
      a.dataset.grabRewritten = "1";
      a.setAttribute("href", dest);
      a.setAttribute("title", `via GrabNet: ${href}`);
    }
  }

  document.querySelectorAll('a[href^="grab://"], a[href^="GRAB://"]').forEach(tagAnchor);

  // Pick up dynamically-added links.
  const obs = new MutationObserver((records) => {
    for (const rec of records) {
      for (const node of rec.addedNodes) {
        if (node.nodeType !== 1) continue;
        if (node.matches && node.matches('a[href^="grab://" i]')) tagAnchor(node);
        if (node.querySelectorAll) {
          node.querySelectorAll('a[href^="grab://" i]').forEach(tagAnchor);
        }
      }
    }
  });
  obs.observe(document.documentElement, { childList: true, subtree: true });
})();

# GrabNet Resolver (Browser Extension)

A WebExtension (Manifest V3) that makes the decentralized GrabNet web reachable from any standard browser by routing `grab://` URIs and the `grab` omnibox keyword through a local or hosted **GrabNet HTTP gateway**.

## What it does

- **Omnibox keyword `grab`** — type `grab` then space in the address bar:
  - `grab <base58 site-id>[/path]` → opens `<gateway>/site/<id>/path`
  - `grab <hostname>[/path]` → opens the gateway, hinting `Host:` so a configured `--alias` or `--dns-aliases` can route it
  - `grab ens <name>.eth` → opens `<gateway>/ens/<name>.eth`
- **Link rewriting** — anchors with `href="grab://…"` on any page are rewritten on the fly to gateway URLs (the original URI is preserved in `data-grab-original-href` and `title`).
- **Configurable gateway** — defaults to `http://127.0.0.1:8080`. Point it at any trusted gateway via the options page.

## Why an extension instead of a native scheme

Manifest V3 does not allow extensions to register custom URL schemes (`grab://`) at the OS level. The omnibox keyword + link rewriter give you a near-equivalent experience: clickable `grab://` links and address-bar resolution, all without changing browser internals.

If you also want true `grab://` deep links, install the GrabNet desktop app, which registers `grab://` with your OS.

## Install (unpacked)

1. Open `chrome://extensions` (or `about:debugging` in Firefox).
2. Enable **Developer mode**.
3. Choose **Load unpacked** and select this `extension/grabnet-resolver/` folder.
4. Click the toolbar icon to set your gateway URL (defaults to `http://127.0.0.1:8080`).
5. Run a gateway:
   ```sh
   grab gateway --port 8080 --dns-aliases \
       --alias rootedrevival.us=rootedrevival
   ```
6. In the address bar try: `grab rootedrevival.us` or `grab <site-id>`.

## Files

- `manifest.json` — MV3 manifest.
- `background.js` — Omnibox handler + URL builder.
- `content.js` — In-page `grab://` link rewriter.
- `options.html` / `options.js` — Gateway URL settings.
- `popup.html` / `popup.js` — Toolbar quick-open.
- `icons/` — Extension icons (placeholders; replace as needed).

## Privacy

The extension only contacts your configured gateway. No third-party services are queried unless your gateway is configured to use one (e.g. ENS resolution).

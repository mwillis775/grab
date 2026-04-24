const DEFAULT_GATEWAY = "http://127.0.0.1:8080";

const input = document.getElementById("gateway");
const saved = document.getElementById("saved");

(async () => {
  const { gatewayUrl } = await chrome.storage.sync.get({ gatewayUrl: DEFAULT_GATEWAY });
  input.value = gatewayUrl;
})();

document.getElementById("save").addEventListener("click", async () => {
  let url = input.value.trim() || DEFAULT_GATEWAY;
  url = url.replace(/\/+$/, "");
  await chrome.storage.sync.set({ gatewayUrl: url });
  saved.hidden = false;
  setTimeout(() => (saved.hidden = true), 1500);
});

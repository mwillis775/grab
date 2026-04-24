const q = document.getElementById("q");
q.addEventListener("keydown", async (ev) => {
  if (ev.key !== "Enter") return;
  const input = q.value.trim();
  if (!input) return;
  const { url } = await chrome.runtime.sendMessage({ type: "grab:resolve", input });
  if (url) chrome.tabs.create({ url });
});

document.getElementById("opts").addEventListener("click", (ev) => {
  ev.preventDefault();
  chrome.runtime.openOptionsPage();
});

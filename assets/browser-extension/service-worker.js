// ── sbxw extension: the toolbar icon ──────────────────────────────────────
// Clicking the icon (or Alt+Shift+S) opens the side panel beside the current
// tab. Done here, from `action.onClicked`, rather than with
// `openPanelOnActionClick`, because this is what grants `activeTab` on that
// tab: the panel can then read its URL and take a screenshot of it with no
// dialog and no "all sites" permission (see panel.js).

chrome.action.onClicked.addListener(tab => {
  // First and synchronous: `sidePanel.open` only works inside the click.
  chrome.sidePanel.open({ windowId: tab.windowId });
  // A panel already open learns that it can now read this tab.
  chrome.runtime.sendMessage({ type: 'sbxw:granted', tabId: tab.id }).catch(() => {});
});

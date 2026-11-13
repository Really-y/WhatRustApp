const { contextBridge, ipcRenderer, webFrame } = require('electron');

// Clicking a WhatsApp notification brings the window back.
contextBridge.exposeInMainWorld('__walite', { focus: () => ipcRenderer.send('walite-focus') });
webFrame.executeJavaScript(`(() => {
  const N = window.Notification;
  if (!N || N.__walite) return;
  const W = function (title, opts) {
    const n = new N(title, opts);
    n.addEventListener('click', () => window.__walite.focus());
    return n;
  };
  W.prototype = N.prototype;
  W.requestPermission = N.requestPermission.bind(N);
  Object.defineProperty(W, 'permission', { get: () => N.permission });
  W.__walite = true;
  window.Notification = W;
})()`);

// While hidden, force a full V8 GC every 60 s (needs --expose-gc). Nothing
// runs while the window is visible, so there is no jank.
let timer = null;
function onVis() {
  if (document.hidden && !timer) {
    const run = () => { try { globalThis.gc && globalThis.gc(); webFrame.clearCache(); } catch (_) {} };
    setTimeout(run, 15000);
    timer = setInterval(run, 60000);
  } else if (!document.hidden && timer) {
    clearInterval(timer);
    timer = null;
  }
}
window.addEventListener('DOMContentLoaded', () => {
  document.addEventListener('visibilitychange', onVis);
  onVis();
});

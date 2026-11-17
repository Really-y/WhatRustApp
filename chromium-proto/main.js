// WaLite: WhatsApp Web (with calls) on the system Chromium runtime (electron41),
// fast while visible, memory reclaimed while hidden. Run: electron41 chromium-proto
const { app, BrowserWindow, Menu, Tray, ipcMain, nativeImage, session, shell } = require('electron');
const path = require('path');

const PROFILE = path.join(app.getPath('home'), '.local/share/walite-chromium');
app.setPath('userData', PROFILE);

// --- memory tuning ---------------------------------------------------------
const sw = app.commandLine.appendSwitch.bind(app.commandLine);
sw('renderer-process-limit', '1');
sw('process-per-site');
sw('disable-site-isolation-trials');
sw('disable-features', [
  'SpareRendererForSitePerProcess',
  'BackForwardCache',
  'MediaRouter',
  'OptimizationHints',
  'Translate',
  'CalculateNativeWinOcclusion',
].join(','));
// Speed while visible: full JIT, GPU compositing, roomy heap cap (a tight cap
// caused GC stalls). RAM is reclaimed only while hidden (see preload.js).
// --expose-gc lets preload trigger that background GC.
sw('js-flags', '--max-old-space-size=1024 --expose-gc');
if (process.env.WALITE_NO_GPU) sw('disable-gpu');

const UA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) ' +
  `Chrome/${process.versions.chrome} Safari/537.36`;

// WhatsApp Web shows the call buttons only when WAWebVoipGatingUtils.isCallingEnabled()
// is true. Outside the official Windows app that is the server-sent AB prop
// `enable_web_calling`, which is still being rolled out per account. This hook
// runs before any page script, wraps the module loader (`__d`) and, once the
// WAWebABProps module is defined, reports `enable_web_calling` as true. Only
// that one flag changes; everything else still comes from WhatsApp.
// WALITE_NO_CALL_FLAG=1 turns this off.
const ENABLE_CALLING_HOOK = `(() => {
  const FLAG = 'enable_web_calling';
  const patch = (ex) => {
    const orig = ex && ex.getABPropConfigValue;
    if (typeof orig !== 'function' || orig.__walite) return;
    const p = function (name) { return name === FLAG ? true : orig.apply(this, arguments); };
    p.__walite = true;
    ex.getABPropConfigValue = p;
  };
  const wrapDefine = (real) => function (name, deps, factory, flags) {
    if (name === 'WAWebABProps' && typeof factory === 'function') {
      const f = factory;
      // Keep arity 7: the loader passes arguments based on factory.length.
      factory = function (a, b, c, d, e, g, exportsObj) {
        const r = f.apply(this, arguments);
        patch(exportsObj);
        return r;
      };
    }
    return real.apply(this, arguments);
  };
  let current;
  Object.defineProperty(window, '__d', {
    configurable: true,
    get() { return current; },
    set(v) { current = typeof v === 'function' ? wrapDefine(v) : v; },
  });
})();`;

// Attaches the Chrome DevTools Protocol to the page once, before WhatsApp loads:
// - Client Hints (navigator.userAgentData, Sec-CH-UA) report "Google Chrome" to
//   match the UA string (Electron says "Chromium"). WALITE_NO_UA_HINTS=1 skips it.
// - injects ENABLE_CALLING_HOOK into every new document.
async function preparePage(wc) {
  const hints = !process.env.WALITE_NO_UA_HINTS;
  const calls = !process.env.WALITE_NO_CALL_FLAG;
  if (!hints && !calls) return;
  try {
    wc.debugger.attach('1.3');
  } catch (e) {
    console.error('walite: cannot attach debugger:', e.message);
    return;
  }
  if (calls) {
    try {
      await wc.debugger.sendCommand('Page.enable');
      await wc.debugger.sendCommand('Page.addScriptToEvaluateOnNewDocument', { source: ENABLE_CALLING_HOOK });
    } catch (e) {
      console.error('walite: call flag hook failed:', e.message);
    }
  }
  if (!hints) return;
  const full = process.versions.chrome;
  const major = full.split('.')[0];
  const brands = (v, filler) => [
    { brand: 'Google Chrome', version: v },
    { brand: 'Chromium', version: v },
    { brand: 'Not-A.Brand', version: filler },
  ];
  try {
    await wc.debugger.sendCommand('Emulation.setUserAgentOverride', {
      userAgent: UA,
      userAgentMetadata: {
        brands: brands(major, '24'),
        fullVersionList: brands(full, '24.0.0.0'),
        fullVersion: full,
        platform: 'Linux',
        platformVersion: '',
        architecture: 'x86',
        bitness: '64',
        model: '',
        mobile: false,
        wow64: false,
      },
    });
  } catch (e) {
    console.error('walite: Client Hints override failed:', e.message);
  }
}

let tray = null;
let quitting = false;
let win = null;

// Logo assets: ./assets when installed, ../assets when run from the repo.
const fs = require('fs');
const ASSETS = [path.join(__dirname, 'assets'), path.join(__dirname, '..', 'assets')]
  .find((d) => fs.existsSync(path.join(d, 'logo-32.png')));

// Tray icon = logo, plus a red dot (top-right) when there are unread messages.
function createTrayIcon(unread) {
  const base = ASSETS ? nativeImage.createFromPath(path.join(ASSETS, 'logo-32.png')) : null;
  if (!base || base.isEmpty()) return nativeImage.createEmpty();
  if (unread <= 0) return base;
  const { width, height } = base.getSize();
  const buf = Buffer.from(base.toBitmap()); // BGRA on Linux
  const cx = width * 0.78, cy = height * 0.22, r = width * 0.2;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const dx = x + 0.5 - cx, dy = y + 0.5 - cy;
      const d = Math.sqrt(dx * dx + dy * dy);
      if (d > r + 1) continue;
      const i = (y * width + x) * 4;
      // white ring around a red dot for contrast
      const [b, g, rr] = d > r - 1.5 ? [255, 255, 255] : [40, 40, 230];
      buf[i] = b; buf[i + 1] = g; buf[i + 2] = rr; buf[i + 3] = 255;
    }
  }
  return nativeImage.createFromBitmap(buf, { width, height });
}

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => {
    if (win) {
      if (!win.isVisible()) win.show();
      if (win.isMinimized()) win.restore();
      win.focus();
    }
  });

  app.on('before-quit', () => {
    quitting = true;
  });

  app.whenReady().then(() => {
    session.defaultSession.setPermissionRequestHandler((wc, perm, cb) => {
      cb(['media', 'notifications', 'clipboard-sanitized-write', 'fullscreen'].includes(perm));
    });

    session.defaultSession.on('will-download', (event, item) => {
      item.setSavePath(path.join(app.getPath('downloads'), item.getFilename()));
    });

    win = new BrowserWindow({
      width: 1100, height: 760, title: 'WhatsApp',
      icon: ASSETS ? path.join(ASSETS, 'logo-256.png') : undefined,
      show: !process.env.WALITE_HIDDEN,
      webPreferences: {
        preload: path.join(__dirname, 'preload.js'),
        backgroundThrottling: true,
        spellcheck: false,
        contextIsolation: true,
      },
    });
    win.setMenu(null);
    win.webContents.setUserAgent(UA);
    win.webContents.setWindowOpenHandler(({ url }) => {
      shell.openExternal(url);
      return { action: 'deny' };
    });

    const contextMenu = Menu.buildFromTemplate([
      {
        label: 'Show',
        click: () => {
          if (win) {
            if (win.isMinimized()) win.restore();
            win.show();
            win.focus();
          }
        },
      },
      {
        label: 'Quit',
        click: () => {
          quitting = true;
          app.quit();
        },
      },
    ]);

    tray = new Tray(createTrayIcon(0));
    tray.setToolTip('WhatsApp');
    tray.setContextMenu(contextMenu);

    tray.on('click', () => {
      if (!win) return;
      if (win.isVisible() && !win.isMinimized()) {
        win.hide();
      } else {
        if (win.isMinimized()) win.restore();
        win.show();
        win.focus();
      }
    });

    // Web notification clicked (wrapped in preload.js) → bring window up.
    ipcMain.on('walite-focus', () => {
      if (win.isMinimized()) win.restore();
      win.show();
      win.focus();
    });

    win.on('close', (event) => {
      if (!quitting) {
        event.preventDefault();
        win.hide();
      }
    });

    let currentUnreadCount = 0;
    win.on('page-title-updated', (event, title) => {
      event.preventDefault();
      const match = title.match(/\((\d+)\)/);
      const unreadCount = match ? parseInt(match[1], 10) : 0;
      const newTitle = unreadCount > 0 ? `(${unreadCount}) WhatsApp` : 'WhatsApp';
      win.setTitle(newTitle);

      if (unreadCount !== currentUnreadCount) {
        currentUnreadCount = unreadCount;
        if (tray) {
          tray.setImage(createTrayIcon(unreadCount));
          tray.setToolTip(unreadCount > 0 ? `WhatsApp (${unreadCount})` : 'WhatsApp');
        }
        app.setBadgeCount(unreadCount);
      }
    });

    win.webContents.on('before-input-event', (event, input) => {
      if (input.type === 'keyDown' && (input.control || input.meta)) {
        if (input.key.toLowerCase() === 'q') {
          event.preventDefault();
          quitting = true;
          app.quit();
        } else if (input.key.toLowerCase() === 'r') {
          event.preventDefault();
          win.webContents.reload();
        }
      }
    });

    // The CDP override needs a live renderer, so load a blank page first.
    win.loadURL('about:blank')
      .then(() => preparePage(win.webContents))
      .then(() => win.loadURL('https://web.whatsapp.com/', { userAgent: UA }));
  });
}

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

let tray = null;
let quitting = false;
let win = null;

function createTrayIcon(unread) {
  const size = 32;
  const buf = Buffer.alloc(size * size * 4);
  const cx = 15.5;
  const cy = 15.5;
  const rOuter2 = 14 * 14;
  const rInner2 = 7 * 7;
  const redCx = 24.5;
  const redCy = 7.5;
  const rRed2 = 5 * 5;

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const idx = (y * size + x) * 4;
      const dx = x - cx;
      const dy = y - cy;
      const d2 = dx * dx + dy * dy;

      let r = 0;
      let g = 0;
      let b = 0;
      let a = 0;

      if (d2 <= rOuter2) {
        if (d2 <= rInner2) {
          // White inner circle
          r = 255;
          g = 255;
          b = 255;
          a = 255;
        } else {
          // WhatsApp green circle #25D366
          r = 0x25;
          g = 0xd3;
          b = 0x66;
          a = 255;
        }
      }

      if (unread > 0) {
        const rdx = x - redCx;
        const rdy = y - redCy;
        if (rdx * rdx + rdy * rdy <= rRed2) {
          // Red dot top-right
          r = 255;
          g = 0;
          b = 0;
          a = 255;
        }
      }

      // createFromBitmap expects Skia's native BGRA order on Linux.
      buf[idx] = b;
      buf[idx + 1] = g;
      buf[idx + 2] = r;
      buf[idx + 3] = a;
    }
  }

  return nativeImage.createFromBitmap(buf, { width: size, height: size });
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

    win.loadURL('https://web.whatsapp.com/', { userAgent: UA });
  });
}

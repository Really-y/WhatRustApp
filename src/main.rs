//! WaLite — lightweight WhatsApp Web wrapper (GTK3 + WebKitGTK).
//!
//! Uses the official web.whatsapp.com (no unofficial protocol → no ban risk),
//! with WebKitGTK tuned for low memory instead of a bundled Chromium.

mod tray;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use webkit2gtk::{
    CacheModel, CookieManagerExt, DeviceInfoPermissionRequest, CookiePersistentStorage, DownloadExt, HardwareAccelerationPolicy,
    MemoryPressureSettings, NavigationPolicyDecision, NavigationPolicyDecisionExt, NavigationType,
    NotificationExt, NotificationPermissionRequest, PermissionRequestExt, PolicyDecisionExt,
    PolicyDecisionType, ResponsePolicyDecision, ResponsePolicyDecisionExt, SecurityOrigin,
    SettingsExt, URIRequestExt, UserMediaPermissionRequest,
    WebContext, WebContextExt, WebView, WebViewExt, WebsiteDataManager, WebsiteDataManagerExt,
};

const APP_ID: &str = "az.walite.WaLite";
const HOME_URL: &str = "https://web.whatsapp.com/";
// WhatsApp Web refuses unknown browsers; present a current Chrome on Linux.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";

/// Web-process memory limit in MB (WebKit starts releasing caches and running
/// GC well below this). Override with WALITE_MEM_LIMIT=<MB>.
/// Measured: WhatsApp needs ~500 MB live; a lower limit keeps WebKit in
/// permanent "critical" mode (cache purge + GC every poll) and makes the UI janky.
const DEFAULT_MEM_LIMIT_MB: u32 = 1200;

pub enum Cmd {
    Toggle,
    Show,
    Quit,
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn env_flag(name: &str) -> bool {
    matches!(std::env::var(name).as_deref(), Ok("1") | Ok("true") | Ok("yes"))
}

fn dirs() -> (PathBuf, PathBuf) {
    let data = glib::user_data_dir().join("walite");
    let cache = glib::user_cache_dir().join("walite");
    let _ = std::fs::create_dir_all(&data);
    let _ = std::fs::create_dir_all(&cache);
    (data, cache)
}

fn memory_settings(limit_mb: u32) -> MemoryPressureSettings {
    let mut s = MemoryPressureSettings::new();
    s.set_memory_limit(limit_mb);
    // Trim at ~60% (≈720 MB), get aggressive at ~80%, never kill the process.
    s.set_conservative_threshold(0.6);
    s.set_strict_threshold(0.8);
    s.set_kill_threshold(0.0);
    s.set_poll_interval(30.0);
    s
}

fn build_context() -> WebContext {
    let (data, cache) = dirs();
    let limit = env_u32("WALITE_MEM_LIMIT").unwrap_or(DEFAULT_MEM_LIMIT_MB);

    // Network process limit (static, must be set before the manager exists).
    WebsiteDataManager::set_memory_pressure_settings(&mut memory_settings(limit.min(256)));

    let manager = WebsiteDataManager::builder()
        .base_data_directory(data.to_string_lossy().to_string())
        .base_cache_directory(cache.to_string_lossy().to_string())
        .build();
    if let Some(cookies) = manager.cookie_manager() {
        cookies.set_persistent_storage(
            &data.join("cookies.sqlite").to_string_lossy(),
            CookiePersistentStorage::Sqlite,
        );
    }

    let ctx = WebContext::builder()
        .website_data_manager(&manager)
        .memory_pressure_settings(&memory_settings(limit))
        .build();
    // Single-page app: no back/forward or page caching needed.
    ctx.set_cache_model(CacheModel::DocumentViewer);
    ctx.set_spell_checking_enabled(false);

    // Background-GC extension (libwalite_ext.so), see ext/src/lib.rs.
    ctx.connect_initialize_web_extensions(|ctx| match extension_dir() {
        Some(dir) => ctx.set_web_extensions_directory(&dir.to_string_lossy()),
        None => eprintln!("walite: libwalite_ext.so not found; background GC disabled"),
    });

    // Re-grant notification permission on every start so Notification.permission
    // is "granted" without a prompt.
    ctx.connect_initialize_notification_permissions(|ctx| {
        let origin = SecurityOrigin::new("https", "web.whatsapp.com", 0);
        ctx.initialize_notification_permissions(&[&origin], &[]);
    });

    ctx.connect_download_started(|_, download| {
        download.connect_decide_destination(|d, suggested| {
            let dir = glib::user_special_dir(glib::UserDirectory::Downloads)
                .unwrap_or_else(glib::home_dir);
            let name = std::path::Path::new(suggested)
                .file_name()
                .map(|n| n.to_owned())
                .unwrap_or_else(|| "whatsapp-download".into());
            d.set_destination(&dir.join(name).to_string_lossy());
            true
        });
    });
    ctx
}

/// Directory holding libwalite_ext.so: next to the binary (cargo builds) or
/// in ../lib/walite (installed layout).
fn extension_dir() -> Option<PathBuf> {
    const LIB: &str = "libwalite_ext.so";
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [exe_dir.join("../lib/walite"), exe_dir]
        .into_iter()
        .find(|d| d.join(LIB).is_file())
}

fn is_whatsapp(uri: &str) -> bool {
    let rest = match uri.split_once("://") {
        Some(("https", rest)) => rest,
        _ => return uri.starts_with("blob:") || uri.starts_with("about:") || uri.starts_with("data:"),
    };
    let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("");
    host == "whatsapp.com" || host.ends_with(".whatsapp.com") || host.ends_with(".whatsapp.net")
}

fn open_external(uri: &str) {
    if let Err(e) = gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>) {
        eprintln!("walite: cannot open {uri}: {e}");
    }
}

fn build_webview(ctx: &WebContext) -> WebView {
    let webview = WebView::with_context(ctx);
    if let Some(s) = WebViewExt::settings(&webview) {
        s.set_user_agent(Some(USER_AGENT));
        s.set_enable_page_cache(false);
        s.set_enable_back_forward_navigation_gestures(false);
        s.set_enable_offline_web_application_cache(false);
        s.set_enable_html5_local_storage(true);
        s.set_enable_html5_database(true);
        s.set_enable_media_stream(true); // voice notes, calls (mic/camera)
        // Voice/video calls need WebRTC. WALITE_NO_CALLS=1 turns it off.
        s.set_enable_webrtc(!env_flag("WALITE_NO_CALLS"));
        s.set_enable_developer_extras(env_flag("WALITE_DEVTOOLS"));
        s.set_hardware_acceleration_policy(if env_flag("WALITE_GPU") {
            HardwareAccelerationPolicy::Always
        } else {
            HardwareAccelerationPolicy::Never
        });
    }

    webview.connect_permission_request(|_, req| {
        // Only web.whatsapp.com is ever loaded (other links open externally),
        // so mic/camera for voice notes and calls can be granted here.
        if req.is::<NotificationPermissionRequest>()
            || req.is::<UserMediaPermissionRequest>()
            || req.is::<DeviceInfoPermissionRequest>()
        {
            req.allow();
        } else {
            req.deny();
        }
        true
    });

    webview.connect_decide_policy(|_, decision, kind| match kind {
        PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction => {
            let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() else {
                return false;
            };
            let Some(action) = nav.navigation_action() else { return false };
            let Some(uri) = action.request().and_then(|r| r.uri()) else { return false };
            let user_link = action.navigation_type() == NavigationType::LinkClicked;
            if kind == PolicyDecisionType::NewWindowAction || (user_link && !is_whatsapp(&uri)) {
                if !uri.starts_with("blob:") {
                    open_external(&uri);
                }
                decision.ignore();
                return true;
            }
            false
        }
        PolicyDecisionType::Response => {
            if let Some(r) = decision.downcast_ref::<ResponsePolicyDecision>() {
                if !r.is_mime_type_supported() {
                    decision.download();
                    return true;
                }
            }
            false
        }
        _ => false,
    });

    webview
}

fn unread_from_title(title: &str) -> u32 {
    title
        .strip_prefix('(')
        .and_then(|t| t.split_once(')'))
        .and_then(|(n, _)| n.trim().parse().ok())
        .unwrap_or(0)
}

fn present(window: &gtk::ApplicationWindow) {
    window.show_all();
    window.present();
}

fn build_ui(app: &gtk::Application) {
    let ctx = build_context();
    let webview = build_webview(&ctx);

    let window = gtk::ApplicationWindow::new(app);
    window.set_title("WhatsApp");
    window.set_default_size(1100, 760);
    window.set_icon(Some(&tray::app_pixbuf(0)));
    window.add(&webview);

    let (tx, rx) = async_channel::unbounded::<Cmd>();
    let tray = tray::spawn(tx);
    let has_tray = tray.is_some();
    if has_tray {
        // Keep the app alive while the window is hidden in the tray.
        std::mem::forget(app.hold());
    }

    // Unread counter from the page title "(3) WhatsApp".
    {
        let tray = tray.clone();
        let window = window.clone();
        let last = Rc::new(Cell::new(u32::MAX));
        webview.connect_title_notify(move |wv| {
            let title = wv.title().map(|t| t.to_string()).unwrap_or_default();
            let n = unread_from_title(&title);
            if n != last.get() {
                last.set(n);
                window.set_title(if n > 0 { &title } else { "WhatsApp" });
                window.set_icon(Some(&tray::app_pixbuf(n)));
                if let Some(t) = &tray {
                    t.update(|t| t.unread = n);
                }
            }
        });
    }

    {
        let window = window.clone();
        webview.connect_show_notification(move |_, n| {
            let window = window.clone();
            n.connect_clicked(move |_| present(&window));
            false // let WebKit show the desktop notification
        });
    }

    // Close → hide to tray (keeps receiving messages). Without a tray, quit.
    window.connect_delete_event(move |w, _| {
        if has_tray {
            w.hide();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });

    // Ctrl+Q quits, Ctrl+R reloads.
    {
        let app = app.clone();
        let webview = webview.clone();
        window.connect_key_press_event(move |_, ev| {
            use gdk::keys::constants as k;
            if !ev.state().contains(gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            let key = ev.keyval();
            if key == k::q || key == k::Q {
                app.quit();
            } else if key == k::r || key == k::R {
                webview.reload();
            } else {
                return glib::Propagation::Proceed;
            }
            glib::Propagation::Stop
        });
    }

    {
        let app = app.clone();
        let window = window.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(cmd) = rx.recv().await {
                match cmd {
                    Cmd::Toggle if window.is_visible() && window.is_active() => window.hide(),
                    Cmd::Toggle | Cmd::Show => present(&window),
                    Cmd::Quit => app.quit(),
                }
            }
        });
    }

    webview.load_uri(HOME_URL);
    if has_tray && env_flag("WALITE_HIDDEN") {
        webview.show();
    } else {
        window.show_all();
    }
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::empty());
    app.connect_activate(|app| match app.windows().first() {
        Some(w) => {
            w.show_all();
            w.present();
        }
        None => build_ui(app),
    });
    app.run()
}

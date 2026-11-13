//! Loaded by WebKit into the web process. While the page is hidden (WaLite is
//! in the tray) it periodically forces a full JavaScriptCore GC and returns
//! freed malloc memory to the OS — roughly what Chromium does for background
//! tabs. While the window is visible it does nothing, so there is no jank.

use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

type Gpointer = *mut c_void;
type JSContextRef = *mut c_void;
type JSValueRef = *const c_void;
type JSStringRef = *mut c_void;

#[link(name = "glib-2.0")]
extern "C" {
    fn g_timeout_add_seconds(interval: c_uint, func: extern "C" fn(Gpointer) -> c_int, data: Gpointer) -> c_uint;
}
#[link(name = "gobject-2.0")]
extern "C" {
    fn g_signal_connect_data(
        instance: Gpointer,
        signal: *const c_char,
        handler: Gpointer,
        data: Gpointer,
        destroy: Gpointer,
        flags: c_int,
    ) -> c_ulong;
    fn g_object_ref(obj: Gpointer) -> Gpointer;
}
#[link(name = "webkit2gtk-4.1")]
extern "C" {
    fn webkit_web_page_get_main_frame(page: Gpointer) -> Gpointer;
    fn webkit_frame_get_javascript_global_context(frame: Gpointer) -> JSContextRef;
}
#[link(name = "javascriptcoregtk-4.1")]
extern "C" {
    fn JSStringCreateWithUTF8CString(s: *const c_char) -> JSStringRef;
    fn JSStringRelease(s: JSStringRef);
    fn JSEvaluateScript(
        ctx: JSContextRef,
        script: JSStringRef,
        this: *mut c_void,
        source_url: JSStringRef,
        line: c_int,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    fn JSValueToBoolean(ctx: JSContextRef, v: JSValueRef) -> bool;
    fn JSSynchronousGarbageCollectForDebugging(ctx: JSContextRef);
}
extern "C" {
    fn malloc_trim(pad: usize) -> c_int;
}

const TICK_SECS: u32 = 15;
/// While hidden: first GC one tick after hiding, then every 4 ticks (60 s).
const GC_EVERY_TICKS: u32 = 4;

static PAGE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static HIDDEN_TICKS: AtomicU32 = AtomicU32::new(0);

unsafe fn page_hidden(ctx: JSContextRef) -> bool {
    eval_bool(ctx, c"document.hidden === true")
}

unsafe fn eval_bool(ctx: JSContextRef, src: &std::ffi::CStr) -> bool {
    let script = JSStringCreateWithUTF8CString(src.as_ptr());
    let mut exc: JSValueRef = ptr::null();
    let v = JSEvaluateScript(ctx, script, ptr::null_mut(), ptr::null_mut(), 1, &mut exc);
    JSStringRelease(script);
    !v.is_null() && exc.is_null() && JSValueToBoolean(ctx, v)
}

extern "C" fn tick(_: Gpointer) -> c_int {
    unsafe {
        let page = PAGE.load(Ordering::Relaxed);
        if page.is_null() {
            return 1;
        }
        let frame = webkit_web_page_get_main_frame(page);
        if frame.is_null() {
            return 1;
        }
        let ctx = webkit_frame_get_javascript_global_context(frame);
        if ctx.is_null() {
            return 1;
        }
        let debug = std::env::var_os("WALITE_DEBUG").is_some();
        if debug {
            let granted = eval_bool(
                ctx,
                c"typeof Notification !== 'undefined' && Notification.permission === 'granted'",
            );
            eprintln!("walite-ext: notifications granted = {granted}");
            // Voice notes need getUserMedia(audio) + MediaRecorder.
            let rec = eval_bool(ctx, c"typeof MediaRecorder !== 'undefined'");
            let opus = eval_bool(
                ctx,
                c"typeof MediaRecorder !== 'undefined' && (MediaRecorder.isTypeSupported('audio/ogg;codecs=opus') || MediaRecorder.isTypeSupported('audio/webm;codecs=opus') || MediaRecorder.isTypeSupported('audio/mp4'))",
            );
            eval_bool(
                ctx,
                c"(window.__waliteMic === undefined) && (window.__waliteMic = 'pending', navigator.mediaDevices.getUserMedia({audio: true}).then(s => { window.__waliteMic = 'ok'; s.getTracks().forEach(t => t.stop()); }, e => { window.__waliteMic = 'err:' + e.name; })) && true",
            );
            let mic_ok = eval_bool(ctx, c"window.__waliteMic === 'ok'");
            let rtc = eval_bool(ctx, c"typeof RTCPeerConnection === 'function'");
            eprintln!("walite-ext: MediaRecorder = {rec}, opus/mp4 recording = {opus}, microphone = {mic_ok}, RTCPeerConnection = {rtc}");
        }
        if !page_hidden(ctx) {
            if debug {
                eprintln!("walite-ext: page visible");
            }
            HIDDEN_TICKS.store(0, Ordering::Relaxed);
            return 1;
        }
        let n = HIDDEN_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n % GC_EVERY_TICKS == 0 {
            let t = std::time::Instant::now();
            JSSynchronousGarbageCollectForDebugging(ctx);
            malloc_trim(0);
            if debug {
                eprintln!("walite-ext: full GC took {:?}", t.elapsed());
            }
        }
    }
    1 // G_SOURCE_CONTINUE
}

extern "C" fn page_created(_ext: Gpointer, page: Gpointer, _data: Gpointer) {
    // WaLite has a single page; keep the first one alive for the process lifetime.
    if PAGE.load(Ordering::Relaxed).is_null() {
        unsafe { PAGE.store(g_object_ref(page), Ordering::Relaxed) };
    }
}

/// Entry point looked up by WebKit.
#[no_mangle]
pub extern "C" fn webkit_web_extension_initialize(ext: Gpointer) {
    if std::env::var_os("WALITE_NO_BG_GC").is_some() {
        return;
    }
    unsafe {
        g_signal_connect_data(
            ext,
            c"page-created".as_ptr(),
            page_created as Gpointer,
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        );
        g_timeout_add_seconds(TICK_SECS, tick, ptr::null_mut());
    }
}

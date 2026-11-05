//! C ABI for the mobile app (Flutter via dart:ffi). The app runs the same server as the hub,
//! bound to 127.0.0.1 on a free port, and shows it in a WebView; native screens use its JSON API.
//!
//! All strings are UTF-8, NUL-terminated. Strings returned by this library must be released
//! with `sp_free`. Results are JSON: `{"ok": …}` or `{"error": "…"}`.
//!
//!   sp_start(config_json)  start (or restart) the server; config = sp_server::Config as JSON;
//!                          returns {"ok": {"port": N, "describe": "…"}}
//!   sp_stop()              stop the server (e.g. before replacing pack files)
//!   sp_share(port)         also serve on every interface (Wi-Fi, hotspot) at `port`, or the next free one
//!                          up to port+9, so people nearby can use it in a browser: {"ok": {"port": N, "https": M}};
//!                          with chat on, also HTTPS at 8443 (or the next free one; M, else 0), for chat's
//!                          microphone and GPS. 0 stops sharing. Kept across sp_start restarts.
//!   sp_blake3(path)        {"ok": "<hex>"}: verify downloads
//!   sp_version()           {"ok": "<crate version>"}

use std::ffi::{CStr, CString, c_char};
use std::sync::Mutex;

struct Running {
    shutdown: tokio::sync::oneshot::Sender<()>,
    thread: std::thread::JoinHandle<()>,
    handle: tokio::runtime::Handle,
    router: axum::Router,
    app: std::sync::Arc<sp_server::App>,
}

static SERVER: Mutex<Option<Running>> = Mutex::new(None);
/// The shared (LAN) listeners: requested port, and how to stop them (dropping the sender stops both).
static SHARE: Mutex<Option<(u16, Option<tokio::sync::watch::Sender<()>>)>> = Mutex::new(None);

fn out(r: anyhow::Result<serde_json::Value>) -> *mut c_char {
    let v = match r {
        Ok(v) => serde_json::json!({"ok": v}),
        Err(e) => serde_json::json!({"error": format!("{e:#}")}),
    };
    CString::new(v.to_string()).unwrap_or_default().into_raw()
}

unsafe fn arg(p: *const c_char) -> anyhow::Result<String> {
    anyhow::ensure!(!p.is_null(), "null argument");
    Ok(unsafe { CStr::from_ptr(p) }.to_str()?.to_string())
}

fn stop() {
    if let Some(s) = SHARE.lock().unwrap().as_mut() {
        s.1 = None;
    }
    if let Some(r) = SERVER.lock().unwrap().take() {
        let _ = r.shutdown.send(());
        let _ = r.thread.join();
    }
}

fn start(config: &str) -> anyhow::Result<serde_json::Value> {
    stop();
    let cfg: sp_server::Config = serde_json::from_str(config)?;
    let app = sp_server::load(&cfg)?;
    let describe = sp_server::describe(&app);
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let router = sp_server::router(app.clone(), &cfg);
    let handle = rt.handle().clone();
    let r2 = router.clone();
    let thread = std::thread::Builder::new().name("sp-server".into()).spawn(move || {
        rt.block_on(async move {
            let _ = axum::serve(listener, r2)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });
    })?;
    *SERVER.lock().unwrap() = Some(Running { shutdown: tx, thread, handle, router, app });
    // sharing was on before this restart: open it again
    let want = SHARE.lock().unwrap().as_ref().map(|s| s.0);
    if let Some(p) = want {
        let _ = share(p);
    }
    Ok(serde_json::json!({"port": port, "describe": describe}))
}

fn share(port: u16) -> anyhow::Result<serde_json::Value> {
    let mut sh = SHARE.lock().unwrap();
    // dropping the old sender stops the old listeners
    sh.take();
    if port == 0 {
        if let Some(c) = SERVER.lock().unwrap().as_ref().and_then(|r| r.app.chat.as_ref()) {
            c.set_https_port(0);
        }
        return Ok(serde_json::json!({"port": 0}));
    }
    let guard = SERVER.lock().unwrap();
    let Some(r) = guard.as_ref() else {
        // not running yet: sp_start will open it
        *sh = Some((port, None));
        return Ok(serde_json::json!({"port": 0}));
    };
    let (p, listener) = bind(&r.handle, port).ok_or_else(|| anyhow::anyhow!("no free port from {port}"))?;
    let (tx, rx) = tokio::sync::watch::channel(());
    let router = r.router.clone();
    let mut stop = rx.clone();
    r.handle.spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = stop.changed().await;
            })
            .await;
    });
    // HTTPS beside it, for chat's microphone and GPS (sp_server::tls); sharing works without it
    let mut https = 0;
    if let Some(chat) = &r.app.chat {
        if let (Ok(tls), Some((hp, l))) = (sp_server::tls::config(chat.dir()), bind(&r.handle, 8443)) {
            let _guard = r.handle.enter();
            if let Ok(l) = sp_server::tls::Listener::new(l, tls) {
                let (router, mut stop) = (r.router.clone(), rx.clone());
                r.handle.spawn(async move {
                    let _ = axum::serve(l, router)
                        .with_graceful_shutdown(async move {
                            let _ = stop.changed().await;
                        })
                        .await;
                });
                https = hp;
            }
        }
        chat.set_https_port(https);
    }
    *sh = Some((port, Some(tx)));
    Ok(serde_json::json!({"port": p, "https": https}))
}

/// Listens on every interface at `port`, or the next free one up to port+9.
fn bind(h: &tokio::runtime::Handle, port: u16) -> Option<(u16, tokio::net::TcpListener)> {
    for p in port..port.saturating_add(10) {
        // the old listener may take a moment to let go of the port after a restart
        for _ in 0..10 {
            match h.block_on(tokio::net::TcpListener::bind(("0.0.0.0", p))) {
                Ok(l) => return Some((p, l)),
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && p == port => std::thread::sleep(std::time::Duration::from_millis(100)),
                Err(_) => break,
            }
        }
    }
    None
}

/// # Safety
/// `config_json` must be a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sp_start(config_json: *const c_char) -> *mut c_char {
    out(unsafe { arg(config_json) }.and_then(|c| start(&c)))
}

#[unsafe(no_mangle)]
pub extern "C" fn sp_stop() {
    stop();
}

/// Shares the running server on the local network at `port` (or the next free one); 0 stops.
#[unsafe(no_mangle)]
pub extern "C" fn sp_share(port: u16) -> *mut c_char {
    out(share(port))
}

/// # Safety
/// `path` must be a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sp_blake3(path: *const c_char) -> *mut c_char {
    out(unsafe { arg(path) }.and_then(|p| {
        Ok(serde_json::json!(blake3::Hasher::new().update_mmap_rayon(&p)?.finalize().to_hex().to_string()))
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn sp_version() -> *mut c_char {
    out(Ok(serde_json::json!(env!("CARGO_PKG_VERSION"))))
}

/// # Safety
/// `s` must come from this library and not be freed twice.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sp_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, path: &str) -> Option<String> {
        use std::io::{Read, Write};
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
        write!(c, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").ok()?;
        let mut s = String::new();
        c.read_to_string(&mut s).ok()?;
        Some(s)
    }

    #[test]
    fn share_on_the_network_and_keep_it_across_restarts() {
        let chat = std::env::temp_dir().join(format!("sp-ffi-chat-{}", std::process::id()));
        let cfg = serde_json::json!({"chat": chat}).to_string();
        start(&cfg).unwrap();
        let v = share(18731).unwrap();
        let p = v["port"].as_u64().unwrap() as u16;
        assert!((18731..18741).contains(&p));
        let hp = v["https"].as_u64().unwrap() as u16;
        assert!(hp >= 8443, "no https: {v}");
        assert!(std::net::TcpStream::connect(("127.0.0.1", hp)).is_ok());
        // the chat page knows where to send people for the mic and GPS
        assert!(get(p, "/chat").unwrap().contains(&format!("const HTTPS_PORT = {hp};")));
        assert!(get(p, "/chat").unwrap().starts_with("HTTP/1.1 200"));
        // a restart (new content installed) keeps sharing
        start(&cfg).unwrap();
        let mut ok = false;
        for _ in 0..20 {
            if get(p, "/chat").is_some_and(|r| r.starts_with("HTTP/1.1 200")) {
                ok = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(ok, "shared port not reopened after restart");
        share(0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(get(p, "/chat").is_none());
        stop();
        std::fs::remove_dir_all(chat).ok();
    }

    #[test]
    fn version_roundtrip() {
        let p = sp_version();
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string();
        unsafe { sp_free(p) };
        assert!(s.contains("\"ok\""));
    }
}

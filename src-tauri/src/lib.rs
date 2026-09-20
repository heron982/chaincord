mod crypto;
mod erasure;
mod log;
mod nat;
mod peer;
mod relay;
mod store;
#[cfg(target_os = "linux")]
mod rtc;

use peer::{AppState, UiState};
use std::sync::Arc;
use tauri::{Manager, WebviewWindow};

#[tauri::command]
fn get_state(state: tauri::State<Arc<AppState>>) -> UiState {
    state.snapshot()
}

#[tauri::command]
fn create_community(name: String, relay: Option<String>, app: tauri::AppHandle) -> Result<(), String> {
    peer::create_local(&app, &name, relay.as_deref())
}

#[tauri::command]
fn join_community(invite: String, app: tauri::AppHandle) -> Result<(), String> {
    peer::join_community(&app, &invite)
}

#[tauri::command]
fn switch_community(id: String, app: tauri::AppHandle) -> Result<(), String> {
    peer::switch_community(&app, &id)
}

#[tauri::command]
fn send_chat(text: String, channel: Option<String>, app: tauri::AppHandle) -> Result<(), String> {
    peer::send_chat(&app, &text, channel.as_deref().unwrap_or("general"))
}

#[tauri::command]
fn add_room(kind: String, name: String, app: tauri::AppHandle) -> Result<(), String> {
    peer::add_room(&app, &kind, &name)
}

#[tauri::command]
fn join_call(room: String, app: tauri::AppHandle) -> Result<(), String> {
    peer::join_call(&app, &room)
}

#[tauri::command]
fn leave_call(app: tauri::AppHandle) -> Result<(), String> {
    peer::leave_call(&app)
}

#[tauri::command]
fn send_signal(frame: serde_json::Value, app: tauri::AppHandle) -> Result<(), String> {
    peer::send_signal(&app, frame)
}

#[tauri::command]
async fn rtc_start(room: String, me: String, app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        crate::rtc::start(&app, room, me).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (room, me, app);
        Ok(())
    }
}

#[tauri::command]
async fn rtc_stop(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        crate::rtc::stop(&app).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
        Ok(())
    }
}

#[tauri::command]
async fn rtc_sync(peers: Vec<String>, hub: Option<String>, app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        crate::rtc::sync(&app, peers, hub).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (peers, hub, app);
        Ok(())
    }
}

#[tauri::command]
async fn rtc_signal(frame: serde_json::Value, app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let parsed: crate::rtc::RtcFrameIn =
            serde_json::from_value(frame).map_err(|e| e.to_string())?;
        crate::rtc::handle(&app, parsed).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (frame, app);
        Ok(())
    }
}

#[tauri::command]
async fn rtc_push_frame(screen: bool, jpeg: String, app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        crate::rtc::push_frame(&app, screen, jpeg).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (screen, jpeg, app);
        Ok(())
    }
}

#[tauri::command]
async fn rtc_share_screen(on: bool, app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        crate::rtc::share_screen(&app, on).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (on, app);
        Err("Native capture is only available on Linux".into())
    }
}

#[tauri::command]
fn leave_community(force: Option<bool>, app: tauri::AppHandle) -> Result<(), String> {
    peer::leave_community(&app, force.unwrap_or(false))
}

#[tauri::command]
fn save_profile(display_name: String, avatar: String, app: tauri::AppHandle) -> Result<(), String> {
    peer::save_profile(&app, &display_name, &avatar)
}

#[tauri::command]
fn publish_presence(
    muted: bool,
    deafened: bool,
    status: Option<String>,
    sharing_screen: Option<bool>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    peer::publish_presence(&app, muted, deafened, status.as_deref(), sharing_screen)
}

#[tauri::command]
fn get_history(app: tauri::AppHandle) -> Vec<peer::UiMessage> {
    peer::get_history(&app)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallLogLine {
    t: Option<i64>,
    mode: Option<String>,
    peer: Option<String>,
    event: String,
    level: Option<String>,
}

#[tauri::command]
fn append_call_log(line: CallLogLine) {
    crate::log::append_call(
        line.t.unwrap_or(0),
        line.mode.as_deref().unwrap_or("-"),
        line.peer.as_deref(),
        &line.event,
        line.level.as_deref().unwrap_or("info"),
    );
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WEBKIT_DISABLE_SANDBOX").is_none() {
            std::env::set_var("WEBKIT_DISABLE_SANDBOX", "1");
        }
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }
    tauri::Builder::default()
        .manage(Arc::new(AppState::new()))
        .invoke_handler(tauri::generate_handler![
            get_state,
            create_community,
            join_community,
            switch_community,
            send_chat,
            add_room,
            join_call,
            leave_call,
            send_signal,
            rtc_start,
            rtc_stop,
            rtc_sync,
            rtc_signal,
            rtc_push_frame,
            rtc_share_screen,
            leave_community,
            save_profile,
            publish_presence,
            get_history,
            append_call_log
        ])
        .setup(|app| {
            #[cfg(desktop)]
            {
                app.handle()
                    .plugin(tauri_plugin_process::init())
                    .expect("process plugin");
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())
                    .expect("updater plugin");
            }
            peer::boot(app.handle());
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = peer::run_listener(handle).await {
                    eprintln!("listener: {err}");
                }
            });
            if let Some(window) = app.get_webview_window("main") {
                allow_media(&window);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                peer::announce_gone(window.app_handle());
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to start Chaincord");
}

fn allow_media(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        let _ = window.with_webview(|webview| {
            use webview2_com::{
                Microsoft::Web::WebView2::Win32::COREWEBVIEW2_PERMISSION_STATE_ALLOW,
                PermissionRequestedEventHandler,
            };
            unsafe {
                let Ok(core) = webview.controller().CoreWebView2() else {
                    return;
                };
                let mut token = 0_i64;
                let _ = core.add_PermissionRequested(
                    &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW);
                        Ok(())
                    })),
                    &mut token,
                );
            }
        });
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    {
        let _ = window.with_webview(|webview| {
            use webkit2gtk::glib::prelude::*;
            use webkit2gtk::{PermissionRequestExt, Settings, SettingsExt, WebViewExt};
            let view = webview.inner();
            let settings = view.settings().unwrap_or_else(Settings::new);
            settings.set_enable_media_stream(true);
            settings.set_enable_webrtc(true);
            settings.set_media_playback_requires_user_gesture(false);
            view.set_settings(&settings);
            view.connect_permission_request(|_, request| {
                if request.is::<webkit2gtk::UserMediaPermissionRequest>() {
                    request.allow();
                    return true;
                }
                false
            });
            if settings.enables_webrtc() {
                view.reload_bypass_cache();
            } else {
                eprintln!("chaincord: WebKit recusou enable-webrtc");
            }
        });
    }
}

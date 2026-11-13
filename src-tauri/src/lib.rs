//! The desktop app: a Tauri shell around gg-core. The UI reaches the core through one
//! command, `invoke_core(cmd, args)`, and receives its events as Tauri events.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use gg_core::{Core, CoreError, ErrorCode, EventSink};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::OnceCell;

/// Forwards core events to the webview.
struct TauriSink(AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, name: &str, payload: Value) {
        // Fails only once the app is shutting down.
        let _ = self.0.emit(name, payload);
    }
}

/// The core, started in the background at launch (finding git may ask the login shell for
/// its PATH, which takes a moment) so the window does not wait for it. Commands wait.
struct CoreSlot {
    app: AppHandle,
    core: OnceCell<Result<Arc<Core>, CoreError>>,
}

impl CoreSlot {
    async fn get(&self) -> Result<Arc<Core>, CoreError> {
        self.core
            .get_or_init(|| start_core(self.app.clone()))
            .await
            .clone()
    }
}

async fn start_core(app: AppHandle) -> Result<Arc<Core>, CoreError> {
    let config_dir = app.path().app_config_dir().map_err(|e| {
        CoreError::new(ErrorCode::Io, "Could not find the app's config folder")
            .with_detail(e.to_string())
    })?;
    let launch = launch_path(std::env::args_os().nth(1), std::env::var_os("GG_OPEN"));
    let core = Core::new(config_dir, launch, Arc::new(TauriSink(app))).await?;
    core.set_perf_log(std::env::var_os("GG_PERF_LOG").map(PathBuf::from));
    Ok(core)
}

/// The folder to open at launch: the first command-line argument if it is an existing
/// directory, else `GG_OPEN` if that is.
fn launch_path(first_arg: Option<OsString>, gg_open: Option<OsString>) -> Option<PathBuf> {
    [first_arg, gg_open]
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|p| p.is_dir())
}

#[tauri::command]
async fn invoke_core(
    slot: State<'_, Arc<CoreSlot>>,
    cmd: String,
    args: Value,
) -> Result<Value, CoreError> {
    slot.get().await?.dispatch(&cmd, args).await
}

/// The macOS menu bar: Tauri's default minus "Close Window", whose Cmd+W would close the
/// whole window; the UI uses Cmd+W to close the active tab instead (the red traffic light
/// still closes the window).
#[cfg(target_os = "macos")]
fn app_menu<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{AboutMetadata, Menu, PredefinedMenuItem as Item, Submenu};

    let name = app.package_info().name.clone();
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(app.package_info().version.to_string()),
        ..Default::default()
    };
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                name,
                true,
                &[
                    &Item::about(app, None, Some(about))?,
                    &Item::separator(app)?,
                    &Item::services(app, None)?,
                    &Item::separator(app)?,
                    &Item::hide(app, None)?,
                    &Item::hide_others(app, None)?,
                    &Item::show_all(app, None)?,
                    &Item::separator(app)?,
                    &Item::quit(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &Item::undo(app, None)?,
                    &Item::redo(app, None)?,
                    &Item::separator(app)?,
                    &Item::cut(app, None)?,
                    &Item::copy(app, None)?,
                    &Item::paste(app, None)?,
                    &Item::select_all(app, None)?,
                ],
            )?,
            &Submenu::with_items(app, "View", true, &[&Item::fullscreen(app, None)?])?,
            &Submenu::with_items(
                app,
                "Window",
                true,
                &[&Item::minimize(app, None)?, &Item::maximize(app, None)?],
            )?,
        ],
    )
}

/// Script that turns on the UI's scroll measurement (see `src/lib/perf/scrollProbe.ts`):
/// with `GG_PERF_SCROLL=1` the graph scrolls itself top to bottom once the repository has
/// loaded and reports frame times as perf marks; with `GG_PERF_SCROLL=still` it records
/// frame times for as long without scrolling (the control). `None` otherwise.
fn perf_scroll_script(gg_perf_scroll: Option<OsString>) -> Option<&'static str> {
    match gg_perf_scroll?.to_str()? {
        "1" => Some("window.__GG_PROBE__ = {}; window.__GG_PERF_SCROLL__ = true;"),
        "still" => Some("window.__GG_PROBE__ = {}; window.__GG_PERF_SCROLL__ = \"still\";"),
        _ => None,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The core starts after the window and webview are created; the login shell it asks
    // for git's PATH can run meanwhile.
    gg_core::git::runner::prefetch_login_path();
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(app_menu);
    let builder = match perf_scroll_script(std::env::var_os("GG_PERF_SCROLL")) {
        Some(script) => builder.plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("perf-scroll")
                .js_init_script(script)
                .build(),
        ),
        None => builder,
    };
    builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let slot = Arc::new(CoreSlot {
                app: app.handle().clone(),
                core: OnceCell::new(),
            });
            app.manage(slot.clone());
            tauri::async_runtime::spawn(async move {
                let _ = slot.get().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![invoke_core])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_path_prefers_an_existing_argument() {
        let dir = std::env::temp_dir();
        let other = std::env::current_dir().unwrap();
        let missing = OsString::from("/no/such/folder");
        let os = |p: &PathBuf| Some(p.clone().into_os_string());
        assert_eq!(launch_path(os(&dir), os(&other)), Some(dir.clone()));
        assert_eq!(
            launch_path(Some(missing.clone()), os(&other)),
            Some(other.clone())
        );
        // macOS may pass a process serial number instead of a path.
        assert_eq!(launch_path(Some("-psn_0_12345".into()), None), None);
        assert_eq!(launch_path(None, os(&dir)), Some(dir));
        assert_eq!(launch_path(None, Some(missing)), None);
        assert_eq!(launch_path(None, None), None);
    }

    #[test]
    fn perf_scroll_only_with_gg_perf_scroll_1_or_still() {
        let script = perf_scroll_script(Some("1".into())).unwrap();
        assert!(script.contains("__GG_PROBE__") && script.contains("__GG_PERF_SCROLL__ = true"));
        let still = perf_scroll_script(Some("still".into())).unwrap();
        assert!(still.contains("__GG_PROBE__") && still.contains("__GG_PERF_SCROLL__ = \"still\""));
        assert_eq!(perf_scroll_script(None), None);
        assert_eq!(perf_scroll_script(Some("0".into())), None);
        assert_eq!(perf_scroll_script(Some("".into())), None);
    }
}

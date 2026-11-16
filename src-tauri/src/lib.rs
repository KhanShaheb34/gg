//! The desktop app: a Tauri shell around gg-core. The UI reaches the core through one
//! command, `invoke_core(cmd, args)`, and receives its events as Tauri events.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gg_core::{Core, CoreError, ErrorCode, EventSink};

// A release build of the app never carries gg-core's test helpers and hooks. (Test builds of
// the workspace can: cargo unifies the features of gg-core's own dev-dependency on itself.)
#[cfg(not(debug_assertions))]
const _: () = assert!(
    !gg_core::TEST_HOOKS,
    "gg-core's `testutil` feature must not be built into the release app"
);
#[cfg(not(debug_assertions))]
const _: () = assert!(
    !gg_core::TOOL_PATH_HOOK,
    "gg-core's `tool-path` feature must not be built into the release app"
);
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
    // Before the store loads: the settings saved under the app's old name are copied over.
    let config_dir = gg_core::store::config_dir(|| {
        app.path().app_config_dir().map_err(|e| {
            CoreError::new(ErrorCode::Io, "Could not find the app's config folder")
                .with_detail(e.to_string())
        })
    })?;
    let launch = launch_path(std::env::args_os().nth(1), std::env::var_os("GG_OPEN"));
    let core = Core::new(config_dir, launch, Arc::new(TauriSink(app))).await?;
    // The `gg` command: the login shell is asked (in the background) whether it has a `gg`
    // of its own.
    core.start_cli(gg_core::cli::CliSetup {
        shell: std::env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
        ..Default::default()
    });
    // Performance runs only (`--features perf`, see scripts/perf.sh).
    #[cfg(feature = "perf")]
    core.set_perf_log(std::env::var_os("GG_PERF_LOG").map(PathBuf::from));
    // Performance runs only: editors, terminals and the browser are recorded to this file,
    // never started (the walks must reach no program and no web page; Task 13 review I1).
    #[cfg(feature = "perf")]
    if let Some(file) = std::env::var_os("GG_PERF_LAUNCHES").filter(|v| !v.is_empty()) {
        core.record_launches_to(Some(PathBuf::from(file)));
    }
    core.open_startup_repos();
    // Folders the system asked to open before the core was there go to it now; later ones
    // go straight to it (see `deliver_opened`).
    let mut opened = lock(&OPENED);
    opened.core = Some(core.clone());
    core.open_later(std::mem::take(&mut opened.pending));
    drop(opened);
    Ok(core)
}

/// Folders the operating system asked the app to open (`RunEvent::Opened`: Finder's Open
/// With, a folder dropped on the Dock icon, `open -a gg <folder>`), held until the core
/// has started; then the core itself, which queues them for the UI.
struct OpenedQueue {
    pending: Vec<PathBuf>,
    core: Option<Arc<Core>>,
}

static OPENED: Mutex<OpenedQueue> = Mutex::new(OpenedQueue {
    pending: Vec::new(),
    core: None,
});

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Hands `paths` to the core, or keeps them until it starts. Call inside the async runtime.
fn deliver_opened(paths: Vec<PathBuf>) {
    let mut opened = lock(&OPENED);
    match &opened.core {
        Some(core) => core.open_later(paths),
        None => opened.pending.extend(paths),
    }
}

/// The existing folders among `urls` (`file:` URLs only).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn opened_dirs(urls: &[tauri::Url]) -> Vec<PathBuf> {
    urls.iter()
        .filter(|u| u.scheme() == "file")
        .filter_map(|u| u.to_file_path().ok())
        .filter(|p| p.is_dir())
        .collect()
}

/// The folders a second launch of the app (`gg <path>`, which starts a new instance) asked
/// to open, from its arguments (`args`, the program first) and working folder `cwd`: each
/// argument that is an existing folder, a relative one taken from `cwd`. Anything else (a
/// flag, macOS's process serial number, a missing path) is left out.
fn forwarded_paths(args: &[String], cwd: &str) -> Vec<PathBuf> {
    args.iter()
        .skip(1)
        .map(|arg| Path::new(cwd).join(arg))
        .filter(|p| p.is_dir())
        .collect()
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

/// Brings the main window to the front (unminimized, shown, focused).
fn bring_forward<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
async fn invoke_core(
    slot: State<'_, Arc<CoreSlot>>,
    cmd: String,
    args: Value,
) -> Result<Value, CoreError> {
    slot.get().await?.dispatch(&cmd, args).await
}

/// The app menu item that opens the UI's "Install ‘gg’ Command" dialog (the UI listens for
/// the `menu-install-cli` event).
const MENU_INSTALL_CLI: &str = "install-cli";

/// The app menu's "Settings…" (Cmd+,) and Help's "Keyboard Shortcuts": the UI opens them on
/// the `menu-settings` and `menu-shortcuts` events.
const MENU_SETTINGS: &str = "settings";
const MENU_SHORTCUTS: &str = "shortcuts";

/// Windows and Linux: the File menu's "Quit".
#[cfg(not(target_os = "macos"))]
const MENU_QUIT: &str = "quit";

/// The window's menu on Windows and Linux, which have no app menu bar: File with "Settings…"
/// (Ctrl+,), "Install ‘gg’ Command…" and "Quit"; Help with "Keyboard Shortcuts" and "About
/// gg". No item takes Ctrl+W (the UI closes the active tab with it) nor F1 (the UI's own
/// shortcut for the shortcuts sheet).
#[cfg(not(target_os = "macos"))]
fn app_menu<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem as Item, Submenu};

    let name = app.package_info().name.clone();
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(app.package_info().version.to_string()),
        ..Default::default()
    };
    let settings = MenuItem::with_id(app, MENU_SETTINGS, "Settings…", true, Some("CmdOrCtrl+,"))?;
    let install_cli = MenuItem::with_id(
        app,
        MENU_INSTALL_CLI,
        "Install ‘gg’ Command…",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
    let shortcuts = MenuItem::with_id(
        app,
        MENU_SHORTCUTS,
        "Keyboard Shortcuts",
        true,
        None::<&str>,
    )?;
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                "File",
                true,
                &[&settings, &install_cli, &Item::separator(app)?, &quit],
            )?,
            &Submenu::with_items(
                app,
                "Help",
                true,
                &[
                    &shortcuts,
                    &Item::separator(app)?,
                    &Item::about(app, Some(&format!("About {name}")), Some(about))?,
                ],
            )?,
        ],
    )
}

/// The macOS menu bar: Tauri's default with "Settings…" and "Install ‘gg’ Command…" in the
/// app menu and a Help menu with "Keyboard Shortcuts", minus
/// "Close Window", whose Cmd+W would close the whole window; the UI uses Cmd+W to close the
/// active tab instead (the red traffic light still closes the window).
#[cfg(target_os = "macos")]
fn app_menu<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{AboutMetadata, Menu, PredefinedMenuItem as Item, Submenu};

    let name = app.package_info().name.clone();
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(app.package_info().version.to_string()),
        ..Default::default()
    };
    let install_cli = tauri::menu::MenuItem::with_id(
        app,
        MENU_INSTALL_CLI,
        "Install ‘gg’ Command…",
        true,
        None::<&str>,
    )?;
    let settings =
        tauri::menu::MenuItem::with_id(app, MENU_SETTINGS, "Settings…", true, Some("CmdOrCtrl+,"))?;
    let shortcuts = tauri::menu::MenuItem::with_id(
        app,
        MENU_SHORTCUTS,
        "Keyboard Shortcuts",
        true,
        None::<&str>,
    )?;
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
                    &settings,
                    &Item::separator(app)?,
                    &install_cli,
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
            &Submenu::with_items(app, "Help", true, &[&shortcuts])?,
        ],
    )
}

/// Script that turns on the UI's scroll measurement (see `src/lib/perf/scrollProbe.ts`):
/// with `GG_PERF_SCROLL=1` the graph scrolls itself top to bottom once the repository has
/// loaded and reports frame times as perf marks; with `GG_PERF_SCROLL=still` it records
/// frame times for as long without scrolling (the control); with `GG_PERF_SCROLL=smooth` it
/// scrolls a few px every frame, like a trackpad; with `GG_PERF_SCROLL=pages` it loads every
/// page of rows once without scrolling (to tell the memory of loading rows from that of
/// drawing them); with `GG_PERF_SCROLL=jump3` it scrolls top to bottom three times, marking
/// `pass-1`..`pass-3` after each (memory over repeated scrolls); with `GG_PERF_SCROLL=actions`
/// it times right-click, menu, dialog and operation feedback (`actionsProbe.ts`; it creates
/// branches, so only on a throwaway repository); with `GG_PERF_SCROLL=walk` it uses the app
/// through its menus, dialogs, banner and log (`walkProbe.ts`; a throwaway repository made by
/// its driver only); `walk3` and `walk3-dark` use the worktree, commit, pull-request and
/// settings features the same way (the second in the dark theme, set through the settings);
/// `worktrees` times opening the side panel and switching worktrees. Every mode also records the page's errors, unhandled rejections, CSP
/// violations and console errors and warnings in `window.__GG_PAGE_ERRORS__`, which the
/// probe reports as `page-error` marks. `None` otherwise.
#[cfg(any(test, feature = "perf"))]
fn perf_scroll_script(gg_perf_scroll: Option<OsString>) -> Option<String> {
    let mode = match gg_perf_scroll?.to_str()? {
        "1" => "true",
        "still" => "\"still\"",
        "smooth" => "\"smooth\"",
        "pages" => "\"pages\"",
        "jump3" => "\"jump3\"",
        "actions" => "\"actions\"",
        "walk" => "\"walk\"",
        "walk3" => "\"walk3\"",
        "walk3-dark" => "\"walk3-dark\"",
        "worktrees" => "\"worktrees\"",
        _ => return None,
    };
    Some(format!(
        "window.__GG_PROBE__ = {{}}; window.__GG_PERF_SCROLL__ = {mode};\n{PAGE_ERRORS_SCRIPT}"
    ))
}

/// The window's size for a performance run, from `GG_PERF_WINDOW` (`900x560`: logical
/// pixels), so the walk can use the app in the smallest window too. `None` otherwise.
#[cfg(any(test, feature = "perf"))]
fn perf_window_size(gg_perf_window: Option<OsString>) -> Option<(f64, f64)> {
    let value = gg_perf_window?;
    let (w, h) = value.to_str()?.split_once('x')?;
    let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
    (w > 0 && h > 0).then_some((f64::from(w), f64::from(h)))
}

/// Records what goes wrong on the page (see `perf_scroll_script`).
#[cfg(any(test, feature = "perf"))]
const PAGE_ERRORS_SCRIPT: &str = r#"(() => {
  const errors = (window.__GG_PAGE_ERRORS__ = []);
  const note = (kind, text) => errors.push(`${kind}: ${String(text).slice(0, 200)}`);
  addEventListener("error", (e) => note("error", e.message));
  addEventListener("unhandledrejection", (e) => note("unhandled rejection", e.reason));
  document.addEventListener("securitypolicyviolation", (e) =>
    note("csp", `${e.violatedDirective} ${e.blockedURI}`));
  for (const level of ["error", "warn"]) {
    const original = console[level].bind(console);
    console[level] = (...args) => {
      note(`console.${level}`, args.map(String).join(" "));
      original(...args);
    };
  }
})();"#;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // First of all: a second launch (`gg <path>` starts a new instance) hands its folders to
    // the running app and exits here, during plugin set-up, before the app's own set-up, so
    // it never reads or writes the config folder or migrates anything. The running app opens
    // the folders (or shows their tabs) and comes to the front.
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            let paths = forwarded_paths(&args, &cwd);
            if !paths.is_empty() {
                tauri::async_runtime::spawn(async move { deliver_opened(paths) });
            }
            bring_forward(app);
        }))
        // Plugins are set up in order, before the window is made: a second launch has exited
        // in the one above, so only the instance that stays starts the login shell it asks for
        // git's PATH, and it runs while the window and webview are created (started later, in
        // the app's own set-up, it held the first rows back by 100-250 ms).
        .plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("gg-login-path")
                .setup(|_, _| {
                    gg_core::git::runner::prefetch_login_path();
                    Ok(())
                })
                .build(),
        );
    // macOS: the menu bar; Windows and Linux: the window's menu (the same items reach the UI).
    let builder = builder.menu(app_menu).on_menu_event(|app, event| {
        let id = event.id();
        if id == MENU_INSTALL_CLI {
            let _ = app.emit("menu-install-cli", ());
        } else if id == MENU_SETTINGS {
            let _ = app.emit("menu-settings", ());
        } else if id == MENU_SHORTCUTS {
            let _ = app.emit("menu-shortcuts", ());
        }
        #[cfg(not(target_os = "macos"))]
        if id == MENU_QUIT {
            app.exit(0);
        }
    });
    // Performance runs only (`--features perf`, see scripts/perf.sh).
    #[cfg(feature = "perf")]
    let builder = match perf_scroll_script(std::env::var_os("GG_PERF_SCROLL")) {
        Some(script) => builder.plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("perf-scroll")
                .js_init_script(script.as_str())
                .build(),
        ),
        None => builder,
    };
    // Performance runs only: with a perf log, the page marks its start-up phases too
    // (`src/lib/perf/startMarks.ts`), so a launch's time after navigation can be split.
    #[cfg(feature = "perf")]
    let builder = if std::env::var_os("GG_PERF_LOG").is_some_and(|v| !v.is_empty()) {
        builder.plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("perf-start")
                .js_init_script("window.__GG_PERF_START__ = true;")
                .build(),
        )
    } else {
        builder
    };
    builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Only the instance that stays gets here (a second one has exited during plugin
            // set-up, having run nothing). The core starts after the window and webview are
            // created; the login shell it asks for git's PATH is running already (above).
            // Performance runs only: the window at the size the run asks for.
            #[cfg(feature = "perf")]
            if let (Some((w, h)), Some(window)) = (
                perf_window_size(std::env::var_os("GG_PERF_WINDOW")),
                app.get_webview_window("main"),
            ) {
                let _ = window.set_size(tauri::LogicalSize::new(w, h));
            }
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
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = event {
                let paths = opened_dirs(&urls);
                if paths.is_empty() {
                    return;
                }
                tauri::async_runtime::spawn(async move { deliver_opened(paths) });
                // The app may be running behind other windows: bring it forward.
                bring_forward(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opened_urls_give_existing_folders_only() {
        let dir = std::env::temp_dir();
        let file = dir.join("gg-opened-test.txt");
        std::fs::write(&file, "x").unwrap();
        let urls = [
            tauri::Url::from_directory_path(&dir).unwrap(),
            tauri::Url::from_file_path(&file).unwrap(),
            // Absolute on every platform (a bare "/no/such" is not on Windows).
            tauri::Url::from_directory_path(dir.join("gg-no-such-folder-test")).unwrap(),
            tauri::Url::parse("https://example.com/").unwrap(),
        ];
        let dirs = opened_dirs(&urls);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(
            std::fs::canonicalize(&dirs[0]).unwrap(),
            std::fs::canonicalize(&dir).unwrap()
        );
    }

    #[test]
    fn a_second_launch_forwards_its_existing_folders() {
        let dir = std::env::temp_dir().join(format!("gg-forwarded-test-{}", std::process::id()));
        let sub = dir.join("my repo");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.join("file"), "x").unwrap();
        let cwd = dir.to_string_lossy().into_owned();
        let args: Vec<String> = [
            "/Applications/gg.app/Contents/MacOS/gg-desktop",
            "my repo",
            &sub.to_string_lossy(),
            "file",
            "-psn_0_12345",
            "missing",
        ]
        .map(str::to_owned)
        .into();
        let forwarded = forwarded_paths(&args, &cwd);
        let only = forwarded_paths(&args[..1], &cwd);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(forwarded, [sub.clone(), sub]);
        // Without a path it only comes to the front.
        assert!(only.is_empty());
    }

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
    fn perf_scroll_only_with_a_known_gg_perf_scroll() {
        let script = perf_scroll_script(Some("1".into())).unwrap();
        assert!(script.contains("__GG_PROBE__") && script.contains("__GG_PERF_SCROLL__ = true"));
        let still = perf_scroll_script(Some("still".into())).unwrap();
        assert!(still.contains("__GG_PROBE__") && still.contains("__GG_PERF_SCROLL__ = \"still\""));
        let smooth = perf_scroll_script(Some("smooth".into())).unwrap();
        assert!(
            smooth.contains("__GG_PROBE__") && smooth.contains("__GG_PERF_SCROLL__ = \"smooth\"")
        );
        // Every mode also records what goes wrong on the page.
        let three = perf_scroll_script(Some("jump3".into())).unwrap();
        assert!(three.contains("__GG_PERF_SCROLL__ = \"jump3\""));
        for mode in ["1", "still", "smooth", "pages", "jump3"] {
            let script = perf_scroll_script(Some(mode.into())).unwrap();
            assert!(script.contains("__GG_PAGE_ERRORS__"), "{mode}");
            assert!(script.contains("securitypolicyviolation"), "{mode}");
            assert!(script.contains("unhandledrejection"), "{mode}");
            assert!(script.contains("console[level]"), "{mode}");
        }
        let actions = perf_scroll_script(Some("actions".into())).unwrap();
        assert!(actions.contains("__GG_PERF_SCROLL__ = \"actions\""));
        let walk = perf_scroll_script(Some("walk".into())).unwrap();
        assert!(walk.contains("__GG_PERF_SCROLL__ = \"walk\""));
        for mode in ["walk3", "walk3-dark", "worktrees"] {
            let script = perf_scroll_script(Some(mode.into())).unwrap();
            assert!(
                script.contains(&format!("__GG_PERF_SCROLL__ = \"{mode}\"")),
                "{mode}"
            );
        }
        let pages = perf_scroll_script(Some("pages".into())).unwrap();
        assert!(pages.contains("__GG_PERF_SCROLL__ = \"pages\""));
        assert_eq!(perf_scroll_script(None), None);
        assert_eq!(perf_scroll_script(Some("0".into())), None);
        assert_eq!(perf_scroll_script(Some("".into())), None);
    }

    #[test]
    fn perf_window_size_reads_width_x_height() {
        assert_eq!(
            perf_window_size(Some("900x560".into())),
            Some((900.0, 560.0))
        );
        assert_eq!(
            perf_window_size(Some("1280x800".into())),
            Some((1280.0, 800.0))
        );
        for bad in ["", "900", "x560", "900x", "0x560", "900x-1", "big"] {
            assert_eq!(perf_window_size(Some(bad.into())), None, "{bad}");
        }
        assert_eq!(perf_window_size(None), None);
    }
}

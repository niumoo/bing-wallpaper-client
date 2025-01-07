use std::{
    fs::{self, File},
    io::{Write, Read},
    path::PathBuf,
    process::Command,
    thread::{self, JoinHandle},
    time::Duration,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use chrono::Local;
use log::{info, error, warn};
use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder}
};

#[cfg(target_os = "windows")]
use winapi::{
    um::winuser::{SystemParametersInfoA, SPI_SETDESKWALLPAPER, SPIF_UPDATEINIFILE, SPIF_SENDCHANGE},
    shared::minwindef::TRUE,
};

const REFRESH_INTERVAL: u64 = 600; // 10 minutes in seconds
const CHINA_API_URL: &str = "https://bing.wdbyte.com/zh-cn/today";
const GLOBAL_API_URL: &str = "https://bing.wdbyte.com/today";

#[derive(Clone, Copy, PartialEq)]
enum RefreshMode {
    DailyChina,
    DailyGlobal,
    None,
}

struct AppState {
    refresh_mode: RefreshMode,
    timer_handle: Option<(JoinHandle<()>, Arc<AtomicBool>)>,
}

// Error type
#[derive(Debug)]
struct AppError(String);

impl std::error::Error for AppError {}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn get_app_data_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")?;
    let app_dir = PathBuf::from(home).join(".bing-wallpaper-client");
    
    if !app_dir.exists() {
        fs::create_dir_all(&app_dir)?;
        info!("Created app directory: {:?}", app_dir);
    }
    
    Ok(app_dir)
}

fn get_today_wallpaper_path() -> Result<PathBuf> {
    let app_dir = get_app_data_dir()?;
    let date = Local::now().format("%Y-%m-%d");
    Ok(app_dir.join(format!("{}.jpg", date)))
}

fn is_today_wallpaper_exists() -> bool {
    get_today_wallpaper_path().map(|path| path.exists()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn set_wallpaper(path: &str) -> Result<()> {
    let script = format!(
        "tell application \"System Events\" to tell every desktop to set picture to \"{}\"",
        path
    );
    let output = Command::new("osascript")
        .args(&["-e", &script])
        .output()?;

    if output.status.success() {
        info!("Wallpaper set successfully on macOS");
        Ok(())
    } else {
        let error_msg = String::from_utf8_lossy(&output.stderr);
        Err(AppError(format!("Failed to set wallpaper on macOS: {}", error_msg)).into())
    }
}

#[cfg(target_os = "windows")]
fn set_wallpaper(path: &str) -> Result<()> {
    use std::ffi::CString;
    
    let path_cstr = CString::new(path)?;
    
    unsafe {
        if SystemParametersInfoA(
            SPI_SETDESKWALLPAPER,
            0,
            path_cstr.as_ptr() as _,
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        ) == TRUE
        {
            info!("Wallpaper set successfully on Windows");
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().into())
        }
    }
}

fn get_bing_wallpaper_url(is_china: bool) -> Result<String> {
    let api_url = if is_china { CHINA_API_URL } else { GLOBAL_API_URL };
    let response = ureq::get(api_url).call()?;
    let mut url = String::new();
    response.into_reader().read_to_string(&mut url)?;
    Ok(url)
}

fn download_and_set_wallpaper(force: bool, is_china: bool) -> Result<()> {
    if !force && is_today_wallpaper_exists() {
        info!("Today's wallpaper already exists, skipping download");
        return Ok(());
    }

    let wallpaper_path = get_today_wallpaper_path()?;
    let wallpaper_url = get_bing_wallpaper_url(is_china)?;
    
    let response = ureq::get(&wallpaper_url).call()?;
    let mut bytes = Vec::new();
    response.into_reader().read_to_end(&mut bytes)?;

    File::create(&wallpaper_path)?.write_all(&bytes)?;
    
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    set_wallpaper(wallpaper_path.to_str().unwrap())?;

    Ok(())
}

fn create_timer_thread(is_china: bool) -> (JoinHandle<()>, Arc<AtomicBool>) {
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    let handle = thread::spawn(move || {
        while running_clone.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(REFRESH_INTERVAL));
            
            if !running_clone.load(Ordering::Relaxed) {
                break;
            }
            
            if let Err(e) = download_and_set_wallpaper(false, is_china) {
                error!("Failed to update wallpaper: {:?}", e);
            }
        }
    });

    (handle, running)
}

fn update_menu(app: &tauri::AppHandle, tray: &TrayIcon, refresh_mode: RefreshMode) -> Result<()> {
    let new_menu = Menu::with_items(app, &[
        &MenuItem::with_id(
            app,
            "daily_china",
            if refresh_mode == RefreshMode::DailyChina { "每日壁纸刷新(中国) ✓" } else { "每日壁纸刷新(中国)" },
            true,
            None::<&str>,
        )?,
        &MenuItem::with_id(
            app,
            "daily_global",
            if refresh_mode == RefreshMode::DailyGlobal { "每日壁纸刷新(国际) ✓" } else { "每日壁纸刷新(国际)" },
            true,
            None::<&str>,
        )?,
        &MenuItem::with_id(app, "separator", "--------------", false, None::<&str>)?,
        &MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?,
    ])?;
    
    tray.set_menu(Some(new_menu))?;
    Ok(())
}

fn handle_refresh_mode(
    app: &tauri::AppHandle,
    tray: &TrayIcon,
    state: &Mutex<AppState>,
    new_mode: RefreshMode,
    is_china: bool,
) -> Result<()> {
    let mut state = state.lock().unwrap();
    
    // 停止现有计时器
    if let Some((_handle, running)) = state.timer_handle.take() {
        running.store(false, Ordering::Relaxed);
    }

    // 更新模式
    state.refresh_mode = if state.refresh_mode == new_mode {
        RefreshMode::None
    } else {
        new_mode
    };

    // 更新菜单
    update_menu(app, tray, state.refresh_mode)?;

    // 如果切换到新模式，启动定时器
    if state.refresh_mode == new_mode {
        download_and_set_wallpaper(true, is_china)?;
        state.timer_handle = Some(create_timer_thread(is_china));
    }

    Ok(())
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    if let Err(e) = tauri::Builder::default()
        .manage(Mutex::new(AppState {
            refresh_mode: RefreshMode::None,
            timer_handle: None,
        }))
        .setup(|app| {
            let tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&Menu::with_items(app, &[
                    &MenuItem::with_id(app, "daily_china", "每日壁纸刷新(中国)", true, None::<&str>)?,
                    &MenuItem::with_id(app, "daily_global", "每日壁纸刷新(国际)", true, None::<&str>)?,
                    &MenuItem::with_id(app, "separator", "--------------", false, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?,
                ])?)
                .build(app)?;

            let tray_clone = tray.clone();

            tray.on_menu_event(move |app, event| {
                let state = app.state::<Mutex<AppState>>();
                
                match event.id.0.as_str() {
                    "daily_china" => {
                        if let Err(e) = handle_refresh_mode(app, &tray_clone, &state, RefreshMode::DailyChina, true) {
                            error!("Failed to handle China refresh mode: {:?}", e);
                        }
                    }
                    "daily_global" => {
                        if let Err(e) = handle_refresh_mode(app, &tray_clone, &state, RefreshMode::DailyGlobal, false) {
                            error!("Failed to handle Global refresh mode: {:?}", e);
                        }
                    }
                    "quit" => app.exit(0),
                    _ => warn!("Unhandled menu item: {:?}", event.id),
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
    {
        error!("Error running application: {:?}", e);
    }
}
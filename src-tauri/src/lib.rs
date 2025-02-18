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
use log::{info, error, warn};
use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder}
};
use serde::Deserialize;
use uuid::Uuid;

#[cfg(target_os = "windows")]
use winapi::{
    um::winuser::{SystemParametersInfoA, SPI_SETDESKWALLPAPER, SPIF_UPDATEINIFILE, SPIF_SENDCHANGE},
    shared::minwindef::TRUE,
};

const REFRESH_INTERVAL: u64 = 600; // 10分钟
const CHINA_API_URL: &str = "https://bing.wdbyte.com/zh-cn/today";
const GLOBAL_API_URL: &str = "https://bing.wdbyte.com/today";
const UUID_FILE_NAME: &str = "device_uuid.txt";

// 简单的日志实现
static LOGGER: SimpleLogger = SimpleLogger;

struct SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("{} - {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

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

// 简化的错误类型
#[derive(Debug)]
struct AppError(String);

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError(err.to_string())
    }
}

impl From<minreq::Error> for AppError {
    fn from(err: minreq::Error) -> Self {
        AppError(err.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError(err.to_string())
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, Deserialize)]
struct WallpaperInfo {
    file_name: String,
    url: String,
}

fn get_or_create_uuid() -> Result<String> {
    let uuid_path = get_app_data_dir()?.join(UUID_FILE_NAME);
    
    if uuid_path.exists() {
        let mut contents = String::new();
        File::open(uuid_path)?.read_to_string(&mut contents)?;
        Ok(contents.trim().to_string())
    } else {
        let new_uuid = Uuid::new_v4().to_string();
        File::create(&uuid_path)?.write_all(new_uuid.as_bytes())?;
        info!("Created new UUID: {}", new_uuid);
        Ok(new_uuid)
    }
}

fn get_app_data_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    let app_dir = {
        let app_data = std::env::var("APPDATA").map_err(|e| AppError(e.to_string()))?;
        PathBuf::from(app_data).join("bing-wallpaper-client")
    };

    #[cfg(not(windows))]
    let app_dir = {
        let home = std::env::var("HOME").map_err(|e| AppError(e.to_string()))?;
        PathBuf::from(home).join(".bing-wallpaper-client")
    };
    
    if !app_dir.exists() {
        fs::create_dir_all(&app_dir)?;
        info!("Created app directory: {:?}", app_dir);
    }
    
    Ok(app_dir)
}

fn get_wallpaper_path(filename: &str) -> Result<PathBuf> {
    Ok(get_app_data_dir()?.join(filename))
}

fn is_wallpaper_exists(filename: &str) -> bool {
    get_wallpaper_path(filename).map(|path| path.exists()).unwrap_or(false)
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
        Err(AppError(format!("Failed to set wallpaper on macOS: {}", error_msg)))
    }
}

#[cfg(target_os = "windows")]
fn set_wallpaper(path: &str) -> Result<()> {
    use std::ffi::CString;
    
    let path_cstr = CString::new(path).map_err(|e| AppError(e.to_string()))?;
    
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
            Err(AppError("Failed to set wallpaper on Windows".to_string()))
        }
    }
}

fn get_bing_wallpaper_info(is_china: bool) -> Result<WallpaperInfo> {
    let api_url = if is_china { CHINA_API_URL } else { GLOBAL_API_URL };
    
    // 获取UUID
    let uuid = get_or_create_uuid()?;
    
    let response = minreq::get(api_url)
        .with_header("client-version", "0.1.0")
        .with_header("client-device-uuid", &uuid)
        .send()?;
    
    let content = response.as_str().map_err(|e| AppError(e.to_string()))?;
    Ok(serde_json::from_str(content)?)
}

fn download_and_set_wallpaper(force: bool, is_china: bool) -> Result<()> {
    let wallpaper_info = get_bing_wallpaper_info(is_china)?;
    
    if !force && is_wallpaper_exists(&wallpaper_info.file_name) {
        info!("Wallpaper {} already exists, skipping download", wallpaper_info.file_name);
        return Ok(());
    }

    let wallpaper_path = get_wallpaper_path(&wallpaper_info.file_name)?;
    
    let response = minreq::get(&wallpaper_info.url).send()?;
    let bytes = response.into_bytes();

    File::create(&wallpaper_path)?.write_all(&bytes)?;
    
    info!("Downloaded wallpaper: {}", wallpaper_info.file_name);
    
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
                error!("Failed to update wallpaper: {}", e);
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
        ).map_err(|e| AppError(e.to_string()))?,
        &MenuItem::with_id(
            app,
            "daily_global",
            if refresh_mode == RefreshMode::DailyGlobal { "每日壁纸刷新(国际) ✓" } else { "每日壁纸刷新(国际)" },
            true,
            None::<&str>,
        ).map_err(|e| AppError(e.to_string()))?,
        &MenuItem::with_id(app, "separator1", "--------------", false, None::<&str>)
            .map_err(|e| AppError(e.to_string()))?,
        &MenuItem::with_id(app, "open_website", "打开必应壁纸网站", true, None::<&str>)
            .map_err(|e| AppError(e.to_string()))?,
        &MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
            .map_err(|e| AppError(e.to_string()))?,
    ]).map_err(|e| AppError(e.to_string()))?;
    
    tray.set_menu(Some(new_menu)).map_err(|e| AppError(e.to_string()))?;
    Ok(())
}

fn handle_refresh_mode(
    app: &tauri::AppHandle,
    tray: &TrayIcon,
    state: &Mutex<AppState>,
    new_mode: RefreshMode,
    is_china: bool,
) -> Result<()> {
    let mut state = state.lock().map_err(|_| AppError("Failed to lock state".to_string()))?;
    
    if let Some((_handle, running)) = state.timer_handle.take() {
        running.store(false, Ordering::Relaxed);
    }

    state.refresh_mode = if state.refresh_mode == new_mode {
        RefreshMode::None
    } else {
        new_mode
    };

    update_menu(app, tray, state.refresh_mode)?;

    if state.refresh_mode == new_mode {
        download_and_set_wallpaper(true, is_china)?;
        state.timer_handle = Some(create_timer_thread(is_china));
    }

    Ok(())
}

pub fn run() {
    // 初始化日志
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Info);

    // 启动时确保UUID已经生成
    match get_or_create_uuid() {
        Ok(uuid) => info!("Using device UUID: {}", uuid),
        Err(e) => error!("Failed to initialize UUID: {}", e),
    }

    if let Err(e) = tauri::Builder::default()
        .manage(Mutex::new(AppState {
            refresh_mode: RefreshMode::None,
            timer_handle: None,
        }))
        .setup(|app| {
            // 在 macOS 托盘中隐藏
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            #[cfg(target_os = "windows")]
            {
                use tauri::WindowBuilder;
                // 创建一个隐藏的主窗口
                WindowBuilder::new(
                    app,
                    "main", /* 这是窗口的唯一标识符 */
                    tauri::WindowUrl::default(),
                )
                .title("Bing Wallpaper")
                .visible(false)
                .skip_taskbar(true)
                .build()?;
            }

            let tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&Menu::with_items(app, &[
                    &MenuItem::with_id(app, "daily_china", "每日壁纸刷新(中国)", true, None::<&str>)?,
                    &MenuItem::with_id(app, "daily_global", "每日壁纸刷新(国际)", true, None::<&str>)?,
                    &MenuItem::with_id(app, "separator1", "--------------", false, None::<&str>)?,
                    &MenuItem::with_id(app, "open_website", "打开必应壁纸网站", true, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?,
                ])?)
                .build(app)?;

            let tray_clone = tray.clone();

            tray.on_menu_event(move |app, event| {
                let state = app.state::<Mutex<AppState>>();
                
                match event.id.0.as_str() {
                    "daily_china" => {
                        if let Err(e) = handle_refresh_mode(app, &tray_clone, &state, RefreshMode::DailyChina, true) {
                            error!("Failed to handle China refresh mode: {}", e);
                        }
                    }
                    "daily_global" => {
                        if let Err(e) = handle_refresh_mode(app, &tray_clone, &state, RefreshMode::DailyGlobal, false) {
                            error!("Failed to handle Global refresh mode: {}", e);
                        }
                    }
                    "open_website" => {
                        if let Err(e) = open::that("https://bing.wdbyte.com") {
                            error!("Failed to open website: {}", e);
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
        error!("Error running application: {}", e);
    }
}
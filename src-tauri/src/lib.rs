use std::{
    fs::{self, File},
    io::{Write, Read},
    path::PathBuf,
    process::Command,
    thread::{self, JoinHandle},
    time::Duration,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use chrono::Local;
use log::{info, error, warn};
use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};

#[cfg(target_os = "windows")]
use winapi::{
    um::winuser::{SystemParametersInfoA, SPI_SETDESKWALLPAPER, SPIF_UPDATEINIFILE, SPIF_SENDCHANGE},
    shared::minwindef::TRUE,
};

// 定义刷新模式枚举
#[derive(Clone, Copy, PartialEq)]
enum RefreshMode {
    Daily,
    Random,
    None,
}

// 使用 Mutex 来存储当前的刷新模式
struct AppState {
    refresh_mode: RefreshMode,
    timer_handle: Option<(JoinHandle<()>, Arc<AtomicBool>)>,
}

// 获取应用数据目录
fn get_app_data_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home = std::env::var("HOME")?;
    let app_dir = PathBuf::from(home).join(".bing-wallpaper-client");
    
    if !app_dir.exists() {
        fs::create_dir_all(&app_dir)?;
        info!("Created app directory: {:?}", app_dir);
    }
    
    Ok(app_dir)
}

// 获取今日壁纸路径
fn get_today_wallpaper_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let app_dir = get_app_data_dir()?;
    let date = Local::now().format("%Y-%m-%d");
    Ok(app_dir.join(format!("{}.jpg", date)))
}

// 检查今日壁纸是否已存在
fn is_today_wallpaper_exists() -> bool {
    match get_today_wallpaper_path() {
        Ok(path) => path.exists(),
        Err(e) => {
            error!("Failed to check wallpaper existence: {:?}", e);
            false
        }
    }
}

// 设置壁纸的平台特定实现
#[cfg(target_os = "macos")]
fn set_wallpaper(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("osascript")
        .args(&[
            "-e",
            &format!(
                "tell application \"System Events\" to tell every desktop to set picture to \"{}\"",
                path
            ),
        ])
        .output()?;

    if output.status.success() {
        info!("Wallpaper set successfully on macOS");
        Ok(())
    } else {
        let error_msg = String::from_utf8_lossy(&output.stderr);
        error!("Failed to set wallpaper on macOS: {}", error_msg);
        Err(error_msg.into())
    }
}

#[cfg(target_os = "windows")]
fn set_wallpaper(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::ffi::CString;
    
    info!("Setting wallpaper on Windows: {}", path);
    
    // 将路径转换为 CString
    let path_cstr = CString::new(path)?;
    
    unsafe {
        let result = SystemParametersInfoA(
            SPI_SETDESKWALLPAPER,
            0,
            path_cstr.as_ptr() as _,
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        );
        
        if result == TRUE {
            info!("Wallpaper set successfully on Windows");
            Ok(())
        } else {
            let error = std::io::Error::last_os_error();
            error!("Failed to set wallpaper on Windows: {:?}", error);
            Err(error.into())
        }
    }
}

// 下载和设置壁纸的函数
fn download_and_set_wallpaper(force: bool) -> Result<(), Box<dyn std::error::Error>> {
    // 只有在非强制模式下才检查今日壁纸是否存在
    if !force && is_today_wallpaper_exists() {
        info!("Today's wallpaper already exists, skipping download");
        return Ok(());
    }

    let wallpaper_path = get_today_wallpaper_path()?;
    info!("Downloading wallpaper to: {:?}", wallpaper_path);

    // 下载图片
    let response = ureq::get("https://cn.bing.com/th?id=OHR.VietnamFalls_EN-US9133406245_UHD.jpg&pid=hp&w=1920")
        .call()?;
    
    let mut bytes: Vec<u8> = Vec::new();
    response.into_reader().read_to_end(&mut bytes)?;

    // 保存图片
    let mut file = File::create(&wallpaper_path)?;
    file.write_all(&bytes)?;
    info!("Wallpaper downloaded successfully");

    // 设置壁纸
    #[cfg(target_os = "windows")]
    {
        // Windows 需要绝对路径
        let abs_path = wallpaper_path.canonicalize()?;
        set_wallpaper(abs_path.to_str().unwrap())?;
    }
    
    #[cfg(target_os = "macos")]
    {
        set_wallpaper(wallpaper_path.to_str().unwrap())?;
    }

    Ok(())
}

pub fn run() {
    // 初始化日志
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    info!("Starting Bing Wallpaper Client");

    // 确保应用目录存在
    if let Err(e) = get_app_data_dir() {
        error!("Failed to create app directory: {:?}", e);
        return;
    }

    tauri::Builder::default()
        .manage(std::sync::Mutex::new(AppState {
            refresh_mode: RefreshMode::None,
            timer_handle: None,
        }))
        .setup(|app| {
            info!("Setting up application");
            
            // 创建菜单项
            let daily_item = MenuItem::with_id(
                app,
                "daily",
                "每日壁纸刷新",
                true,
                None::<&str>,
            )?;
            
            let random_item = MenuItem::with_id(
                app,
                "random",
                "随机壁纸刷新",
                true,
                None::<&str>,
            )?;
            
            let separator = MenuItem::with_id(
                app,
                "separator",
                "--------------",
                false,
                None::<&str>,
            )?;
            
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            
            // 创建菜单
            let menu = Menu::with_items(app, &[
                &daily_item,
                &random_item,
                &separator,
                &quit_item,
            ])?;
            
            // 创建托盘图标
            let tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(true)
                .build(app)?;

            info!("System tray initialized");

            // 克隆 tray 用于闭包
            let tray_clone = tray.clone();

            // 监听菜单事件
            tray.on_menu_event(move |app, event| {
                let state = app.state::<std::sync::Mutex<AppState>>();
                
                match event.id.0.as_str() {
                    "daily" => {
                        let mut state = state.lock().unwrap();
                        let was_daily = state.refresh_mode == RefreshMode::Daily;
                        
                        // 更新状态
                        state.refresh_mode = if was_daily {
                            RefreshMode::None
                        } else {
                            RefreshMode::Daily
                        };
                        
                        // 创建新菜单
                        let new_menu = Menu::with_items(app, &[
                            &MenuItem::with_id(
                                app,
                                "daily",
                                if !was_daily { "每日壁纸刷新 ✓" } else { "每日壁纸刷新" },
                                true,
                                None::<&str>,
                            ).unwrap(),
                            &MenuItem::with_id(
                                app,
                                "random",
                                "随机壁纸刷新",
                                true,
                                None::<&str>,
                            ).unwrap(),
                            &MenuItem::with_id(
                                app,
                                "separator",
                                "--------------",
                                false,
                                None::<&str>,
                            ).unwrap(),
                            &MenuItem::with_id(app, "quit", "退出", true, None::<&str>).unwrap(),
                        ]).unwrap();
                        
                        // 更新菜单
                        if let Err(e) = tray_clone.set_menu(Some(new_menu)) {
                            error!("Failed to update menu: {:?}", e);
                        }
                        if !was_daily {
                            info!("Enabling daily wallpaper refresh");
                            // 手动启用时强制更新壁纸
                            if let Err(e) = download_and_set_wallpaper(true) {
                                error!("Failed to set wallpaper: {:?}", e);
                            }
                        
                            // 创建一个原子布尔值用于控制线程
                            let running = Arc::new(AtomicBool::new(true));
                            let running_clone = running.clone();
                        
                            // 创建新线程
                            let handle = thread::spawn(move || {
                                while running_clone.load(Ordering::Relaxed) {
                                    thread::sleep(Duration::from_secs(600)); // 10分钟
                                    
                                    if !running_clone.load(Ordering::Relaxed) {
                                        info!("Timer thread stopping");
                                        break;
                                    }
                                    
                                    // 定时器运行时不强制更新
                                    match download_and_set_wallpaper(false) {
                                        Ok(_) => info!("Wallpaper check/update completed"),
                                        Err(e) => error!("Failed to update wallpaper: {:?}", e),
                                    }
                                }
                            });
                        
                            // 保存线程句柄和控制标志
                            state.timer_handle = Some((handle, running));
                        } else {
                            info!("Disabling daily wallpaper refresh");
                            // 停止定时器线程
                            if let Some((_handle, running)) = state.timer_handle.take() {
                                running.store(false, Ordering::Relaxed);
                                info!("Timer thread signaled to stop");
                            }
                        }  
    
                    }
                    "random" => {
                        let mut state = state.lock().unwrap();
                        let was_random = state.refresh_mode == RefreshMode::Random;
                        
                        // 更新状态
                        state.refresh_mode = if was_random {
                            RefreshMode::None
                        } else {
                            RefreshMode::Random
                        };
                        
                        // 创建新菜单
                        let new_menu = Menu::with_items(app, &[
                            &MenuItem::with_id(
                                app,
                                "daily",
                                "每日壁纸刷新",
                                true,
                                None::<&str>,
                            ).unwrap(),
                            &MenuItem::with_id(
                                app,
                                "random",
                                if !was_random { "随机壁纸刷新 ✓" } else { "随机壁纸刷新" },
                                true,
                                None::<&str>,
                            ).unwrap(),
                            &MenuItem::with_id(
                                app,
                                "separator",
                                "--------------",
                                false,
                                None::<&str>,
                            ).unwrap(),
                            &MenuItem::with_id(app, "quit", "退出", true, None::<&str>).unwrap(),
                        ]).unwrap();
                        
                        // 更新菜单
                        if let Err(e) = tray_clone.set_menu(Some(new_menu)) {
                            error!("Failed to update menu: {:?}", e);
                        }
                        
                        if !was_random {
                            info!("Enabling random wallpaper refresh");
                        } else {
                            info!("Disabling random wallpaper refresh");
                        }
                    }
                    "quit" => {
                        info!("Exiting application");
                        app.exit(0);
                    }
                    _ => {
                        warn!("Unhandled menu item: {:?}", event.id);
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用时出错");
}

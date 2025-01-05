use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
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
}

pub fn run() {
    tauri::Builder::default()
        .manage(std::sync::Mutex::new(AppState {
            refresh_mode: RefreshMode::None,
        }))
        .setup(|app| {
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
            
            // 创建分隔符
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
                            println!("更新菜单失败: {:?}", e);
                        }
                        
                        if !was_daily {
                            println!("启用每日壁纸刷新");
                            // 在这里添加每日壁纸刷新的具体逻辑
                        } else {
                            println!("禁用每日壁纸刷新");
                            // 在这里添加禁用每日壁纸刷新的具体逻辑
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
                            println!("更新菜单失败: {:?}", e);
                        }
                        
                        if !was_random {
                            println!("启用随机壁纸刷新");
                            // 在这里添加随机壁纸刷新的具体逻辑
                        } else {
                            println!("禁用随机壁纸刷新");
                            // 在这里添加禁用随机壁纸刷新的具体逻辑
                        }
                    }
                    "quit" => {
                        println!("退出应用程序");
                        app.exit(0);
                    }
                    _ => {
                        println!("未处理的菜单项: {:?}", event.id);
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用时出错");
}

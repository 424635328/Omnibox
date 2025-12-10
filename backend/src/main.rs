#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod models;
mod scanner;
mod storage;

use crate::models::{AppSettings, SearchResult, UserHabits};
use crate::storage::Storage;
use auto_launch::AutoLaunchBuilder;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use once_cell::sync::Lazy;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{
    CustomMenuItem, GlobalShortcutManager, Manager, SystemTray, SystemTrayEvent, SystemTrayMenu,
    Window,
};

struct AppState {
    apps: Vec<SearchResult>,
    habits: UserHabits,
    settings: AppSettings,
    storage: Storage,
}

// 线程安全的全局状态
static APP_STATE: Lazy<Arc<Mutex<AppState>>> = Lazy::new(|| {
    let storage = Storage::new();
    let apps = storage.load_apps();
    let habits = storage.load_habits();
    let settings = storage.load_settings();

    Arc::new(Mutex::new(AppState {
        apps,
        habits,
        settings,
        storage,
    }))
});

// 辅助函数：安全获取锁（防止 PoisonError 导致崩溃）
fn get_state_lock() -> std::sync::MutexGuard<'static, AppState> {
    match APP_STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("Warning: Mutex was poisoned. Recovering...");
            poisoned.into_inner()
        }
    }
}

#[tauri::command]
fn search(query: String) -> Vec<SearchResult> {
    let state = get_state_lock();
    let query_trim = query.trim();
    let max_results = state.settings.max_results;

    // 空搜索返回历史记录
    if query_trim.is_empty() {
        let mut recent: Vec<SearchResult> = state
            .apps
            .iter()
            .filter(|a| a.use_count > 0)
            .cloned()
            .collect();
        recent.sort_by(|a, b| b.use_count.cmp(&a.use_count));
        return recent.into_iter().take(max_results).collect();
    }

    let matcher = SkimMatcherV2::default();
    let mut results: Vec<SearchResult> = state
        .apps
        .iter()
        .filter_map(|item| {
            // 标题匹配
            let score_title = matcher.fuzzy_match(&item.title, query_trim).unwrap_or(0);
            
            // 文件名匹配 (处理路径中的文件名)
            let path = Path::new(&item.id);
            let filename = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            let score_filename = matcher.fuzzy_match(&filename, query_trim).unwrap_or(0);
            
            let fuzzy_score = std::cmp::max(score_title, score_filename);

            if fuzzy_score > 0 {
                let mut new_item = item.clone();
                // 习惯权重 + 历史使用权重 + 模糊匹配分数
                let habit_score = state.habits.get_weight(query_trim, &item.id);
                let global_weight = item.use_count as i64 * 5;
                
                new_item.score = fuzzy_score + global_weight + habit_score;
                Some(new_item)
            } else {
                None
            }
        })
        .collect();

    // 按分数降序
    results.sort_by(|a, b| b.score.cmp(&a.score));
    results.into_iter().take(max_results).collect()
}

#[tauri::command]
fn get_settings() -> AppSettings {
    let state = get_state_lock();
    state.settings.clone()
}

fn handle_autostart(enable: bool) {
    if let Ok(current_exe) = std::env::current_exe() {
        // 获取绝对路径
        let path_buf = current_exe.canonicalize().unwrap_or(current_exe.clone());
        let mut path_str = path_buf.to_str().unwrap_or("");
        
        // Windows UNC 路径处理 (\\?\)
        #[cfg(target_os = "windows")]
        {
            path_str = path_str.trim_start_matches(r"\\?\");
        }

        let auto = AutoLaunchBuilder::new()
            .set_app_name("Omnibox")
            .set_app_path(path_str)
            .set_use_launch_agent(false)
            .build();

        if let Ok(auto) = auto {
            if enable {
                if !auto.is_enabled().unwrap_or(false) {
                    let _ = auto.enable();
                }
            } else {
                if auto.is_enabled().unwrap_or(false) {
                    let _ = auto.disable();
                }
            }
        }
    }
}

#[tauri::command]
async fn save_settings(new_settings: AppSettings) -> Result<(), String> {
    handle_autostart(new_settings.enable_autostart);

    let mut state = get_state_lock();
    state.settings = new_settings;
    state.storage.save_settings(&state.settings);
    
    Ok(())
}

#[tauri::command]
fn execute_item(id: String, query: String) {
    // 1. 在主线程更新内存状态（快速）
    {
        let mut state = get_state_lock();
        if !query.trim().is_empty() {
            state.habits.record(&query, &id);
            // 这里可以立即保存习惯，因为数据量小
            state.storage.save_habits(&state.habits);
        }
        
        if let Some(item) = state.apps.iter_mut().find(|a| a.id == id) {
            item.use_count += 1;
            item.last_used = Some(chrono::Utc::now());
        }
    }

    // 2. 在子线程执行打开操作和繁重的保存操作
    let id_clone = id.clone();
    std::thread::spawn(move || {
        // 打开文件/程序
        if let Err(e) = open::that_detached(&id_clone) {
            eprintln!("Failed to open item: {}", e);
        }

        // 保存应用列表（包含更新后的 use_count）
        // 注意：如果列表很大，频繁保存会有 IO 压力，但为了数据一致性暂时保留
        let state = get_state_lock();
        state.storage.save_apps(&state.apps);
    });
}

#[tauri::command]
fn refresh_index() {
    std::thread::spawn(|| {
        println!("Starting background scan...");
        let start = std::time::Instant::now();
        
        // 耗时操作：扫描文件（不持有锁）
        let new_apps = scanner::scan_applications();
        let duration = start.elapsed();
        
        // 快速操作：合并数据（持有锁）
        let mut state = get_state_lock();
        
        // 保留旧数据的统计信息 (use_count, last_used)
        // 创建一个旧数据的查找表加速匹配
        let old_stats: std::collections::HashMap<String, (u32, Option<chrono::DateTime<chrono::Utc>>)> = 
            state.apps.iter()
                .map(|a| (a.id.clone(), (a.use_count, a.last_used)))
                .collect();

        let mut merged_apps = new_apps;
        for app in &mut merged_apps {
            if let Some((count, last_used)) = old_stats.get(&app.id) {
                app.use_count = *count;
                app.last_used = *last_used;
            }
        }

        state.apps = merged_apps;
        state.storage.save_apps(&state.apps);
        
        println!("Index refreshed in {:.2?}. Found {} apps.", duration, state.apps.len());
    });
}

#[tauri::command]
fn quit_app() {
    std::process::exit(0);
}

fn toggle_window(window: &Window) {
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        // 先居中再显示，体验更好
        let _ = window.center();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("window-focused", {});
    }
}

fn main() {
    let quit = CustomMenuItem::new("quit".to_string(), "Quit");
    let tray_menu = SystemTrayMenu::new().add_item(quit);
    let system_tray = SystemTray::new().with_menu(tray_menu);

    tauri::Builder::default()
        .system_tray(system_tray)
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::MenuItemClick { id, .. } => {
                if id == "quit" {
                    std::process::exit(0);
                }
            }
            SystemTrayEvent::LeftClick { .. } => {
                let window = app.get_window("main").unwrap();
                toggle_window(&window);
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            search,
            execute_item,
            refresh_index,
            quit_app,
            get_settings,
            save_settings
        ])
        .setup(|app| {
            let window = app.get_window("main").unwrap();
            
            // 注册全局快捷键
            let mut shortcut = app.global_shortcut_manager();
            let w_clone = window.clone();
            if let Err(e) = shortcut.register("Alt+Space", move || toggle_window(&w_clone)) {
                eprintln!("Failed to register shortcut: {}", e);
            }

            // 失去焦点自动隐藏
            let w_clone2 = window.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::Focused(focused) = event {
                    if !focused && w_clone2.is_visible().unwrap_or(false) {
                        let _ = w_clone2.hide();
                    }
                }
            });

            // 启动时初始化
            refresh_index();
            
            // 延时处理自启动逻辑，避免阻塞启动
            std::thread::spawn(move || {
                let state = get_state_lock();
                let enable = state.settings.enable_autostart;
                drop(state); // 显式释放锁
                handle_autostart(enable);
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
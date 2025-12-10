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

// ==========================================
// 智能搜索算法
// ==========================================
#[tauri::command]
fn search(query: String) -> Vec<SearchResult> {
    let state = get_state_lock();
    let query = query.trim().to_lowercase();
    let max_results = state.settings.max_results;

    // 1. 空搜索：返回最常用的
    if query.is_empty() {
        let mut recent: Vec<SearchResult> = state
            .apps
            .iter()
            .filter(|a| a.use_count > 0)
            .cloned()
            .collect();
        // 按使用次数降序
        recent.sort_by(|a, b| b.use_count.cmp(&a.use_count));
        return recent.into_iter().take(max_results).collect();
    }

    let matcher = SkimMatcherV2::default();
    
    let mut results: Vec<SearchResult> = state
        .apps
        .iter()
        .filter_map(|item| {
            let mut score = 0i64;
            let mut matched = false;

            // A. 基础 Fuzzy 匹配 (英文)
            // ----------------------------------------------------
            if let Some(fuzzy_score) = matcher.fuzzy_match(&item.title, &query) {
                score += fuzzy_score;
                matched = true;
            }

            // B. 中文拼音 / 首字母缩写匹配
            // ----------------------------------------------------
            if !matched || score < 50 {
                // 如果标题拼音包含查询 (e.g., "weixin" contains "wx")
                if item.title_pinyin.contains(&query) {
                    score += 80;
                    matched = true;
                } 
                // 或者首字母包含 (e.g., "wx" contains "wx")
                else if item.title_acronym.contains(&query) {
                    score += 100;
                    matched = true;
                }
            }

            if !matched {
                return None;
            }

            // C. 智能加权 (Heuristics)
            // ----------------------------------------------------
            let title_lower = item.title.to_lowercase();

            // 1. 完全匹配奖励 (Exact Match)
            if title_lower == query || item.title_acronym == query {
                score += 1000;
            }
            // 2. 前缀匹配奖励 (Starts With) - "code" 匹配 "Code.exe" 优于 "VS Code"
            else if title_lower.starts_with(&query) || item.title_pinyin.starts_with(&query) {
                score += 200;
            }

            // 3. 历史记录权重 (最重要!)
            let habit_score = state.habits.get_weight(&query, &item.id);
            score += habit_score;

            // 4. 通用热度加成
            score += (item.use_count as i64) * 20;

            // 5. 长度惩罚 (Length Penalty)
            // 名字越短通常越精确。 "Calc" 比 "OpenOffice Calc" 更好。
            score -= item.title.len() as i64 * 2;

            // 6. 路径/文件名 兜底匹配
            // 如果标题没匹配上，但文件名匹配上了 (e.g. 标题是"微信", 搜"WeChat.exe")
            let path = Path::new(&item.id);
            if let Some(filename) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) {
                 if filename.contains(&query) {
                     score += 50;
                 }
            }

            let mut new_item = item.clone();
            new_item.score = score;
            Some(new_item)
        })
        .collect();

    // D. 排序：分数高在前
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
        let path_buf = current_exe.canonicalize().unwrap_or(current_exe.clone());
        let mut path_str = path_buf.to_str().unwrap_or("");
        
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
    // 1. 更新内存状态 (快速)
    {
        let mut state = get_state_lock();
        if !query.trim().is_empty() {
            state.habits.record(&query, &id);
            state.storage.save_habits(&state.habits);
        }
        
        if let Some(item) = state.apps.iter_mut().find(|a| a.id == id) {
            item.use_count += 1;
            item.last_used = Some(chrono::Utc::now());
        }
    }

    // 2. 异步执行和重写应用缓存 (慢速)
    let id_clone = id.clone();
    std::thread::spawn(move || {
        if let Err(e) = open::that_detached(&id_clone) {
            eprintln!("Failed to open item: {}", e);
        }
        // 更新缓存中的 use_count
        let state = get_state_lock();
        state.storage.save_apps(&state.apps);
    });
}

#[tauri::command]
fn refresh_index() {
    std::thread::spawn(|| {
        println!("Starting background scan...");
        let start = std::time::Instant::now();
        
        // 耗时扫描 (无锁)
        let new_apps = scanner::scan_applications();
        let duration = start.elapsed();
        
        // 合并数据 (有锁)
        let mut state = get_state_lock();
        
        // 保留旧数据的统计信息
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
            
            let mut shortcut = app.global_shortcut_manager();
            let w_clone = window.clone();
            if let Err(e) = shortcut.register("Alt+Space", move || toggle_window(&w_clone)) {
                eprintln!("Failed to register shortcut: {}", e);
            }

            let w_clone2 = window.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::Focused(focused) = event {
                    if !focused && w_clone2.is_visible().unwrap_or(false) {
                        let _ = w_clone2.hide();
                    }
                }
            });
            
            // 启动时自动扫描
            refresh_index();
            
            // 延时处理自启动
            std::thread::spawn(move || {
                let state = get_state_lock();
                let enable = state.settings.enable_autostart;
                drop(state);
                handle_autostart(enable);
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
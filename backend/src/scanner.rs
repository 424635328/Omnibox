use crate::models::SearchResult;
use jwalk::{DirEntry, WalkDir};
use pinyin::ToPinyin;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use winreg::enums::*;
#[cfg(target_os = "windows")]
use winreg::RegKey;

// ==========================================
// 1. 更加智能的黑名单过滤
// ==========================================

// 文件夹黑名单：增加更多开发和系统缓存目录
const CRITICAL_FOLDER_BLACKLIST: &[&str] = &[
    "node_modules", "bower_components", "target", "build", "dist", "vendor", // 开发相关
    ".git", ".svn", ".hg", ".idea", ".vscode", ".settings", // 版本控制与IDE
    "__pycache__", "site-packages", "gems", "cargo", // 语言包库
    "$recycle.bin", "system volume information", "msocache", "config.msi", // Windows 系统
    "windows", "programdata", "perflogs", // 避免直接扫描这些巨无霸目录（我们通过StartMenu覆盖了）
];

// 文件名关键词黑名单：过滤掉卸载程序、帮助文档、升级程序
const FILENAME_NOISE_KEYWORDS: &[&str] = &[
    "uninstall", "uninst", "setup", "install", "update", "helper", 
    "config", "readme", "license", "eula", "vcredist", "dxsetup"
];

fn is_critical_garbage_folder(name: &str) -> bool {
    // 忽略大小写比较
    CRITICAL_FOLDER_BLACKLIST.iter().any(|&bad| name.eq_ignore_ascii_case(bad))
}

fn is_garbage_path(path_str: &str) -> bool {
    // 路径过长通常是自动生成的乱七八糟的东西
    if path_str.len() > 260 { return true; } 
    false
}

// 判断是否为噪音文件（如 uninstall.exe）
fn is_noise_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    FILENAME_NOISE_KEYWORDS.iter().any(|&bad| lower.contains(bad))
}

fn is_launchable(entry: &DirEntry<((), ())>) -> bool {
    if !entry.file_type().is_file() { return false; }
    
    let path = entry.path();
    // 1. 扩展名检查
    let ext = match path.extension().and_then(OsStr::to_str) {
        Some(e) => e.to_ascii_lowercase(),
        None => return false,
    };

    let is_valid_ext = match ext.as_str() {
        #[cfg(target_os = "windows")]
        "lnk" | "exe" | "bat" | "cmd" | "com" | "msc" => true,
        #[cfg(target_os = "macos")]
        "app" | "prefPane" => true, 
        #[cfg(target_os = "linux")]
        "desktop" | "sh" | "AppImage" => true,
        _ => false 
    };

    if !is_valid_ext { return false; }

    // 2. 噪音文件检查 (仅针对非快捷方式)
    // 如果是 .lnk 快捷方式，通常是用户特意创建的，不应该被过滤
    if ext != "lnk" {
        if let Some(stem) = path.file_stem().and_then(OsStr::to_str) {
            if is_noise_file(stem) { return false; }
        }
    }

    true
}

// ==========================================
// 2. 辅助工具 (拼音生成与名称清洗)
// ==========================================

fn get_file_type_display(ext: &str) -> String {
    match ext {
        "lnk" => "Shortcut",
        "exe" | "app" | "AppImage" => "Application",
        "bat" | "cmd" | "sh" => "Script",
        "msc" => "System Tool",
        _ => "File",
    }.to_string()
}

fn clean_filename(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(" - Shortcut", "")
        .replace(" - 快捷方式", "")
        .replace("_", " ")
        .trim()
        .to_string()
}

fn generate_pinyin_data(name: &str) -> (String, String) {
    let mut full = String::with_capacity(name.len() * 2);
    let mut abbr = String::with_capacity(name.len());
    // 优化：只转换中文字符，英文字符直接追加，提升性能
    for c in name.chars() {
        if c.is_ascii() {
            let lower = c.to_ascii_lowercase();
            full.push(lower);
            abbr.push(lower);
        } else if let Some(p) = c.to_pinyin() {
            let plain = p.plain();
            full.push_str(plain);
            if let Some(first) = plain.chars().next() {
                abbr.push(first);
            }
        } else {
            // 处理其他语言或符号
            let lower = c.to_lowercase().to_string();
            full.push_str(&lower);
            abbr.push_str(&lower);
        }
    }
    (full, abbr)
}

// ==========================================
// 3. 核心改进：PATH 环境变量与注册表
// ==========================================

// 【新增】获取 PATH 环境变量中的路径
// 这是找到 git, node, python 等开发工具的关键
fn get_path_env_dirs() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path_var) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_var) {
            if path.exists() {
                paths.push(path);
            }
        }
    }
    paths
}

#[cfg(target_os = "windows")]
fn get_registry_installed_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let hives = [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER];
    let keys = [
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ];
    for hive in hives {
        let root = RegKey::predef(hive);
        for subkey in keys {
            if let Ok(list) = root.open_subkey(subkey) {
                for app_name in list.enum_keys().filter_map(|x| x.ok()) {
                    if let Ok(app) = list.open_subkey(&app_name) {
                        // 很多程序会在 InstallLocation 字段记录安装路径
                        let loc: String = app.get_value("InstallLocation").unwrap_or_default();
                        if !loc.is_empty() {
                            let p = PathBuf::from(&loc);
                            if p.exists() { paths.push(p); }
                        }
                    }
                }
            }
        }
    }
    paths
}

// ==========================================
// 4. 扫描逻辑主体
// ==========================================
pub fn scan_applications() -> Vec<SearchResult> {
    // 使用 HashSet 自动去重路径，避免重复扫描同一个目录
    let mut scan_roots = HashSet::new();

    // -----------------------------------------------------------
    // A. 关键系统路径 (高纯度数据源)
    // -----------------------------------------------------------
    #[cfg(target_os = "windows")]
    {
        // 1. 开始菜单 (最重要)
        if let Ok(pd) = std::env::var("ProgramData") {
            scan_roots.insert(PathBuf::from(pd).join(r"Microsoft\Windows\Start Menu\Programs"));
        }
        if let Some(roaming) = dirs::config_dir() {
            scan_roots.insert(roaming.join(r"Microsoft\Windows\Start Menu\Programs"));
        }
        
        // 2. 桌面
        if let Some(desktop) = dirs::desktop_dir() {
            scan_roots.insert(desktop);
        }

        // 3. 用户常用目录 (Downloads, Documents) - 限制深度扫描
        if let Some(docs) = dirs::document_dir() { scan_roots.insert(docs); }
        if let Some(dl) = dirs::download_dir() { scan_roots.insert(dl); }

        // 4. 注册表记录的安装位置
        for p in get_registry_installed_paths() {
            scan_roots.insert(p);
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        scan_roots.insert(PathBuf::from("/Applications"));
        scan_roots.insert(PathBuf::from("/usr/share/applications"));
        if let Some(home) = dirs::home_dir() {
             scan_roots.insert(home.join(".local/share/applications"));
        }
    }

    // -----------------------------------------------------------
    // B. PATH 环境变量 (关键改进)
    // -----------------------------------------------------------
    // 覆盖 git.exe, code.exe, node.exe 等 CLI 工具
    for p in get_path_env_dirs() {
        scan_roots.insert(p);
    }

    // -----------------------------------------------------------
    // C. 磁盘全盘扫描 (查漏补缺)
    // -----------------------------------------------------------
    #[cfg(target_os = "windows")]
    {
        // 扫描 D: 到 Z:
        // 注意：全盘扫描非常慢，建议后续加上开关控制或限制深度
        for drive_char in b'D'..=b'Z' {
            let drive_str = format!("{}:\\", drive_char as char);
            let drive_path = PathBuf::from(&drive_str);
            if drive_path.exists() {
                scan_roots.insert(drive_path);
            }
        }
    }

    let roots_vec: Vec<PathBuf> = scan_roots.into_iter().collect();
    
    // -----------------------------------------------------------
    // D. 并行扫描执行
    // -----------------------------------------------------------
    let scanned_results: Vec<SearchResult> = roots_vec.par_iter()
        .flat_map(|root| {
            // 差异化深度策略：
            // - 用户目录（下载/文档）：杂文件多，限制浅层扫描 (Depth 5)
            // - 根驱动器 (D:\)：防止进入深层备份目录，限制中等深度 (Depth 10-15)
            // - 开始菜单/PATH：本身就是存放程序的，允许较深 (Depth 20+)
            
            let root_str = root.to_string_lossy();
            let is_user_garbage = root_str.contains("Downloads") || root_str.contains("Documents");
            let is_root_drive = root_str.len() <= 3 && root_str.ends_with(":\\"); // e.g. "D:\"

            let max_depth = if is_user_garbage { 4 } 
                            else if is_root_drive { 8 } 
                            else { 30 };

            WalkDir::new(root)
                .skip_hidden(true) // 跳过隐藏文件
                .follow_links(true) 
                .max_depth(max_depth) 
                .process_read_dir(|_depth, _path, _state, children| {
                    // 在进入目录前就进行过滤，大幅提升性能
                    children.retain(|entry_result| {
                        match entry_result {
                            Ok(entry) => {
                                if entry.file_type().is_dir() {
                                    !is_critical_garbage_folder(&entry.file_name().to_string_lossy())
                                } else {
                                    true 
                                }
                            },
                            Err(_) => false,
                        }
                    });
                })
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let path = e.path();
                    let path_str = path.to_string_lossy();
                    
                    if is_garbage_path(&path_str) { return false; }
                    
                    is_launchable(e)
                })
                .map(|e| {
                    let path = e.path();
                    let name = clean_filename(&path);
                    let ext = path.extension().and_then(OsStr::to_str).unwrap_or("").to_ascii_lowercase();
                    let (pinyin, abbr) = generate_pinyin_data(&name);
                    
                    SearchResult::new(
                        path.to_string_lossy().to_string(),
                        name,
                        get_file_type_display(&ext), // Subtitle 建议显示类型或路径
                        pinyin,
                        abbr
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();

    deduplicate(scanned_results)
}

// ==========================================
// 5. 启发式去重 (Heuristic Deduplication)
// ==========================================
fn deduplicate(items: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut final_map = HashMap::new();
    
    for app in items {
        // 使用文件名作为去重主键 (e.g., "Visual Studio Code")
        let key = app.title.to_lowercase();
        
        final_map.entry(key)
            .and_modify(|existing: &mut SearchResult| {
                let new_is_shortcut = app.subtitle == "Shortcut";
                let old_is_shortcut = existing.subtitle == "Shortcut";
                let new_path_len = app.id.len(); 
                let old_path_len = existing.id.len();

                // 决策逻辑：保留哪一个？
                // 1. 快捷方式优先 (.lnk) - 因为它们通常带有正确的图标和启动参数
                // 2. 如果类型相同，保留路径更短的 - (C:\bin\app.exe 优于 C:\...build\release\app.exe)
                // 3. 避免保留 "Uninstall" 相关的（虽然前面已过滤，这里是双保险）
                
                if new_is_shortcut && !old_is_shortcut {
                    *existing = app.clone();
                } else if new_is_shortcut == old_is_shortcut {
                    if new_path_len < old_path_len {
                         *existing = app.clone();
                    }
                }
            })
            .or_insert(app);
    }

    let mut final_list: Vec<SearchResult> = final_map.into_values().collect();
    
    // 排序优化：短的标题排前面（通常更匹配），或者按字典序
    final_list.sort_by(|a, b| {
        // 长度优先排序，解决搜索 "Code" 时 "Code" 排在 "Code Helper" 前面
        match a.title.len().cmp(&b.title.len()) {
            std::cmp::Ordering::Equal => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            other => other,
        }
    });
    final_list
}
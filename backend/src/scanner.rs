use crate::models::SearchResult;
use jwalk::{DirEntry, WalkDir};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use sysinfo::Disks;

// ==========================================
// 1. 智能黑名单 (遇到这些文件夹绝对不进去)
// ==========================================
fn is_garbage_folder(name: &str) -> bool {
    let n = name.to_lowercase();
    matches!(
        n.as_str(),
        // 系统核心
        "windows" | "winnt" | "system32" | "syswow64" | "programdata" | "recovery" |
        "$recycle.bin" | "system volume information" | "msocache" | "perflogs" | "boot" |
        // 开发垃圾
        "node_modules" | "target" | "build" | "dist" | "vendor" | "packages" | "bin" | "obj" |
        ".git" | ".svn" | ".idea" | ".vscode" | "__pycache__" | "venv" | ".env" |
        // 用户缓存
        "appdata" | "temp" | "tmp" | "cache" | "cookies" | "history" | "thumbnails" |
        // 硬件驱动
        "nvidia" | "intel" | "amd" | "realtek" | "drivers"
    ) || n.starts_with('.') // 忽略所有隐藏文件夹 (.config, .local 等)
}

// ==========================================
// 2. 智能深度控制 (判断是否值得深入扫描)
// ==========================================
fn get_max_depth_for_path(path: &Path) -> usize {
    let path_str = path.to_string_lossy().to_lowercase();
    
    // 如果路径包含这些关键词，说明是软件安装目录，允许挖深一点
    if path_str.contains("program files") || 
       path_str.contains("steam") || 
       path_str.contains("games") || 
       path_str.contains("common") || 
       path_str.contains("apps") ||
       path_str.contains("software") {
        return 5; // 允许深度 5 (例如: D:\Games\WoW\_retail_\Wow.exe)
    }
    
    // 普通目录 (如 D:\Data\Backup\...) 浅尝辄止，防止陷入深层文件夹
    3 
}

// ==========================================
// 3. 文件白名单 (只收录可执行程序，防止内存爆炸)
// ==========================================
fn is_executable(entry: &DirEntry<((), ())>) -> bool {
    if !entry.file_type().is_file() { return false; }
    
    let path = entry.path();
    let ext = path.extension().and_then(OsStr::to_str).unwrap_or("").to_lowercase();

    // 严选模式：只通过 exe 和 快捷方式。
    // 如果你全盘扫描图片/文档，内存一定会爆，所以这里只放行 App。
    match ext.as_str() {
        "exe" | "lnk" | "bat" | "cmd" | "com" | "msc" => true,
        #[cfg(target_os = "macos")]
        "app" | "command" | "sh" => true,
        #[cfg(target_os = "linux")]
        "desktop" | "sh" | "py" => true,
        _ => false 
    }
}

// 获取文件类型
fn get_file_type_name(path: &Path) -> String {
    let ext = path.extension().and_then(OsStr::to_str).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "lnk" => "Shortcut".to_string(),
        "exe" => "Application".to_string(),
        "bat" | "cmd" | "sh" => "Script".to_string(),
        _ => "Application".to_string(),
    }
}

fn clean_name(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(" - Shortcut", "")
        .replace(" - 快捷方式", "")
        .replace("_", " ")
        .trim()
        .to_string()
}

// ==========================================
// 核心扫描逻辑
// ==========================================
pub fn scan_applications() -> Vec<SearchResult> {
    let mut roots = Vec::new();

    // A. 必须扫描的高价值目标 (C盘核心)
    #[cfg(target_os = "windows")]
    {
        if let Ok(pd) = std::env::var("ProgramData") {
            roots.push(PathBuf::from(pd).join(r"Microsoft\Windows\Start Menu\Programs"));
        }
        if let Some(roaming) = dirs::config_dir() {
            roots.push(roaming.join(r"Microsoft\Windows\Start Menu\Programs"));
        }
        if let Some(desktop) = dirs::desktop_dir() {
            roots.push(desktop);
        }
        // 加入 PATH 环境变量中的路径 (找回开发工具 code, git, node 等)
        if let Some(paths) = std::env::var_os("PATH") {
            for path in std::env::split_paths(&paths) {
                if path.exists() { roots.push(path); }
            }
        }
    }
    
    // Mac/Linux 基础路径
    #[cfg(not(target_os = "windows"))]
    {
        roots.push(PathBuf::from("/Applications"));
        roots.push(PathBuf::from("/usr/share/applications"));
        if let Some(d) = dirs::desktop_dir() { roots.push(d); }
    }

    // B. 全盘智能扫描 (D:, E:, F: ...)
    let disks = Disks::new_with_refreshed_list();
    for disk in &disks {
        let mount = disk.mount_point().to_path_buf();
        
        // 针对 C 盘的特殊处理：
        // C 盘根目录太乱，我们通常只依靠上面的 "Start Menu" 扫描即可。
        // 只有当用户确实在 C:\Games 这种非标准位置装软件时才需要扫根目录。
        // 为了稳健，我们这里跳过 C 盘根目录的全盘扫描，只扫 D/E/F...
        #[cfg(target_os = "windows")]
        if mount == Path::new("C:\\") {
            continue; 
        }
        
        roots.push(mount);
    }

    // 去重
    let unique_roots: HashSet<PathBuf> = roots.into_iter().filter(|p| p.exists()).collect();
    let roots_vec: Vec<PathBuf> = unique_roots.into_iter().collect();

    // C. 并行遍历
    let results: Vec<SearchResult> = roots_vec.par_iter()
        .flat_map(|root| {
            // 智能设定该根目录的扫描深度
            let depth = get_max_depth_for_path(root);
            
            WalkDir::new(root)
                .skip_hidden(true)
                .follow_links(false)
                .max_depth(depth) // <--- 应用智能深度
                .process_read_dir(|_depth, _path, _state, children| {
                    // 核心过滤逻辑：在进入文件夹之前就决定是否要抛弃它
                    children.retain(|dir_entry_result| {
                        if let Ok(entry) = dir_entry_result {
                            if entry.file_type().is_dir() {
                                let name = entry.file_name().to_string_lossy();
                                // 如果是垃圾文件夹，直接在树枝上剪断，不进入
                                return !is_garbage_folder(&name);
                            }
                        }
                        true
                    });
                })
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| is_executable(e)) // <--- 只保留 App
                .map(|e| {
                    let path = e.path();
                    SearchResult::new(
                        path.to_string_lossy().to_string(),
                        clean_name(&path),
                        get_file_type_name(&path)
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // D. 结果聚合与去重
    let mut unique_map = HashMap::new();
    for app in results {
        // 简单的去重策略：同名应用保留路径短的？或者保留先扫描到的？
        // 这里按 ID (路径) 去重，如果两个路径一样才去重
        let key = app.id.to_lowercase(); 
        unique_map.entry(key).or_insert(app);
    }

    let mut final_list: Vec<SearchResult> = unique_map.into_values().collect();
    final_list.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    
    final_list
}
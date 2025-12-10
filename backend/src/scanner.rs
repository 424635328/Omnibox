use crate::models::SearchResult;
use jwalk::{DirEntry, WalkDir};
use pinyin::ToPinyin;
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
    ) || n.starts_with('.') 
}

fn get_max_depth_for_path(path: &Path) -> usize {
    let path_str = path.to_string_lossy().to_lowercase();
    
    // 如果路径包含这些关键词，说明是软件安装目录，允许挖深一点
    if path_str.contains("program files") || 
       path_str.contains("steam") || 
       path_str.contains("games") || 
       path_str.contains("common") || 
       path_str.contains("apps") ||
       path_str.contains("software") {
        return 8;
    }
    
    // 普通目录浅尝辄止
    6
}

// ==========================================
// 3. 文件白名单 (只收录可执行程序)
// ==========================================
fn is_executable(entry: &DirEntry<((), ())>) -> bool {
    if !entry.file_type().is_file() { return false; }
    
    let path = entry.path();
    let ext = path.extension().and_then(OsStr::to_str).unwrap_or("").to_lowercase();

    match ext.as_str() {
        "exe" | "lnk" | "bat" | "cmd" | "com" | "msc" => true,
        #[cfg(target_os = "macos")]
        "app" | "command" | "sh" => true,
        #[cfg(target_os = "linux")]
        "desktop" | "sh" | "py" => true,
        _ => false 
    }
}

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
// 4. 拼音生成工具 (已修复类型错误)
// ==========================================
fn generate_pinyin(name: &str) -> (String, String) {
    let mut full_pinyin = String::new();
    let mut acronym = String::new();

    for char in name.chars() {
        // 修正点：to_pinyin 返回的是 Option<Pinyin>，不需要再次解包
        if let Some(p) = char.to_pinyin() {
            full_pinyin.push_str(p.plain());
            acronym.push(p.plain().chars().next().unwrap_or_default());
            continue;
        }
        
        // 非中文字符直接转小写保留
        full_pinyin.push(char.to_ascii_lowercase());
        acronym.push(char.to_ascii_lowercase());
    }
    (full_pinyin, acronym)
}

// ==========================================
// 核心扫描入口
// ==========================================
pub fn scan_applications() -> Vec<SearchResult> {
    let mut roots = Vec::new();

    // A. 必须扫描的高价值目标 (Windows Start Menu & PATH)
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
        // 扫描环境变量 PATH (找回 code, node, git 等)
        if let Some(paths) = std::env::var_os("PATH") {
            for path in std::env::split_paths(&paths) {
                if path.exists() { roots.push(path); }
            }
        }
    }
    
    #[cfg(not(target_os = "windows"))]
    {
        roots.push(PathBuf::from("/Applications"));
        roots.push(PathBuf::from("/usr/share/applications"));
        roots.push(PathBuf::from("/usr/local/bin"));
        if let Some(d) = dirs::desktop_dir() { roots.push(d); }
    }

    // B. 全盘智能扫描 (挂载的所有磁盘，跳过 C 盘根)
    let disks = Disks::new_with_refreshed_list();
    for disk in &disks {
        let mount = disk.mount_point().to_path_buf();
        // C 盘依赖 Start Menu 和 PATH 已经够了，直接扫根目录太慢且危险
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
                .max_depth(depth)
                .process_read_dir(|_, _, _, children| {
                    // 核心过滤：进入文件夹前检查是否是垃圾目录
                    children.retain(|dir_entry_result| {
                        if let Ok(entry) = dir_entry_result {
                            if entry.file_type().is_dir() {
                                let name = entry.file_name().to_string_lossy();
                                return !is_garbage_folder(&name);
                            }
                        }
                        true
                    });
                })
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| is_executable(e)) // 只要可执行文件
                .map(|e| {
                    let path = e.path();
                    let name = clean_name(&path);
                    // 生成拼音
                    let (pinyin, acronym) = generate_pinyin(&name);
                    
                    SearchResult::new(
                        path.to_string_lossy().to_string(),
                        name,
                        get_file_type_name(&path),
                        pinyin,
                        acronym
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // D. 结果聚合与去重
    let mut unique_map = HashMap::new();
    for app in results {
        let key = app.id.to_lowercase(); 
        unique_map.entry(key).or_insert(app);
    }

    let mut final_list: Vec<SearchResult> = unique_map.into_values().collect();
    final_list.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    
    final_list
}
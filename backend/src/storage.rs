use crate::models::{SearchResult, UserHabits, AppSettings};
use std::fs;
use std::path::PathBuf;
use tauri::api::path::cache_dir;

pub struct Storage {
    cache_dir: PathBuf,
}

impl Storage {
    pub fn new() -> Self {
        let mut path = cache_dir().unwrap_or(PathBuf::from("."));
        path.push("omnibox");
        if let Err(e) = fs::create_dir_all(&path) {
            eprintln!("Error creating cache directory: {}", e);
        }
        Self { cache_dir: path }
    }

    fn get_path(&self, filename: &str) -> PathBuf {
        self.cache_dir.join(filename)
    }

    fn save<T: serde::Serialize + ?Sized>(&self, filename: &str, data: &T) {
        let path = self.get_path(filename);
        match bincode::serialize(data) {
            Ok(bytes) => {
                if let Err(e) = fs::write(&path, bytes) {
                    eprintln!("Failed to write {}: {}", filename, e);
                }
            }
            Err(e) => eprintln!("Failed to serialize {}: {}", filename, e),
        }
    }

    fn load<T: serde::de::DeserializeOwned + Default>(&self, filename: &str) -> T {
        let path = self.get_path(filename);
        match fs::read(&path) {
            Ok(bytes) => bincode::deserialize(&bytes).unwrap_or_else(|e| {
                eprintln!("Failed to deserialize {}: {}", filename, e);
                T::default()
            }),
            Err(_) => T::default(),
        }
    }

    pub fn save_apps(&self, apps: &[SearchResult]) { self.save("apps_cache_v2.bin", apps); }
    pub fn load_apps(&self) -> Vec<SearchResult> { self.load("apps_cache_v2.bin") }

    pub fn save_habits(&self, habits: &UserHabits) { self.save("user_habits.bin", habits); }
    pub fn load_habits(&self) -> UserHabits { self.load("user_habits.bin") }

    pub fn save_settings(&self, settings: &AppSettings) { self.save("settings.bin", settings); }
    pub fn load_settings(&self) -> AppSettings { self.load("settings.bin") }
}
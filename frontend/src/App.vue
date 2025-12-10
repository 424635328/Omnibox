<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick, watch, computed } from "vue";
import { invoke } from "@tauri-apps/api/tauri";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { appWindow } from "@tauri-apps/api/window";
// 已移除 open (dialog) 和 convertFileSrc 引用，因为不再需要选择文件
import { 
  Search, CornerDownLeft, AppWindow, File, Monitor, 
  Settings as SettingsIcon, X, Loader2, Image as ImageIcon, FileText, Folder, Film, Music, ArrowUp, ArrowDown,
  Palette
  // 已移除 Upload icon
} from 'lucide-vue-next';

// --- 类型定义 ---
interface SearchResult {
  id: string; 
  title: string; 
  subtitle: string; 
  score: number;
  action_type: string; 
  action_data: string; 
  file_type: string;
}

interface AppSettings {
  max_results: number;
  enable_autostart: boolean;
  // theme_bg_image 字段保留以兼容后端接口，但前端不再允许修改
  theme_bg_image: string; 
  theme_bg_opacity: number; 
  theme_bg_blur: number; 
}

// --- 状态管理 ---
const query = ref("");
const results = ref<SearchResult[]>([]);
const selectedIndex = ref(0);
const searchInput = ref<HTMLInputElement | null>(null);
const resultListRef = ref<HTMLElement | null>(null);
const isLoading = ref(false);

// 竞态处理与清理
let unlisten: UnlistenFn | null = null;
let searchTimeout: ReturnType<typeof setTimeout> | null = null;
let scrollTimeout: ReturnType<typeof setTimeout> | null = null;
let latestSearchId = 0; 

// 交互状态控制
const isKeyboardScroll = ref(false); 
const isMouseScrolling = ref(false); 
const isKeyboardSelection = ref(false); 

// 设置状态
const showSettings = ref(false);

// 固定背景路径
const FIXED_BG_PATH = "/bg.png"; 

const settings = ref<AppSettings>({ 
  max_results: 100, 
  enable_autostart: false,
  theme_bg_image: FIXED_BG_PATH, 
  theme_bg_opacity: 0.05,
  theme_bg_blur: 0
});

// --- 计算属性：动态样式 ---
const containerStyle = computed(() => {
  // 直接使用固定路径，不再进行逻辑判断
  return {
    backgroundImage: `url('${FIXED_BG_PATH}')`
  };
});

const overlayStyle = computed(() => {
  return {
    backgroundColor: `rgba(30, 30, 36, ${settings.value.theme_bg_opacity})`,
    backdropFilter: `blur(${settings.value.theme_bg_blur}px)`,
    WebkitBackdropFilter: `blur(${settings.value.theme_bg_blur}px)`
  };
});

// --- 图标映射 ---
const getIconComponent = (item: SearchResult) => {
  if (item.action_type === 'app' || item.file_type === 'Application') return AppWindow;
  const path = item.id.toLowerCase();
  if (/\.(png|jpg|jpeg|svg|bmp|webp)$/.test(path)) return ImageIcon;
  if (/\.(txt|md|doc|docx|pdf|xls|xlsx|ppt|pptx)$/.test(path)) return FileText;
  if (/\.(mp4|mkv|avi|mov|webm)$/.test(path)) return Film;
  if (/\.(mp3|wav|flac|aac)$/.test(path)) return Music;
  if (item.file_type === 'Directory' || item.file_type === 'Folder') return Folder;
  return File;
};

// --- 核心逻辑 ---
const loadSettings = async () => {
  try { 
    const saved = await invoke<AppSettings>("get_settings"); 
    settings.value = { ...settings.value, ...saved };
    // 强制覆盖：无论后端存了什么路径，前端始终使用固定路径
    settings.value.theme_bg_image = FIXED_BG_PATH;
  } catch(e) { 
    console.error("加载设置失败:", e); 
  }
};

const performSearch = async (q: string) => {
  const currentSearchId = ++latestSearchId;
  isLoading.value = true;
  
  try { 
    const res = await invoke<SearchResult[]>("search", { query: q });
    if (currentSearchId !== latestSearchId) return;
    results.value = res;
    selectedIndex.value = 0; 
  } catch (e) {
    if (currentSearchId === latestSearchId) {
        console.error("搜索失败:", e);
        results.value = [];
    }
  } finally {
    if (currentSearchId === latestSearchId) {
        isLoading.value = false;
    }
  }
};

const saveSettings = async () => {
  try {
    settings.value.max_results = Number(settings.value.max_results);
    // 确保保存时也是固定路径
    settings.value.theme_bg_image = FIXED_BG_PATH;
    await invoke("save_settings", { newSettings: settings.value });
    
    showSettings.value = false;
    await nextTick();
    searchInput.value?.focus();
    performSearch(query.value);
  } catch(e) { 
    console.error("保存设置失败:", e); 
  }
};

const toggleSettings = () => {
  if (!showSettings.value) { 
    loadSettings(); 
    showSettings.value = true; 
  } else { 
    showSettings.value = false; 
    nextTick(() => searchInput.value?.focus()); 
  }
};

// 已移除 selectWallpaper 和 clearWallpaper 函数

const highlightText = (text: string, keyword: string) => {
  if (!keyword || !text) return text || '';
  const safeText = text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const safeKeyword = keyword.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const regex = new RegExp(`(${safeKeyword})`, "gi");
  return safeText.replace(regex, '<span class="highlight">$1</span>');
};

// --- 监听滚动与键盘选择 ---
watch(selectedIndex, async (newIndex) => {
  if (!isKeyboardScroll.value || !resultListRef.value) return;
  await nextTick();
  const children = resultListRef.value.children;
  if (newIndex >= children.length) return;
  const el = children[newIndex] as HTMLElement;
  if (el) el.scrollIntoView({ block: "nearest", behavior: "smooth" });
  isKeyboardScroll.value = false;
});

// --- 生命周期 ---
onMounted(async () => {
  await loadSettings();
  searchInput.value?.focus();
  
  unlisten = await listen("window-focused", () => {
    nextTick(() => {
        searchInput.value?.focus();
        searchInput.value?.select();
    });
  });
});

onUnmounted(() => { 
  if (unlisten) unlisten(); 
  if (searchTimeout) clearTimeout(searchTimeout);
  if (scrollTimeout) clearTimeout(scrollTimeout);
});

// --- 事件处理 ---
const handleListScroll = () => {
  isMouseScrolling.value = true;
  if (scrollTimeout) clearTimeout(scrollTimeout);
  scrollTimeout = setTimeout(() => { isMouseScrolling.value = false; }, 150);
};

const handleMouseMove = (index: number, e: MouseEvent) => {
  if (isMouseScrolling.value) return;
  if (e.movementX === 0 && e.movementY === 0) return;
  selectedIndex.value = index;
  isKeyboardSelection.value = false; 
};

const hideWindow = async () => { 
    await appWindow.hide(); 
};

const handleInput = () => {
  if (searchTimeout) clearTimeout(searchTimeout);
  isLoading.value = true;
  searchTimeout = setTimeout(() => {
    performSearch(query.value);
  }, 100); 
};

const handleExecute = async (item: SearchResult) => {
  if (!item) return;
  try {
      await invoke("execute_item", { id: item.id, query: query.value });
  } catch (e) {
      console.error("执行失败", e);
  }
};

const handleKeyDown = (e: KeyboardEvent) => {
  if (e.key === "ArrowDown" || e.key === "ArrowUp") e.preventDefault();
  
  if (e.key === "Escape") {
    if (query.value.length > 0) {
      query.value = "";
      performSearch(""); 
    } else {
      hideWindow();
    }
    return;
  }

  if (results.value.length === 0) return;
  
  isKeyboardScroll.value = true;
  isKeyboardSelection.value = true;

  if (e.key === "ArrowDown") {
    selectedIndex.value = (selectedIndex.value + 1) % results.value.length;
  } else if (e.key === "ArrowUp") {
    selectedIndex.value = (selectedIndex.value - 1 + results.value.length) % results.value.length;
  } else if (e.key === "Enter") {
    const item = results.value[selectedIndex.value];
    if (item) handleExecute(item);
  }
};
</script>

<template>
  <!-- 根容器：应用动态背景图 -->
  <div class="app-background" :style="containerStyle">
    <!-- 遮罩层：应用模糊和不透明度 -->
    <div class="app-overlay" :style="overlayStyle">
      
      <div class="app-container">
        <!-- 拖拽区域 -->
        <div class="title-bar" data-tauri-drag-region></div>

        <Transition name="fade" mode="out-in">
          <!-- 主搜索界面 -->
          <div v-if="!showSettings" class="main-view" key="main">
            <div class="search-bar">
              <div class="search-icon-wrapper">
                 <Loader2 v-if="isLoading" class="search-icon animate-spin" :size="20" />
                 <Search v-else class="search-icon" :size="20" />
              </div>
              <input 
                ref="searchInput" 
                v-model="query" 
                class="input-field" 
                placeholder="搜索..." 
                @input="handleInput" 
                @keydown="handleKeyDown" 
                spellcheck="false" 
                autocomplete="off"
                autofocus
              />
            </div>

            <!-- 结果列表 -->
            <div class="result-area" ref="resultListRef" @scroll="handleListScroll">
                 <div 
                    v-for="(item, index) in results" 
                    :key="item.id" 
                    class="result-item" 
                    :class="{ selected: index === selectedIndex }" 
                    @click="handleExecute(item)" 
                    @mousemove="handleMouseMove(index, $event)"
                 >
                    <div class="icon-wrapper">
                       <component :is="getIconComponent(item)" :size="22" stroke-width="1.5" />
                    </div>
                    <div class="text-wrapper">
                      <div class="title" v-html="highlightText(item.title, query)"></div>
                      <div class="subtitle" :title="item.subtitle">{{ item.subtitle }}</div>
                    </div>
                    <div class="meta-info">
                        <span v-if="index === selectedIndex" class="enter-hint">运行 <CornerDownLeft :size="10" /></span>
                        <span v-else class="file-type-tag">{{ item.file_type }}</span>
                    </div>
                    <!-- 选中指示条 -->
                    <div class="selection-bar" v-show="index === selectedIndex"></div>
                  </div>
                  
                  <div v-if="query && results.length === 0 && !isLoading" class="empty-state">
                    <Monitor :size="48" stroke-width="1" class="empty-icon"/>
                    <p>未找到相关结果</p>
                  </div>
            </div>

            <div class="footer">
              <div class="footer-left">
                 <div class="footer-key"><span class="key">选择</span> <ArrowUp :size="10" /><ArrowDown :size="10" /></div>
                 <div class="footer-key"><span class="key">打开</span> <span class="key-enter">↵</span></div>
              </div>
              
              <button class="footer-btn" @click="toggleSettings" title="设置">
                  <SettingsIcon :size="16" />
              </button>
            </div>
          </div>

          <!-- 设置面板 -->
          <div v-else class="settings-view" key="settings">
            <div class="settings-header">
              <h2>设置</h2>
              <button class="close-btn" @click="toggleSettings"><X :size="20"/></button>
            </div>
            
            <div class="settings-content">
              <!-- 通用设置 -->
              <div class="settings-section-title">
                <SettingsIcon :size="14"/> 通用
              </div>
              <div class="setting-group">
                <div class="setting-item">
                  <div class="setting-label">
                    <label>最大结果数</label>
                    <span class="setting-desc">限制搜索显示的条目数量。</span>
                  </div>
                  <input type="number" v-model="settings.max_results" class="setting-input" min="10" max="500" />
                </div>
                
                <div class="setting-item">
                   <div class="setting-label">
                    <label>开机自启</label>
                    <span class="setting-desc">登录系统时自动启动应用。</span>
                  </div>
                  <label class="switch">
                    <input type="checkbox" v-model="settings.enable_autostart">
                    <span class="slider round"></span>
                  </label>
                </div>
              </div>

              <!-- 外观设置 (移除壁纸选择，保留透明度/模糊度) -->
              <div class="settings-section-title" style="margin-top: 16px;">
                <Palette :size="14"/> 外观
              </div>
              <div class="setting-group">
                <!-- 已移除图片选择行 -->
                
                <div class="setting-item column">
                   <div class="setting-label full-width">
                    <label>背景遮罩浓度 ({{ Math.round(settings.theme_bg_opacity * 100) }}%)</label>
                  </div>
                  <input type="range" v-model.number="settings.theme_bg_opacity" min="0" max="1" step="0.05" class="range-slider">
                </div>

                <div class="setting-item column">
                   <div class="setting-label full-width">
                    <label>毛玻璃模糊度 ({{ settings.theme_bg_blur }}px)</label>
                  </div>
                  <input type="range" v-model.number="settings.theme_bg_blur" min="0" max="50" step="1" class="range-slider">
                </div>
              </div>

              <div class="settings-info"><p>Omnibox v0.1.0</p></div>
            </div>

            <div class="settings-footer">
               <button class="cancel-btn" @click="toggleSettings">取消</button>
               <button class="save-btn" @click="saveSettings">保存更改</button>
            </div>
          </div>
        </Transition>
      </div>

    </div>
  </div>
</template>

<style>
:root {
  --text-primary: #f1f5f9;
  --text-secondary: #94a3b8;
  --accent-color: #3b82f6;
  --accent-hover: #2563eb;
  --selected-bg: linear-gradient(90deg, rgba(59, 130, 246, 0.15) 0%, rgba(59, 130, 246, 0.05) 100%);
  --border-color: rgba(255, 255, 255, 0.1);
  --highlight-text: #60a5fa;
  --input-placeholder: #64748b;
  --font-family: "Microsoft YaHei", -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
}

body {
  margin: 0; padding: 0; height: 100vh;
  font-family: var(--font-family);
  background: transparent; user-select: none; overflow: hidden;
}

/* 背景层 */
.app-background {
  width: 100vw; height: 100vh;
  background-size: cover;
  background-position: center;
  background-repeat: no-repeat;
  border-radius: 12px;
  overflow: hidden;
  box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
  transition: background-image 0.3s ease;
}

/* 遮罩层 */
.app-overlay {
  width: 100%; height: 100%;
  display: flex; flex-direction: column;
  transition: backdrop-filter 0.3s ease, background-color 0.3s ease;
}

.app-container {
  display: flex; flex-direction: column; height: 100%;
  color: var(--text-primary); box-sizing: border-box; overflow: hidden;
  border: 1px solid var(--border-color);
  border-radius: 12px;
}

.title-bar { 
  height: 18px; 
  -webkit-app-region: drag; 
  flex-shrink: 0; 
  z-index: 9999;
}

.main-view { 
  display: flex; flex-direction: column; flex: 1; 
  overflow: hidden; position: relative; min-height: 0; 
}

/* 搜索栏优化 */
.search-bar { 
  display: flex; align-items: center; padding: 4px 18px 16px 18px; 
  border-bottom: 1px solid var(--border-color); gap: 14px; flex-shrink: 0; 
}
.search-icon-wrapper { display: flex; align-items: center; justify-content: center; width: 24px; }
.search-icon { color: var(--accent-color); opacity: 0.9; }
.animate-spin { animation: spin 1s linear infinite; } @keyframes spin { to { transform: rotate(360deg); } }

.input-field { 
  flex: 1; background: transparent; border: none; font-size: 20px; 
  color: var(--text-primary); outline: none; font-weight: 300;
  font-family: var(--font-family);
}
.input-field::placeholder { color: var(--input-placeholder); }

/* 结果区域 */
.result-area { 
  flex: 1; 
  overflow-y: auto !important; 
  overflow-x: hidden;
  padding: 10px 8px; 
  min-height: 0; 
  height: 100%; 
  -webkit-app-region: no-drag; 
  position: relative;
  z-index: 10; 
}

/* 滚动条美化 */
.result-area::-webkit-scrollbar { width: 4px; }
.result-area::-webkit-scrollbar-track { background: transparent; }
.result-area::-webkit-scrollbar-thumb { 
  background: rgba(255, 255, 255, 0.15); 
  border-radius: 4px; 
}
.result-area::-webkit-scrollbar-thumb:hover { background: rgba(255, 255, 255, 0.3); }

/* 列表项 */
.result-item { 
  position: relative; display: flex; align-items: center; padding: 10px 14px; 
  border-radius: 8px; cursor: pointer; margin-bottom: 4px; 
  transition: all 0.15s ease; flex-shrink: 0; 
  border: 1px solid transparent;
}
.result-item.selected { 
  background: var(--selected-bg); 
  border-color: rgba(59, 130, 246, 0.1);
  box-shadow: 0 2px 8px rgba(0,0,0,0.1);
}

.selection-bar { 
  position: absolute; left: 0; top: 10%; 
  height: 80%; width: 3px; background: var(--accent-color); 
  border-radius: 0 3px 3px 0; box-shadow: 0 0 8px var(--accent-color);
}

.icon-wrapper { 
  display: flex; align-items: center; justify-content: center; width: 32px; height: 32px; 
  margin-right: 14px; color: var(--text-secondary); 
  background: rgba(255, 255, 255, 0.03); border-radius: 8px; 
  box-shadow: inset 0 0 0 1px rgba(255,255,255,0.05);
}
.result-item.selected .icon-wrapper { color: white; background: rgba(59, 130, 246, 0.2); }

.text-wrapper { flex: 1; overflow: hidden; min-width: 0; display: flex; flex-direction: column; justify-content: center;}
.title { font-size: 15px; font-weight: 500; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; line-height: 1.4;} 
.highlight { color: var(--highlight-text); font-weight: bold; text-shadow: 0 0 10px rgba(96, 165, 250, 0.3); }
.subtitle { 
  font-size: 12px; color: var(--text-secondary); opacity: 0.7; 
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis; 
}

.meta-info { display: flex; align-items: center; gap: 8px; margin-left: 12px; flex-shrink: 0;}
.file-type-tag { font-size: 10px; padding: 3px 6px; border-radius: 4px; background: rgba(255, 255, 255, 0.05); color: var(--text-secondary); border: 1px solid rgba(255,255,255,0.05); }
.enter-hint { 
  font-size: 11px; color: var(--accent-color); font-weight: 600; 
  display: flex; align-items: center; gap: 4px; 
  background: rgba(59,130,246,0.1); padding: 3px 8px; border-radius: 4px; 
}

.empty-state { height: 60%; display: flex; flex-direction: column; align-items: center; justify-content: center; opacity: 0.5; color: var(--text-secondary); }
.empty-icon { margin-bottom: 12px; opacity: 0.7; }

/* 底部栏 */
.footer { 
  display: flex; align-items: center; justify-content: space-between; 
  padding: 0 16px; background: rgba(0,0,0,0.15); 
  border-top: 1px solid var(--border-color); 
  font-size: 11px; color: var(--text-secondary); height: 32px; flex-shrink: 0; 
  backdrop-filter: blur(10px);
}
.footer-left { display: flex; gap: 16px; }
.footer-key { display: flex; align-items: center; gap: 6px; }
.key { opacity: 0.8; }
.key-enter { font-family: monospace; font-size: 14px; position: relative; top: 1px; }

.footer-btn {
  background: transparent; border: none; color: var(--text-secondary);
  width: 26px; height: 26px; border-radius: 6px;
  display: flex; align-items: center; justify-content: center;
  cursor: pointer; transition: all 0.2s;
}
.footer-btn:hover { color: var(--text-primary); background: rgba(255,255,255,0.1); }

/* --- 设置面板 --- */
.settings-view { flex: 1; display: flex; flex-direction: column; padding: 20px 24px; min-height: 0; background: rgba(0,0,0,0.2); }
.settings-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 16px; flex-shrink: 0; }
.settings-header h2 { margin: 0; color: white; font-size: 20px; font-weight: 600; }
.close-btn { background: none; border: none; color: #94a3b8; cursor: pointer; display: flex; padding: 4px; border-radius: 4px;}
.close-btn:hover { background: rgba(255,255,255,0.1); color: white;}

.settings-content { flex: 1; overflow-y: auto; padding-right: 6px; }
.settings-content::-webkit-scrollbar { width: 4px; }
.settings-content::-webkit-scrollbar-thumb { background: rgba(255,255,255,0.1); border-radius: 4px; }

.settings-section-title {
  display: flex; align-items: center; gap: 8px;
  color: var(--highlight-text); font-size: 12px; font-weight: bold;
  margin-bottom: 8px; text-transform: uppercase; letter-spacing: 0.5px;
}

.setting-group { background: rgba(255,255,255,0.03); border: 1px solid var(--border-color); border-radius: 10px; overflow: hidden; margin-bottom: 16px;}
.setting-item { display: flex; justify-content: space-between; align-items: center; padding: 14px 16px; border-bottom: 1px solid var(--border-color); }
.setting-item:last-child { border-bottom: none; }
.setting-item.column { flex-direction: column; align-items: flex-start; gap: 10px; }

.setting-label label { color: var(--text-primary); font-size: 14px; display: block; margin-bottom: 4px; font-weight: 500;}
.setting-desc { font-size: 12px; color: var(--text-secondary); opacity: 0.8; }
.setting-input { background: rgba(0,0,0,0.3); border: 1px solid var(--border-color); color: white; padding: 6px 10px; width: 60px; text-align: center; border-radius: 6px; outline: none; font-size: 13px;}
.setting-input:focus { border-color: var(--accent-color); }

.full-width { width: 100%; display: flex; justify-content: space-between; }

/* 按钮样式 */
.wallpaper-controls { display: flex; gap: 8px; }
.btn-secondary, .btn-danger {
  padding: 6px 12px; border-radius: 6px; border: 1px solid var(--border-color);
  font-size: 12px; cursor: pointer; display: flex; align-items: center; gap: 6px;
  background: rgba(255,255,255,0.05); color: var(--text-primary); transition: all 0.2s;
}
.btn-secondary:hover { background: rgba(255,255,255,0.1); }
.btn-danger { color: #f87171; border-color: rgba(248, 113, 113, 0.3); }
.btn-danger:hover { background: rgba(248, 113, 113, 0.1); }

/* 滑块样式 */
.range-slider {
  -webkit-appearance: none; width: 100%; height: 4px; background: rgba(255,255,255,0.1);
  border-radius: 2px; outline: none;
}
.range-slider::-webkit-slider-thumb {
  -webkit-appearance: none; width: 16px; height: 16px; border-radius: 50%;
  background: var(--text-primary); cursor: pointer; transition: transform 0.1s;
}
.range-slider::-webkit-slider-thumb:hover { transform: scale(1.2); }

.settings-footer { margin-top: auto; display: flex; justify-content: flex-end; gap: 12px; padding-top: 20px; flex-shrink: 0; }
.cancel-btn, .save-btn { padding: 8px 20px; border-radius: 6px; border: none; cursor: pointer; font-size: 13px; font-weight: 500; transition: all 0.2s; }
.cancel-btn { background: transparent; color: var(--text-secondary); border: 1px solid var(--border-color); }
.cancel-btn:hover { background: rgba(255,255,255,0.05); color: white; }
.save-btn { background: var(--accent-color); color: white; box-shadow: 0 4px 10px rgba(59, 130, 246, 0.3); }
.save-btn:hover { background: var(--accent-hover); transform: translateY(-1px); }

.switch { position: relative; width: 40px; height: 22px; }
.switch input { opacity: 0; width: 0; height: 0; }
.slider { position: absolute; cursor: pointer; top: 0; left: 0; right: 0; bottom: 0; background-color: #475569; transition: .3s; border-radius: 34px; }
.slider:before { position: absolute; content: ""; height: 16px; width: 16px; left: 3px; bottom: 3px; background-color: white; transition: .3s; border-radius: 50%; box-shadow: 0 2px 4px rgba(0,0,0,0.2); }
input:checked + .slider { background-color: var(--accent-color); }
input:checked + .slider:before { transform: translateX(18px); }

.settings-info { text-align: center; margin-top: 20px; font-size: 10px; color: var(--text-secondary); opacity: 0.5; }

/* 动画 */
.fade-enter-active, .fade-leave-active { transition: opacity 0.2s ease, transform 0.2s ease; }
.fade-enter-from, .fade-leave-to { opacity: 0; transform: scale(0.98); }
</style>
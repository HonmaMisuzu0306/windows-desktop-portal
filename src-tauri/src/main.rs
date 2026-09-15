#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod desktop;
mod desktop_icons;
mod dock;
mod hotkey;
mod icons;
mod sync;

use config::{AppConfig, Item, ItemSource, Recent, MAX_RECENT};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

/// 所有 command 都是同步 `fn`，在 Tauri 2 里是 `ExecutionContext::Blocking` 内联执行，
/// 也就是**跑在主线程上、串行执行**。因此"读-改-写"不会交错，不需要 State<Mutex<...>>。
/// ⚠️ 一旦有人把某个 command 改成 `async fn` 或丢进 `spawn`，就会立刻出现丢失更新。
fn config_file(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("config.json"))
}

/// 配置加载结果。区分「文件不存在」和「文件损坏」是整套安全网的地基：
/// 前者用默认值完全正确，后者若静默重置就会销毁用户数据。
enum Loaded {
    /// 文件不存在 —— 全新用户
    Fresh(AppConfig),
    Ok(AppConfig),
    /// 文件在但读不了或解析不了 —— 绝不静默重置
    Corrupt(String),
}

/// 迁移前的一次性快照。文件已存在就不重复做，所以可以安全地被每次 load 调用。
/// 比 .bak 更强：.bak 会被后续每次写覆盖，这个不会。
fn snapshot_pre_migration(app: &AppHandle) -> Option<std::path::PathBuf> {
    let path = config_file(app).ok()?;
    let dst = path.with_file_name(format!("config.v{}.json", config::SCHEMA_VERSION - 1));
    if dst.exists() || !path.exists() {
        return Some(dst);
    }
    std::fs::copy(&path, &dst).ok()?;
    Some(dst)
}

fn load(app: &AppHandle) -> Loaded {
    let path = match config_file(app) {
        Ok(p) => p,
        Err(e) => return Loaded::Corrupt(format!("定位配置目录失败：{e}")),
    };

    match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Fresh(AppConfig::default()),
        Err(e) => Loaded::Corrupt(format!("读取配置失败：{e}")),
        Ok(raw) => match serde_json::from_str::<AppConfig>(&raw) {
            Ok(mut cfg) => {
                if cfg.schema_version < config::SCHEMA_VERSION {
                    // 先留快照再改内存里的版本号。落盘发生在下一次写操作。
                    if let Some(p) = snapshot_pre_migration(app) {
                        eprintln!("[config] v{} → v{}，原始快照：{}",
                            cfg.schema_version, config::SCHEMA_VERSION, p.display());
                    }
                    cfg.schema_version = config::SCHEMA_VERSION;
                }
                cfg.normalize();
                Loaded::Ok(cfg)
            }
            Err(e) => Loaded::Corrupt(format!("解析配置失败：{e}")),
        },
    }
}

/// 本次运行是否已因配置损坏而进入只读模式（只提示一次）
static CORRUPT_REPORTED: AtomicBool = AtomicBool::new(false);

/// 把损坏的配置另存一份，不动原文件。
fn quarantine(app: &AppHandle) -> String {
    let Ok(path) = config_file(app) else {
        return "无法定位配置目录".into();
    };
    if !path.exists() {
        return "配置文件不存在".into();
    }
    let dst = path.with_file_name(format!("config.corrupt-{}.json", now()));
    match std::fs::copy(&path, &dst) {
        Ok(_) => format!("原文件已备份到 {}", dst.display()),
        Err(e) => format!("备份原文件失败：{e}"),
    }
}

/// **所有会写配置的命令都必须走这里。**
/// 配置损坏时返回 Err，于是同步不跑、write 不跑——用户的数据原样躺在磁盘上等处置。
fn read_or_fail(app: &AppHandle) -> Result<AppConfig, String> {
    match load(app) {
        Loaded::Fresh(c) | Loaded::Ok(c) => Ok(c),
        Loaded::Corrupt(err) => {
            if !CORRUPT_REPORTED.swap(true, Ordering::SeqCst) {
                eprintln!("[config] {err}；{}", quarantine(app));
            }
            Err(format!(
                "{err}。已备份原文件，本次以只读模式运行，改动不会被保存。"
            ))
        }
    }
}

/// 原子写：先落临时文件并 fsync，再留一代备份，最后 rename 替换。
/// 半截写入只会污染 .tmp，真配置始终是完整的。
fn write(app: &AppHandle, cfg: &AppConfig) -> Result<(), String> {
    let path = config_file(app)?;
    let tmp = path.with_extension("json.tmp");
    let bak = path.with_extension("json.bak");

    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;

    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("建临时文件失败：{e}"))?;
        f.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }

    // 一代备份。失败不阻断，只是少一层保险。
    if path.exists() {
        let _ = std::fs::copy(&path, &bak);
    }

    if std::fs::rename(&tmp, &path).is_err() {
        // rename 无法覆盖已存在目标的退化路径：先删再改名（非原子，但只走异常分支）
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&tmp, &path).map_err(|e| format!("写配置失败：{e}"))?;
    }
    Ok(())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn count_desktop_items(cfg: &AppConfig) -> usize {
    cfg.categories
        .iter()
        .flat_map(|c| &c.items)
        .filter(|i| i.source == ItemSource::Desktop)
        .count()
}

/// 扫描桌面并合并进 `cfg`。
///
/// `explicit == false`（启动时的隐式同步）会额外做**批量删除保护**：
/// 若本次要删掉的桌面条目超过一半且多于 10 条，就放弃这次修改。
/// 防的是 OneDrive 迁移之类导致 `FOLDERID_Desktop` 指向新目录、
/// 扫描"成功"但结果是空的这种情况 —— 此时步骤 1 会删光用户所有桌面条目。
///
/// 口诀：**隐式保守，显式服从。**
fn sync_desktop_into(cfg: &mut AppConfig, explicit: bool) -> Result<sync::SyncReport, String> {
    // 扫描失败必须整体中止，绝不能当成"空目录"
    let scanned = desktop::scan()?;

    let before = count_desktop_items(cfg);
    let mut trial = cfg.clone();
    let report = sync::sync(&mut trial, &scanned);

    if !explicit && report.removed > 10 && report.removed * 2 > before {
        return Err(format!(
            "本次将移除 {} / {} 个桌面条目，数量异常，已跳过同步以免误删。\
             如果你确实在桌面上删掉了这些文件，点刷新按钮强制执行。",
            report.removed, before
        ));
    }

    *cfg = trial;
    Ok(report)
}

#[tauri::command]
fn load_config(app: AppHandle) -> Result<AppConfig, String> {
    let mut cfg = match load(&app) {
        Loaded::Fresh(c) | Loaded::Ok(c) => c,
        Loaded::Corrupt(err) => {
            if !CORRUPT_REPORTED.swap(true, Ordering::SeqCst) {
                eprintln!("[config] {err}；{}", quarantine(&app));
            }
            return Err(format!("{err}。原文件已备份，请修复后重启。"));
        }
    };

    // 启动时隐式同步一次。此刻窗口还是透明的 6px 细线，
    // 几十毫秒的扫描完全不可见，首帧就带上桌面内容、不会闪。
    match sync_desktop_into(&mut cfg, false) {
        Ok(r) => {
            if r.changed() {
                eprintln!("[sync] 启动同步 +{} -{} 收养{}", r.added, r.removed, r.adopted);
                let _ = write(&app, &cfg);
            }
        }
        // 扫描失败或触发保护：不阻断启动，只是这次不导入
        Err(e) => eprintln!("[sync] 启动同步跳过：{e}"),
    }

    Ok(cfg)
}

/// 手动刷新。用户明确要求的 → 无条件服从，不再走批量删除保护。
#[tauri::command]
fn refresh_desktop(app: AppHandle) -> Result<AppConfig, String> {
    let mut cfg = read_or_fail(&app)?;
    let report = sync_desktop_into(&mut cfg, true)?;
    if report.changed() {
        write(&app, &cfg)?;
        eprintln!("[sync] 手动刷新 +{} -{} 收养{}", report.added, report.removed, report.adopted);
    }
    Ok(cfg)
}

/// 拖拽添加：把落进窗口的路径塞进指定分类，按归一化路径去重。
#[tauri::command]
fn add_paths(app: AppHandle, category_id: String, paths: Vec<String>) -> Result<AppConfig, String> {
    let mut cfg = read_or_fail(&app)?;
    if let Some(cat) = cfg.categories.iter_mut().find(|c| c.id == category_id) {
        for path in paths {
            let path = path.trim().to_string();
            if path.is_empty() {
                continue;
            }
            let key = config::norm_path(&path);
            if cat.items.iter().any(|i| config::norm_path(&i.path) == key) {
                continue;
            }
            cat.items.push(Item {
                id: config::uid(),
                name: config::display_name(&path),
                kind: config::detect_kind(&path),
                path,
                source: config::ItemSource::Manual,
            });
        }
    }
    write(&app, &cfg)?;
    Ok(cfg)
}

#[tauri::command]
fn remove_item(app: AppHandle, category_id: String, item_id: String) -> Result<AppConfig, String> {
    let mut cfg = read_or_fail(&app)?;

    // 桌面来源的条目，移除的同时要记进忽略名单，
    // 否则下次同步会把它原样扫回来，表现为"怎么删都删不掉"。
    if let Some(item) = cfg
        .categories
        .iter()
        .find(|c| c.id == category_id)
        .and_then(|c| c.items.iter().find(|i| i.id == item_id))
    {
        if item.source == config::ItemSource::Desktop {
            let key = config::norm_path(&item.path);
            if !cfg.ignored.iter().any(|p| p == &key) {
                cfg.ignored.push(key);
            }
        }
    }

    if let Some(cat) = cfg.categories.iter_mut().find(|c| c.id == category_id) {
        cat.items.retain(|i| i.id != item_id);
    }
    write(&app, &cfg)?;
    Ok(cfg)
}

#[tauri::command]
fn rename_item(
    app: AppHandle,
    category_id: String,
    item_id: String,
    name: String,
) -> Result<AppConfig, String> {
    let name = name.trim().to_string();
    let mut cfg = read_or_fail(&app)?;
    if !name.is_empty() {
        if let Some(item) = cfg
            .categories
            .iter_mut()
            .find(|c| c.id == category_id)
            .and_then(|c| c.items.iter_mut().find(|i| i.id == item_id))
        {
            item.name = name;
        }
    }
    write(&app, &cfg)?;
    Ok(cfg)
}

/// 新建收藏夹。**只建一个空的映射容器** —— 不建目录、不移动任何文件。
#[tauri::command]
fn create_category(app: AppHandle, name: String) -> Result<AppConfig, String> {
    let mut cfg = read_or_fail(&app)?;
    config::add_category(&mut cfg, &name)?;
    write(&app, &cfg)?;
    Ok(cfg)
}

#[tauri::command]
fn rename_category(app: AppHandle, category_id: String, name: String) -> Result<AppConfig, String> {
    let mut cfg = read_or_fail(&app)?;
    config::rename_category(&mut cfg, &category_id, &name)?;
    write(&app, &cfg)?;
    Ok(cfg)
}

/// 删除收藏夹。里面如果是桌面来源的条目，顺手同步一次，
/// 让它们**立刻**回到「桌面」分类 —— 否则文件看起来"消失"了，
/// 而这个程序最重要的承诺恰恰是"文件一直好好的"。
#[tauri::command]
fn delete_category(app: AppHandle, category_id: String) -> Result<AppConfig, String> {
    let mut cfg = read_or_fail(&app)?;
    config::remove_category(&mut cfg, &category_id)?;
    // 同步失败（扫描出错 / 触发批量删除保护）不影响删除本身，下次启动还会再同步
    if let Ok(r) = sync_desktop_into(&mut cfg, false) {
        if r.changed() {
            eprintln!("[sync] 删除收藏夹后回收 +{} -{}", r.added, r.removed);
        }
    }
    write(&app, &cfg)?;
    Ok(cfg)
}

/// 打开目标，并把这次访问记进最近记录。
#[tauri::command]
fn open_item(app: AppHandle, path: String) -> Result<AppConfig, String> {
    opener::open(&path).map_err(|e| format!("打不开 {path}：{e}"))?;

    let mut cfg = read_or_fail(&app)?;
    let name = config::display_name(&path);
    cfg.recent.retain(|r| config::norm_path(&r.path) != config::norm_path(&path));
    cfg.recent.insert(0, Recent { name, path, at: now() });
    cfg.recent.truncate(MAX_RECENT);
    write(&app, &cfg)?;
    Ok(cfg)
}

#[tauri::command]
fn clear_recent(app: AppHandle) -> Result<AppConfig, String> {
    let mut cfg = read_or_fail(&app)?;
    cfg.recent.clear();
    write(&app, &cfg)?;
    Ok(cfg)
}

#[tauri::command]
fn set_settings(app: AppHandle, auto_hide: bool, autostart: bool) -> Result<AppConfig, String> {
    use tauri_plugin_autostart::ManagerExt;

    let launcher = app.autolaunch();
    if autostart {
        launcher.enable().map_err(|e| format!("设置开机启动失败：{e}"))?;
    } else {
        launcher.disable().map_err(|e| format!("取消开机启动失败：{e}"))?;
    }

    let mut cfg = read_or_fail(&app)?;
    cfg.settings.autostart = autostart;
    cfg.settings.auto_hide = auto_hide;
    write(&app, &cfg)?;
    Ok(cfg)
}

/// 把条目从一个分类移到另一个分类。**只改归属，不碰文件系统。**
/// 注意 `source` 保持不变：桌面条目拖到「学习」后仍是 `Desktop`，
/// 这样文件从桌面消失时，同步的步骤 1 照样能把它从「学习」里移除。
#[tauri::command]
fn move_item(app: AppHandle, item_id: String, to_category_id: String) -> Result<AppConfig, String> {
    let mut cfg = read_or_fail(&app)?;

    let Some(from_idx) = cfg
        .categories
        .iter()
        .position(|c| c.items.iter().any(|i| i.id == item_id))
    else {
        return Err(format!("找不到条目：{item_id}"));
    };
    let Some(to_idx) = cfg.categories.iter().position(|c| c.id == to_category_id) else {
        return Err(format!("分类不存在：{to_category_id}"));
    };
    if from_idx == to_idx {
        return Ok(cfg);
    }

    let Some(pos) = cfg.categories[from_idx].items.iter().position(|i| i.id == item_id) else {
        return Err(format!("找不到条目：{item_id}"));
    };
    let item = cfg.categories[from_idx].items.remove(pos);
    let key = config::norm_path(&item.path);

    // 目标分类已有同路径的条目 → 不重复添加，视觉上就是"移过去了"
    if !cfg
        .categories[to_idx]
        .items
        .iter()
        .any(|i| config::norm_path(&i.path) == key)
    {
        cfg.categories[to_idx].items.push(item);
    }

    write(&app, &cfg)?;
    Ok(cfg)
}

/// 隐藏 / 显示桌面图标。**系统级设置**，失败时如实上报而不是装作成功。
#[tauri::command]
fn set_hide_desktop_icons(app: AppHandle, hidden: bool) -> Result<AppConfig, String> {
    let mut cfg = read_or_fail(&app)?;

    let before = desktop_icons::icons_visible().ok_or("读不到桌面图标状态")?;
    let currently_hidden = !before;

    if currently_hidden == hidden {
        // 状态已经符合要求 —— 我们没动过手 → 绝不认领
        cfg.settings.hide_desktop_icons = hidden;
        write(&app, &cfg)?;
        return Ok(cfg);
    }

    if hidden {
        // 先落盘再动手：进程被强杀后，下次启动才知道自己欠了一次恢复
        cfg.settings.desktop_icons_owned_by_us = true;
        cfg.settings.desktop_icons_were_visible = before;
        cfg.settings.hide_desktop_icons = true;
        write(&app, &cfg)?;

        if let Err(e) = desktop_icons::apply_hidden(true) {
            // 系统拒绝了：回滚状态，让 UI 保持诚实
            cfg.settings.desktop_icons_owned_by_us = false;
            cfg.settings.hide_desktop_icons = false;
            let _ = write(&app, &cfg);
            return Err(e);
        }
    } else {
        desktop_icons::apply_hidden(false)?;
        cfg.settings.desktop_icons_owned_by_us = false;
        cfg.settings.desktop_icons_were_visible = true;
        cfg.settings.hide_desktop_icons = false;
        write(&app, &cfg)?;
    }
    Ok(cfg)
}

/// 前端挂载时调一次，处理两种"状态漂移"：
/// 1. 上次被强杀没来得及恢复（`owned_by_us` 还是 true）→ 立刻恢复
/// 2. 用户要求隐藏但图标当前可见（Explorer 重启会丢掉隐藏状态）→ 重新施加
#[tauri::command]
fn reconcile_desktop_icons(app: AppHandle) -> Result<AppConfig, String> {
    let mut cfg = read_or_fail(&app)?;
    // 读不到状态就什么都不做 —— 绝不猜
    let Some(visible) = desktop_icons::icons_visible() else {
        return Ok(cfg);
    };

    if cfg.settings.desktop_icons_owned_by_us {
        if cfg.settings.desktop_icons_were_visible && !visible {
            if let Err(e) = desktop_icons::apply_hidden(false) {
                eprintln!("[icons] 崩溃恢复失败（可右键桌面→查看→显示桌面图标）：{e}");
            }
        }
        cfg.settings.desktop_icons_owned_by_us = false;
        cfg.settings.desktop_icons_were_visible = true;
        write(&app, &cfg)?;
    } else if cfg.settings.hide_desktop_icons && visible {
        if desktop_icons::apply_hidden(true).is_ok() {
            cfg.settings.desktop_icons_owned_by_us = true;
            cfg.settings.desktop_icons_were_visible = true;
            write(&app, &cfg)?;
        }
    }
    Ok(cfg)
}

/// 退出路径上的恢复。幂等，重复调用无害。
/// 内部所有错误都吞掉并打日志 —— 退出路径上 panic 会污染整个关进程流程。
fn restore_desktop_icons(app: &AppHandle) {
    let Ok(mut cfg) = read_or_fail(app) else {
        return;
    };
    if !cfg.settings.desktop_icons_owned_by_us {
        return; // 没藏过 → 一个字节都不碰
    }
    if cfg.settings.desktop_icons_were_visible {
        if let Err(e) = desktop_icons::apply_hidden(false) {
            eprintln!("[icons] 退出恢复失败（可右键桌面→查看→显示桌面图标）：{e}");
        }
    }
    cfg.settings.desktop_icons_owned_by_us = false;
    cfg.settings.desktop_icons_were_visible = true;
    let _ = write(app, &cfg);
}

/// 正常退出。没有这个的话，skipTaskbar + 无托盘意味着用户只能强杀进程，
/// 而强杀正好是"退出时恢复"覆盖不到的场景。
#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

// 毛玻璃现在完全由 `dock::set_region` 通过 `DwmEnableBlurBehindWindow` 施加，
// 不再使用 window-vibrancy：
//
// - acrylic / mica 有"失焦退化"，窗口失焦时会变成更不透明的实色，
//   表现就是"点一下自己透明度尚可，点别的窗口就变浑浊"。
// - `SetWindowCompositionAttribute`（apply_blur 走的路径）虽然没有失焦变体，
//   但它把模糊铺满**整个窗口矩形**，而且裁不掉 —— 模块之间的缝隙会一直是灰的。
//
// `DwmEnableBlurBehindWindow` 的 `hRgnBlur` 能指定模糊区域，这是唯一能同时
// 做到"面板真模糊"和"缝隙真透明"的途径。

/// 面板当前是否处于隐藏（收起）状态。全局轮询线程靠它决定要不要唤醒。
static DOCK_HIDDEN: AtomicBool = AtomicBool::new(false);

/// 展开 / 收起面板。**这里只管窗口，动画归前端。**
///
/// 两个顺序上的讲究：
///
/// 1. 展开时先把尺寸位置摆好（此时窗口还不可见），再显示 —— 避免"先出现在
///    错误的位置和尺寸上、下一帧才跳到位"。
/// 2. 收起时**只隐藏，不清窗口区域**。清区域会把窗口变回整个矩形，
///    而模块之间那几条缝 webview 从来没画过 —— 被裁掉的部分重新暴露出来
///    又没有内容可画，就是那一下黑闪。区域留着不动，下次显示时它本来就是对的。
#[tauri::command]
fn set_dock_expanded(window: WebviewWindow, expanded: bool) -> Result<(), String> {
    if expanded {
        dock::layout(&window)?;
        DOCK_HIDDEN.store(false, Ordering::SeqCst);
        dock::set_visible(&window, true)
    } else {
        let r = dock::set_visible(&window, false);
        DOCK_HIDDEN.store(true, Ordering::SeqCst);
        r
    }
}

/// 光标是否贴在屏幕底边（±2px）。用于隐藏状态下的唤醒判断。
#[cfg(windows)]
fn cursor_at_bottom_edge() -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetSystemMetrics, SM_CYSCREEN};
    let mut p = POINT { x: 0, y: 0 };
    unsafe {
        if GetCursorPos(&mut p) == 0 {
            return false;
        }
        p.y >= GetSystemMetrics(SM_CYSCREEN) - 2
    }
}

#[cfg(not(windows))]
fn cursor_at_bottom_edge() -> bool {
    false
}

/// 隐藏后窗口收不到任何鼠标事件，只能主动轮询光标位置来唤醒。
///
/// 70ms 的间隔是手感和开销的折中：再慢会明显感到迟滞，再快纯属浪费。
/// 只在"已隐藏"时真正做事，展开状态下这个线程基本是空转。
fn spawn_edge_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let mut armed = true;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(70));
            if !DOCK_HIDDEN.load(Ordering::SeqCst) {
                // 展开时不检测；顺手重置armed，避免贴边状态被沿用
                armed = true;
                continue;
            }
            let at_edge = cursor_at_bottom_edge();
            if at_edge && armed {
                armed = false; // 边沿触发，不是电平触发
                let _ = app.emit("dock-wake", ());
            } else if !at_edge {
                armed = true;
            }
        }
    });
}

/// 前端量出三个模块的位置后调这个，把窗口裁成三块圆角矩形。
/// 缝隙处窗口不存在 → 露出**未被模糊的**桌面。
///
/// 同时把这份坐标记成"基准位置"：进出动画每帧只需要在它上面加一个位移，
/// 不必再回前端重新测量一遍。
#[tauri::command]
fn set_panel_regions(window: WebviewWindow, rects: Vec<dock::PanelRect>) -> Result<(), String> {
    dock::remember_base(&rects);
    match dock::set_region(&window, &rects) {
        Ok(()) => Ok(()),
        Err(e) => {
            // 前端会把 reject 吞掉，这里必须留痕，否则区域裁剪静默失效无从察觉
            eprintln!("[region] 裁剪失败：{e}");
            Err(e)
        }
    }
}

/// 动画的每一帧调一次：面板整体下移 `dy` 逻辑像素时，窗口区域和模糊区域跟着走，
/// 整窗不透明度同时降到 `alpha`。
///
/// 不跟着走的话，滑动中的面板会被旧的裁剪区域切掉一条边 —— 那才是"网页元素
/// 突然显示/隐藏"那种廉价感的真正来源。每帧只发两个数字，测量留在前端。
///
/// 淡出必须和位移同帧下达，否则玻璃和内容又会错开一格（见 `dock::set_window_alpha`）。
#[tauri::command]
fn set_panel_offset(window: WebviewWindow, dy: f64, alpha: u8) -> Result<(), String> {
    dock::set_region_offset(&window, dy)?;
    dock::set_window_alpha(&window, alpha)
}

#[tauri::command]
fn open_config_folder(app: AppHandle) -> Result<(), String> {
    opener::open(config_file(&app)?).map_err(|e| e.to_string())
}

/// 批量取 Windows 原生图标。前端按当前分类调用，不做全量预取。
/// 取不到图标的条目 `data` 为 null，前端退回几何图形 —— 不该因此报错。
#[tauri::command]
fn get_icons(paths: Vec<String>) -> Vec<icons::IconEntry> {
    icons::batch(paths)
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            load_config,
            add_paths,
            remove_item,
            rename_item,
            create_category,
            rename_category,
            delete_category,
            open_item,
            clear_recent,
            set_settings,
            set_dock_expanded,
            open_config_folder,
            refresh_desktop,
            move_item,
            set_hide_desktop_icons,
            reconcile_desktop_icons,
            quit_app,
            get_icons,
            set_panel_regions,
            set_panel_offset,
        ])
        .setup(|app| {
            let win = app.get_webview_window("main").expect("主窗口缺失");

            // 这里**故意不**调用 set_background_color —— 加回来会坏掉渲染。
            //
            // 早期"收起时留一条 3px 感应带"的方案需要它：靠
            // set_background_color(Color(0, 0, 0, 0)) 让 webview 不画底。但实测
            // Windows 上 webview 层只接受 alpha = 0，而 alpha = 0 会让它
            // **完全不渲染**（窗口在、能收输入、却一个像素都不画）；
            // 其他 alpha 又会被强制成 255，等于没用。
            //
            // 现在收起走的是 dock::set_visible → window.hide()，整窗消失、
            // 感应带不复存在，这个调用连同它要解决的问题一起作废了。

            // 窗口在 tauri.conf.json 里是 visible: false 创建的，第一次显示由这里负责。
            // 这样第一帧就已经是无边框的 —— 否则创建到 setup 之间那一小段，
            // 窗口是"可见 + 带 WS_CAPTION"的，启动时会闪一下标题栏。
            dock::layout(&win)?;
            dock::set_visible(&win, true)?;

            // 隐藏状态下收不到鼠标事件，只能靠轮询光标来唤醒
            spawn_edge_watcher(app.handle().clone());

            // 全局快捷键 Alt+`：面板关着的时候也能一键唤出
            hotkey::spawn(app.handle().clone());

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Tauri 构建失败");

    // 用 build() + run(closure) 而不是 run(context)，否则拿不到退出事件，
    // "退出时自动恢复桌面图标"就无从实现。
    app.run(|app_handle, event| match event {
        tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => {
            restore_desktop_icons(app_handle);
        }
        _ => {}
    });
}

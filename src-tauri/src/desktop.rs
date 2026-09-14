//! 桌面目录扫描。**纯只读**：只枚举，绝不移动、重命名或修改任何文件。

use crate::config::{detect_kind, display_name, norm_path, ItemKind};
use std::path::PathBuf;

/// 扫描到的桌面条目。`path` 保留原始大小写（用于真正打开文件）。
#[derive(Debug, Clone)]
pub struct Scanned {
    pub path: String,
    pub name: String,
    pub kind: ItemKind,
}

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

/// 已知文件夹解析。返回 None 表示解析失败。
#[cfg(windows)]
fn known_folder(rfid: &windows_sys::core::GUID) -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;

    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: rfid 是编译期常量 GUID；htoken 传 null 表示当前用户；
    // 成功后系统在 COM 堆上分配 NUL 结尾的宽字符串，必须用 CoTaskMemFree 释放。
    let hr = unsafe { SHGetKnownFolderPath(rfid, 0, std::ptr::null_mut(), &mut raw) };
    if hr < 0 || raw.is_null() {
        return None;
    }

    let mut len = 0usize;
    // SAFETY: 系统保证返回的是 NUL 结尾的宽字符串
    unsafe {
        while *raw.add(len) != 0 {
            len += 1;
        }
    }
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, len) });
    unsafe { CoTaskMemFree(raw as *const core::ffi::c_void) };
    Some(PathBuf::from(s))
}

fn push_unique(out: &mut Vec<PathBuf>, p: Option<PathBuf>) {
    let Some(p) = p else { return };
    if !p.is_dir() {
        return;
    }
    let key = norm_path(&p.to_string_lossy());
    if !out.iter().any(|q| norm_path(&q.to_string_lossy()) == key) {
        out.push(p);
    }
}

/// 所有桌面根目录：用户桌面 + 公共桌面，去重且只保留真实存在的。
///
/// 公共桌面的内容同样显示在桌面上，所以两个都要扫。
#[cfg(windows)]
pub fn roots() -> Vec<PathBuf> {
    use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_PublicDesktop};

    let mut out: Vec<PathBuf> = Vec::new();
    push_unique(&mut out, known_folder(&FOLDERID_Desktop));
    push_unique(&mut out, known_folder(&FOLDERID_PublicDesktop));

    // 降级链：已知文件夹 API 失败时退回环境变量。
    // 注意这在 OneDrive 重定向下会指向错误位置，所以只是最后兜底。
    if out.is_empty() {
        eprintln!("[desktop] SHGetKnownFolderPath 失败，退回环境变量（重定向下可能不准）");
        push_unique(
            &mut out,
            std::env::var("USERPROFILE").ok().map(|p| PathBuf::from(p).join("Desktop")),
        );
        push_unique(
            &mut out,
            std::env::var("PUBLIC").ok().map(|p| PathBuf::from(p).join("Desktop")),
        );
    }
    out
}

#[cfg(not(windows))]
pub fn roots() -> Vec<PathBuf> {
    Vec::new()
}

/// 扫描全部桌面根目录。
///
/// **任一 root 枚举失败 → 整体返回 Err。**
/// 绝不能把失败当成"空目录"：那样同步会认为桌面已被清空，
/// 进而删光用户所有的桌面条目。宁可这次不导入，也不能误删。
pub fn scan() -> Result<Vec<Scanned>, String> {
    let roots = roots();
    if roots.is_empty() {
        return Err("找不到任何可用的桌面目录".into());
    }

    let mut out: Vec<Scanned> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for root in &roots {
        let entries = std::fs::read_dir(root)
            .map_err(|e| format!("枚举 {} 失败：{e}", root.display()))?;

        for entry in entries.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };

            // desktop.ini 是隐藏+系统文件，属性过滤本就能挡掉；
            // 这里按名字再兜一次，防止用户手工清掉了属性。
            if fname.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }

            // DirEntry::metadata 在 Windows 上复用 FindNextFileW 已返回的数据，
            // 不产生额外 syscall。顺带挡掉 Thumbs.db、Office 的 ~$ 锁文件等。
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if let Ok(md) = entry.metadata() {
                    if md.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0 {
                        continue;
                    }
                }
            }

            let ps = path.to_string_lossy().to_string();
            if !seen.insert(norm_path(&ps)) {
                continue; // 用户桌面和公共桌面可能有同名文件
            }

            out.push(Scanned {
                name: display_name(&ps),
                kind: detect_kind(&ps),
                path: ps,
            });
        }
    }

    // 和资源管理器一样按名字排，Dock 里 90 多个条目才找得到
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(out)
}

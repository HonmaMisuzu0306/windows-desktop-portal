//! 隐藏 / 显示 Windows 桌面图标。**这是系统级设置**，实现上有几个坑必须避开。
//!
//! ## 为什么这么写
//!
//! - `WM_COMMAND(0x111)` + `wParam = 0x7402` 是**切换**语义，不是设置语义。
//!   所以必须先读当前状态，只在状态不符时才发。
//! - 必须用 `SendMessageTimeoutW` + `SMTO_ABORTIFHUNG`，**绝不能用 `SendMessageW`**。
//!   Tauri 的非 async command 跑在主线程上，跨进程 `SendMessage` 到 Explorer
//!   一旦对方卡死会挂起我们整个 UI。用 500ms 上限换"绝不挂死"。
//! - `SHELLDLL_DefView` 通常挂在 `Progman` 下，但用户用过"显示桌面"或开了
//!   壁纸轮播之后它会搬到某个 `WorkerW` 下，所以两条路都要试。
//!
//! ## 失败隔离
//!
//! 这个模块整块可以下线：删掉 `main.rs` 里 `invoke_handler` 的两行即可，
//! 数据模型不受影响。所有函数返回 `Result`，零 `unwrap`、零索引越界、
//! 所有 FFI 指针先判 null。
//!
//! ## 已知风险
//!
//! 进程被强杀时 `RunEvent::Exit` 不会触发，图标会保持隐藏。
//! 缓解手段是"动手前先把归属标记落盘"+ 下次启动 `reconcile` 自动恢复，
//! 以及 README 里写明手动恢复路径：**右键桌面 → 查看 → 显示桌面图标**。

#![cfg(windows)]

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, IsWindowVisible, SendMessageTimeoutW,
    ShowWindow, SMTO_ABORTIFHUNG, SMTO_NORMAL, SW_HIDE, SW_SHOW, WM_COMMAND,
};

/// 桌面右键菜单里"显示桌面图标"对应的命令号
const TOGGLE_DESKTOP_ICONS: usize = 0x7402;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 找 (SHELLDLL_DefView, SysListView32)。listview 可能为 null（部分系统没有）。
fn find_views() -> Option<(HWND, HWND)> {
    let progman = unsafe { FindWindowW(wide("Progman").as_ptr(), std::ptr::null()) };
    if !progman.is_null() {
        if let Some(v) = child_of(progman) {
            return Some(v);
        }
    }

    // 回退：遍历顶层窗口找含 SHELLDLL_DefView 的 WorkerW
    let mut found: HWND = std::ptr::null_mut();
    unsafe {
        EnumWindows(Some(enum_worker), &mut found as *mut HWND as isize);
    }
    if !found.is_null() {
        return child_of(found);
    }
    None
}

/// ⚠️ 这个回调绝不能 panic —— 跨 FFI 边界 unwind 是 UB。
/// 所以：零 unwrap、零 `expect`、切片一律走 `get()`。
unsafe extern "system" fn enum_worker(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut buf = [0u16; 64];
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if n <= 0 {
        return 1;
    }
    let Some(got) = buf.get(..n as usize) else {
        return 1;
    };
    if got != "WorkerW".encode_utf16().collect::<Vec<u16>>().as_slice() {
        return 1;
    }

    let dv = FindWindowExW(
        hwnd,
        std::ptr::null_mut(),
        wide("SHELLDLL_DefView").as_ptr(),
        std::ptr::null(),
    );
    if dv.is_null() {
        return 1;
    }

    *(lparam as *mut HWND) = hwnd;
    0 // 找到即停
}

fn child_of(parent: HWND) -> Option<(HWND, HWND)> {
    let defview = unsafe {
        FindWindowExW(
            parent,
            std::ptr::null_mut(),
            wide("SHELLDLL_DefView").as_ptr(),
            std::ptr::null(),
        )
    };
    if defview.is_null() {
        return None;
    }
    let listview = unsafe {
        FindWindowExW(
            defview,
            std::ptr::null_mut(),
            wide("SysListView32").as_ptr(),
            std::ptr::null(),
        )
    };
    Some((defview, listview))
}

/// 图标当前是否可见。`IsWindowVisible` 会检查所有祖先的 `WS_VISIBLE`，
/// 正是我们要的"图标实际看得见吗"。系统没有独立的"显示桌面图标"布尔量可查——
/// 右键菜单里那个勾选就是这个状态，这也正是必须自己记录"是不是我们藏的"的原因。
pub fn icons_visible() -> Option<bool> {
    let (defview, listview) = find_views()?;
    let target = if listview.is_null() { defview } else { listview };
    Some(unsafe { IsWindowVisible(target) } != 0)
}

/// 把图标设为可见 / 隐藏。幂等：状态已经对了就什么都不做。
pub fn apply_hidden(hidden: bool) -> Result<(), String> {
    let want = !hidden;

    // 最多两轮，容忍 Explorer 的异步重绘
    for _ in 0..2 {
        let now = icons_visible().ok_or("找不到桌面图标窗口")?;
        if now == want {
            return Ok(());
        }

        let (defview, listview) = find_views().ok_or("找不到桌面图标窗口")?;
        let target = if listview.is_null() { defview } else { listview };

        // 首选 shell 命令：它会同步右键菜单里"显示桌面图标"的勾选状态
        let mut out: usize = 0;
        unsafe {
            SendMessageTimeoutW(
                defview,
                WM_COMMAND,
                TOGGLE_DESKTOP_ICONS,
                0,
                SMTO_ABORTIFHUNG | SMTO_NORMAL,
                500,
                &mut out,
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(60));

        if icons_visible() == Some(want) {
            return Ok(());
        }

        // 新版 Windows 可能忽略这条消息 —— 退回直接改可见性
        unsafe { ShowWindow(target, if hidden { SW_HIDE } else { SW_SHOW }) };
    }

    if icons_visible() == Some(want) {
        Ok(())
    } else {
        Err("系统未接受桌面图标切换请求".into())
    }
}

#[cfg(not(windows))]
pub fn icons_visible() -> Option<bool> {
    None
}

#[cfg(not(windows))]
pub fn apply_hidden(_hidden: bool) -> Result<(), String> {
    Err("仅支持 Windows".into())
}

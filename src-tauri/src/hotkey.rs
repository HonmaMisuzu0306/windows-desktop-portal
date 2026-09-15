//! 全局快捷键 **Ctrl + Alt + Q** —— 鼠标不在屏幕底部时也能把面板唤出来。
//!
//! ## 为什么要自己开一个线程和消息循环
//!
//! `RegisterHotKey` 触发的 `WM_HOTKEY` 是投递到**注册线程**的消息队列里的。
//! 主线程的消息循环归 tao 所有，它不认识 `WM_HOTKEY` —— 投过去等于石沉大海，
//! 而且拿不到任何反馈（注册成功、消息被丢，两边都是静默的）。
//!
//! 所以这里在一个**自己的线程**上注册（`hWnd = NULL`），消息也只由那个线程
//! 的循环来取，整条路径完全不经过 tao。
//!
//! ## 关于系统级独占
//!
//! `RegisterHotKey` 是进程间独占的：注册了 Ctrl+Alt+Q，别的程序就收不到它了。
//! 带 Ctrl+Alt 的组合在 Windows 上没有系统含义，撞车面很小，也正是为了避开
//! 纯 Alt 组合（`Alt+Q` / `Alt+Z` 实测在本机被 ASUS 的后台服务占着）。
//! 万一还是被占了，注册会失败 —— 我们只打日志，不阻断启动。
//!
//! ⚠️ 注意 AltGr：在德语、波兰语等把 AltGr 当作 Ctrl+Alt 的键盘布局上，
//! 这个组合会顺带吃掉 AltGr+Q 打出来的字符。中文/美式布局没有 AltGr，不受影响。

/// 注册失败时也只留一条日志：快捷键是锦上添花，不该拖垮启动。
#[cfg(windows)]
pub fn spawn(app: tauri::AppHandle) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_Q,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

    /// 热键 id。同一个线程里唯一即可，取个不容易撞的值方便调试时认出来。
    const HOTKEY_ID: i32 = 0x5744; // 'W''D'

    std::thread::spawn(move || unsafe {
        // hWnd = NULL：WM_HOTKEY 会投到这个线程的消息队列，而不是某个窗口
        if RegisterHotKey(
            std::ptr::null_mut(),
            HOTKEY_ID,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            VK_Q as u32,
        )
            == 0
        {
            eprintln!("[hotkey] Ctrl+Alt+Q 注册失败，可能被别的程序独占了；其余功能不受影响");
            return;
        }
        eprintln!("[hotkey] Ctrl+Alt+Q 已就绪");

        let mut msg: MSG = std::mem::zeroed();
        // GetMessageW 返回 0 表示 WM_QUIT，-1 表示出错，两者都结束循环
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY && msg.wParam == HOTKEY_ID as usize {
                // 真正的展开/收起由前端做 —— 动画状态归它管，Rust 这侧不该有两份真相
                let _ = tauri::Emitter::emit(&app, "hotkey-toggle", ());
            }
        }
        UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID);
    });
}

#[cfg(not(windows))]
pub fn spawn(_app: tauri::AppHandle) {}

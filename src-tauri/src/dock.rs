use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use tauri::{LogicalPosition, LogicalSize, WebviewWindow};

/// 不显示时把窗口**整个隐藏**，而不是缩成一条细带。
///
/// 缩成细带的做法已经证明行不通：只要窗口还在屏幕上，webview 就会画自己的
/// 底色，那一条再细也是可见的像素块。而"平时感知不到应用存在"要求是零像素。
///
/// 代价是隐藏后收不到任何鼠标事件 —— 唤醒改由全局轮询光标位置负责，
/// 见 `main.rs` 里的 `spawn_edge_watcher`。
///
/// ⚠️ 收起时**不要**顺手清掉窗口区域。`SetWindowRgn(hwnd, NULL)` 会把窗口
/// 变回整个矩形，而模块之间那几条缝 webview 从来没画过 —— 于是被裁掉的部分
/// 重新暴露出来、又没有内容可画，屏幕上就是一下黑闪。
///
/// 但也不能指望"区域一直在"：tao 切换可见性时的样式重写带一个
/// `SetWindowPos(SWP_FRAMECHANGED)`，**它会把自定义窗口区域整个丢掉**
/// （实测显示之后有约 14 ms 窗口完全没有裁剪，整块 2100×1290 的矩形铺在桌面上）。
/// 所以两个方向都要在事后把区域补回来，见下面 `reapply_region`。
pub fn set_visible(window: &WebviewWindow, visible: bool) -> Result<(), String> {
    if visible {
        // 显示之前先补一次：`ShowWindow` 那一瞬间窗口就已经可见了
        let _ = reapply_region(window);
        window.show().map_err(|e| e.to_string())?;
    } else {
        window.hide().map_err(|e| e.to_string())?;
    }
    // 两个方向都要重新摘一遍样式，不能只做 show：tao 每次切换可见性都会用
    // `to_window_styles()` 把窗口样式整个重写一遍，而那套样式里带着 WS_CAPTION，
    // 所以 hide() 之后样式会被**恢复**成"有标题栏"的样子躺在那里。
    // 隐藏时也摘干净，show() 的那一瞬间就没有标题栏可画。见 enforce_frameless。
    enforce_frameless(window)?;

    if visible {
        // ⚠️ 这一步不能省，也不能挪到别处 —— 它是"闪一下整个窗口"的根因。
        // 理由见函数头的注释：样式重写会把区域丢掉，补不回来的话窗口在
        // 显示后到前端下一帧 `set_panel_offset` 之间是完全没有裁剪的。
        reapply_region(window)?;
    }
    Ok(())
}

/// 把 Windows 会自己画的那套非客户区**从窗口样式里真的摘掉**。
///
/// 背景：tao 创建无边框窗口时**并没有**去掉 `WS_CAPTION`。它靠窗口过程里
/// `WM_NCCALCSIZE` 返回 0 把非客户区压成零高度，所以窗口样式里
/// `WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX` 一直在。
///
/// 这是有代价的：只要有任何一次 `WM_NCCALCSIZE` 落到 `DefWindowProcW`
/// （tao 在 `wParam == 0` 时就会这么做），系统就会按"这是个有标题栏的窗口"
/// 重新算一遍非客户区，最小化/最大化/关闭三个按钮**真的**会被画出来。
///
/// 摘下 `WS_CAPTION` 之后非客户区恒为零（实测：窗口可见时客户端矩形始终等于
/// 整个窗口矩形，一个像素的非客户区都不剩），`DefWindowProcW` 想画也没有地方画，
/// 命中测试也不可能返回 `HTCAPTION`。这是把"永远不出现系统标题栏"从
/// "但愿别触发"变成"结构上不可能"。
///
/// ⚠️ 这是一场持久战：tao 每次 show/hide 都会重写样式，所以两个方向都要重摘。
/// `set_visible` 已经把它挂在了每次切换之后。
#[cfg(target_os = "windows")]
pub fn enforce_frameless(window: &WebviewWindow) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as _;

    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let wanted = (style
            & !(WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_THICKFRAME))
            | WS_POPUP
            | WS_CLIPSIBLINGS;

        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        // WS_EX_LAYERED 必须在这里一起保住：tao 每次 show/hide 重写样式都会把它
        // 冲掉，而收起动画的淡出全靠它，见 `set_window_alpha`。
        let wanted_ex = (ex
            & !(WS_EX_WINDOWEDGE
                | WS_EX_CLIENTEDGE
                | WS_EX_DLGMODALFRAME
                | WS_EX_STATICEDGE
                | WS_EX_APPWINDOW))
            | WS_EX_TOOLWINDOW
            | WS_EX_LAYERED;

        // 没漂移就别惊动窗口：SWP_FRAMECHANGED 会触发一次重算和重绘
        if style == wanted && ex == wanted_ex {
            // 样式没漂移也要补一次透明度：`SetWindowLongW` 改扩展样式会把分层
            // 属性清掉，而 tao 自己那次重写我们无从判断内容是否一致。
            return apply_alpha(window);
        }

        SetWindowLongW(hwnd, GWL_STYLE, wanted as i32);
        SetWindowLongW(hwnd, GWL_EXSTYLE, wanted_ex as i32);
        // 不重算的话旧的（零高度的）客户区会一直沿用，直到下次尺寸变化才纠正
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    apply_alpha(window)
}

/// 当前整窗不透明度（0–255）。默认全不透明。
static WINDOW_ALPHA: AtomicU8 = AtomicU8::new(255);

/// 让**整个窗口**一起淡出 —— webview 画的内容，和 DWM 那层原生玻璃。
///
/// 收起动画的淡出必须走这里，不能走 CSS 的 `opacity`。CSS 只管得到 webview
/// 自己画的东西；`DwmEnableBlurBehindWindow` 施加的玻璃是 DWM 在窗口这一层合成
/// 的，**不参与 CSS 透明度**。于是面板淡到全透明之后那层玻璃还留在屏幕上，
/// 直到窗口隐藏才突然消失 —— 实测收起末段约 50 ms 里模块区域与桌面本底对不上
/// （白底实测差 55 个色阶，暗色桌面上看就是"一块黑"），这正是用户报告的
/// "收起后有黑色残留"。
///
/// `WS_EX_LAYERED` + `SetLayeredWindowAttributes(LWA_ALPHA)` 是唯一能把两者绑成
/// 一体的做法。实测（白底 2560×1600，窗口区域取模块内一点）：
///
/// | 状态 | 模块区域读值 |
/// |---|---|
/// | 玻璃在、不分层 | 232 |
/// | 玻璃在、分层 alpha=0 | 255（= 桌面本底，玻璃一起没了）|
/// | 玻璃在、分层 alpha=255 | 232（与不分层逐点一致，玻璃完好）|
///
/// 所以分层窗口不会把毛玻璃弄没，而 alpha=0 时连玻璃一起消失。动画因此改成
/// 逐帧发 alpha，CSS 那侧不再碰 `opacity`。
#[cfg(target_os = "windows")]
pub fn set_window_alpha(window: &WebviewWindow, alpha: u8) -> Result<(), String> {
    WINDOW_ALPHA.store(alpha, Ordering::SeqCst);
    apply_alpha(window)
}

#[cfg(not(windows))]
pub fn set_window_alpha(_window: &WebviewWindow, alpha: u8) -> Result<(), String> {
    WINDOW_ALPHA.store(alpha, Ordering::SeqCst);
    Ok(())
}

/// 把记住的不透明度真正施加到窗口上。样式被抢走时自己补回来。
///
/// ⚠️ 不能只调 `SetLayeredWindowAttributes` 就完事：tao 在 `show()` 之后约 10 ms
/// 还会用 `to_window_styles()` 重写一遍窗口样式，那套样式里没有 `WS_EX_LAYERED`
/// —— 于是分层属性被清掉，此后每一次 `SetLayeredWindowAttributes` 都会失败。
/// 表现是**淡出整个失效**（面板一路不透明，最后硬切没），而不是慢慢淡。
///
/// 所以失败时补回 `WS_EX_LAYERED` 再试一次。补样式只走 `SetWindowLongW` +
/// `SetLayeredWindowAttributes`，**不碰 `SetWindowPos(SWP_FRAMECHANGED)`** ——
/// 那一下会把窗口区域清掉（见 `set_visible` 的注释），而这条路径是动画期间
/// 每帧都可能走到的。
#[cfg(target_os = "windows")]
fn apply_alpha(window: &WebviewWindow) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, SetLayeredWindowAttributes, SetWindowLongW, GWL_EXSTYLE, LWA_ALPHA,
        WS_EX_LAYERED,
    };

    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as _;
    let alpha = WINDOW_ALPHA.load(Ordering::SeqCst);

    unsafe {
        if SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA) != 0 {
            return Ok(());
        }

        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_LAYERED == 0 {
            SetWindowLongW(hwnd, GWL_EXSTYLE, (ex | WS_EX_LAYERED) as i32);
            // 改完样式要重新下发一次属性；这里不需要 FRAMECHANGED（实测有效）
            if SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA) != 0 {
                return Ok(());
            }
        }
    }
    Err("SetLayeredWindowAttributes 失败".into())
}

#[cfg(not(windows))]
fn apply_alpha(_window: &WebviewWindow) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn enforce_frameless(_window: &WebviewWindow) -> Result<(), String> {
    Ok(())
}

/// 一个面板模块在窗口内的位置（**物理像素**，相对窗口客户区左上角）。
#[derive(Clone, Copy, serde::Deserialize)]
pub struct PanelRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub radius: i32,
}

/// 窗口区域的完整状态：**不含动画位移的基准矩形** + **当前位移**。
///
/// 两者必须存在一起。窗口每次显示都会把区域丢掉（原因见 `set_visible`），
/// 补回来的时候得用**当前**的位移 —— 只看基准就等于把动画打回原点。
///
/// 位移单独记还有一个好处：动画每帧只需要在基准上加减一个数，
/// 不必每帧再回前端重新测量一遍所有模块的坐标。
static REGION_STATE: Mutex<(Vec<PanelRect>, f64)> = Mutex::new((Vec::new(), 0.0));

pub fn remember_base(rects: &[PanelRect]) {
    let mut st = REGION_STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.0 = rects.to_vec();
}

/// 把整块面板下移 `dy` **逻辑像素** 后重新裁剪 + 重新指定模糊区域。
///
/// 动画期间这个函数每帧被调一次：原生裁剪必须跟着 CSS 的 transform 走，
/// 否则面板滑动时会被旧的区域切掉一条边。
pub fn set_region_offset(window: &WebviewWindow, dy: f64) -> Result<(), String> {
    {
        let mut st = REGION_STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.1 = dy;
    }
    reapply_region(window)
}

/// 用记住的「基准 + 当前位移」重新裁一次窗口。没有任何记住的东西时什么都不做 ——
/// 面板还没渲染出来的时候乱裁一通比不裁更糟。
pub fn reapply_region(window: &WebviewWindow) -> Result<(), String> {
    let scale = window.scale_factor().unwrap_or(1.0);
    let shifted: Vec<PanelRect> = {
        let st = REGION_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if st.0.is_empty() {
            return Ok(());
        }
        let offset = (st.1 * scale).round() as i32;
        st.0.iter().map(|r| PanelRect { y: r.y + offset, ..*r }).collect()
    };
    set_region(window, &shifted)
}

/// 把若干圆角矩形合并成一个 HRGN。调用方负责 DeleteObject。
#[cfg(target_os = "windows")]
unsafe fn build_region(rects: &[PanelRect]) -> Option<windows_sys::Win32::Graphics::Gdi::HRGN> {
    use windows_sys::Win32::Graphics::Gdi::{
        CombineRgn, CreateRoundRectRgn, DeleteObject, RGN_OR,
    };

    let first = rects.first()?;
    let region = CreateRoundRectRgn(
        first.x,
        first.y,
        first.x + first.w + 1,
        first.y + first.h + 1,
        first.radius,
        first.radius,
    );
    if region.is_null() {
        return None;
    }

    for r in &rects[1..] {
        let piece =
            CreateRoundRectRgn(r.x, r.y, r.x + r.w + 1, r.y + r.h + 1, r.radius, r.radius);
        if piece.is_null() {
            continue;
        }
        CombineRgn(region, region, piece, RGN_OR as i32);
        DeleteObject(piece);
    }
    Some(region)
}

/// 把窗口裁成若干个圆角矩形，并且**只在区域内做模糊**。
///
/// 两件事必须一起做：
///
/// 1. `SetWindowRgn` —— 缝隙处窗口根本不存在，鼠标事件直接穿透到桌面
/// 2. `DwmEnableBlurBehindWindow` 的 `hRgnBlur` —— 模糊只画在模块上
///
/// 第 2 步是关键。`window-vibrancy` 走的是 `SetWindowCompositionAttribute`，
/// 它把模糊铺满整个窗口矩形，而且**裁不掉** —— 结果就是缝隙虽然没了窗口，
/// 但那层模糊背景还在，看起来依然是一片灰。
#[cfg(target_os = "windows")]
pub fn set_region(window: &WebviewWindow, rects: &[PanelRect]) -> Result<(), String> {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmEnableBlurBehindWindow, DWM_BLURBEHIND, DWM_BB_BLURREGION, DWM_BB_ENABLE,
    };
    use windows_sys::Win32::Graphics::Gdi::SetWindowRgn;

    if rects.is_empty() {
        return Ok(());
    }

    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    let hwnd = hwnd.0 as _;

    unsafe {
        // —— 1. 裁剪窗口 ——
        // SetWindowRgn 成功后系统接管 region 所有权，不能再删
        let clip = build_region(rects).ok_or("建裁剪区域失败")?;
        if SetWindowRgn(hwnd, clip, 1) == 0 {
            windows_sys::Win32::Graphics::Gdi::DeleteObject(clip);
            return Err("SetWindowRgn 失败".into());
        }

        // —— 2. 只在同一组矩形内做模糊 ——
        // 这里用**另一个**区域对象：DWM 会在需要时自行引用它
        let blur = build_region(rects).ok_or("建模糊区域失败")?;
        let bb = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
            fEnable: 1,
            hRgnBlur: blur,
            fTransitionOnMaximized: 0,
        };
        let hr = DwmEnableBlurBehindWindow(hwnd, &bb);
        // 调用返回后 DWM 已复制一份，自己这份可以释放
        windows_sys::Win32::Graphics::Gdi::DeleteObject(blur);
        if hr < 0 {
            return Err(format!("DwmEnableBlurBehindWindow 失败：0x{hr:08X}"));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn set_region(_window: &WebviewWindow, _rects: &[PanelRect]) -> Result<(), String> {
    Ok(())
}

/// 展开后的面板尺寸（逻辑像素）。三个悬浮模块并排。
///
/// 下边缘必须留在屏幕底边——边缘唤醒依赖鼠标从底边连续移入，
/// 中间若有空隙会先触发 mouseleave 又立刻收回。所以"往屏幕中间靠"
/// 只能靠**把面板做大、上边缘抬高**来实现，不能真的居中悬浮。
/// 1400×860 在 1707×1067 的屏幕上占据 y≈207 以下，纵向覆盖中下部。
pub const PANEL_W: f64 = 1400.0;
pub const PANEL_H: f64 = 860.0;

/// 把窗口摆到当前显示器底部居中。
///
/// 两个必须处理的细节：
///
/// 1. **用实际外框尺寸回推 y**，而不是拿逻辑尺寸直接算。
///    150% 缩放下 3 逻辑像素 = 4.5 物理像素，会被取整成 5 ——
///    直接算会让窗口底边探出屏幕 1px，落在屏幕之外。
///
/// 2. **每次布局后重新声明置顶**。任务栏同样是 topmost 窗口，它会重新
///    抢到 z 序顶端，把面板下沿整条盖住 —— 那样边缘唤醒永远不会触发，
///    因为鼠标事件全被任务栏吃掉了。
pub fn layout(window: &WebviewWindow) -> Result<(), String> {
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("找不到当前显示器")?;

    let scale = monitor.scale_factor();
    let screen = monitor.size().to_logical::<f64>(scale);
    let origin = monitor.position().to_logical::<f64>(scale);

    // 屏幕比面板窄时（小屏/分屏）收缩到屏幕宽度，避免横向溢出
    let w = PANEL_W.min(screen.width);
    let h = PANEL_H.min(screen.height);

    // 尺寸到顶就不要再发一次 set_size：透明窗口重设尺寸会让新露出来的
    // 那一块先以未绘制状态出现（就是"黑闪"）。收起不改尺寸，
    // 所以这里能省则省。
    let current = window
        .inner_size()
        .map(|s| s.to_logical::<f64>(scale))
        .unwrap_or(LogicalSize::new(0.0, 0.0));
    if (current.width - w).abs() > 0.5 || (current.height - h).abs() > 0.5 {
        window.set_size(LogicalSize::new(w, h)).map_err(|e| e.to_string())?;
    }

    // 读回实际外框高度来定位，绕开 DPI 取整
    let outer_h = window
        .outer_size()
        .map(|s| s.to_logical::<f64>(scale).height)
        .unwrap_or(h);

    window
        .set_position(LogicalPosition::new(
            origin.x + (screen.width - w) / 2.0,
            origin.y + screen.height - outer_h,
        ))
        .map_err(|e| e.to_string())?;

    // 任务栏会重新抢 topmost；每次布局后重新声明一次
    let _ = window.set_always_on_top(true);
    // 布局会触发 tao 重算窗口样式，顺手确认一次无边框没有被带回来
    enforce_frameless(window)?;

    Ok(())
}

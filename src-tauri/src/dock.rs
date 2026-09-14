use tauri::{LogicalPosition, LogicalSize, WebviewWindow};

/// 不显示时把窗口**整个隐藏**，而不是缩成一条细带。
///
/// 缩成细带的做法已经证明行不通：只要窗口还在屏幕上，webview 就会画自己的
/// 底色，那一条再细也是可见的像素块。而"平时感知不到应用存在"要求是零像素。
///
/// 代价是隐藏后收不到任何鼠标事件 —— 唤醒改由全局轮询光标位置负责，
/// 见 `main.rs` 里的 `spawn_edge_watcher`。
pub fn set_visible(window: &WebviewWindow, visible: bool) -> Result<(), String> {
    if visible {
        window.show()
    } else {
        window.hide()
    }
    .map_err(|e| e.to_string())
}

/// 一个面板模块在窗口内的位置（**物理像素**，相对窗口客户区左上角）。
#[derive(serde::Deserialize)]
pub struct PanelRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub radius: i32,
}

/// 把窗口裁成若干个圆角矩形 —— 缝隙处窗口**根本不存在**。
///
/// 这是"面板真模糊 + 缝隙真透明"能在单窗口下同时成立的唯一办法：
/// `apply_blur` 是窗口级效果，会把整个窗口矩形（含缝隙）都模糊掉。
/// 只有用窗口区域把缝隙挖掉，DWM 的模糊背景才会被一并裁掉，
/// 缝隙处直接看到未处理的桌面。
///
/// 区域坐标必须是物理像素，且相对窗口客户区——前端用
/// `getBoundingClientRect()` × `devicePixelRatio` 量出来传进来，
/// 避免在 Rust 里重复一份 CSS 布局知识。
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

/// 收起时清掉区域裁剪和模糊，让窗口恢复成一条无效果的透明细带
#[cfg(target_os = "windows")]
pub fn clear_region(window: &WebviewWindow) -> Result<(), String> {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmEnableBlurBehindWindow, DWM_BLURBEHIND, DWM_BB_ENABLE,
    };
    use windows_sys::Win32::Graphics::Gdi::SetWindowRgn;

    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    let hwnd = hwnd.0 as _;
    unsafe {
        SetWindowRgn(hwnd, std::ptr::null_mut(), 1);
        let bb = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE,
            fEnable: 0,
            hRgnBlur: std::ptr::null_mut(),
            fTransitionOnMaximized: 0,
        };
        DwmEnableBlurBehindWindow(hwnd, &bb);
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn set_region(_window: &WebviewWindow, _rects: &[PanelRect]) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn clear_region(_window: &WebviewWindow) -> Result<(), String> {
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
/// 收起后只剩屏幕底边一条**完全透明**的感应带。
/// 3px 是"能稳定命中鼠标"和"人眼看不见"之间的折中 —— 它的背景由
/// CSS 保证全透明，亚克力也在收起时被清掉，所以屏幕上不该有任何像素变化。
pub const STRIP_H: f64 = 3.0;

/// 把窗口摆到当前显示器底部居中，并按展开/收起切换高度。
///
/// 两个必须处理的细节：
///
/// 1. **用实际外框尺寸回推 y**，而不是拿逻辑尺寸直接算。
///    150% 缩放下 3 逻辑像素 = 4.5 物理像素，会被取整成 5 ——
///    直接算会让窗口底边探出屏幕 1px，落在屏幕之外。
///
/// 2. **每次布局后重新声明置顶**。任务栏同样是 topmost 窗口，它会重新
///    抢到 z 序顶端，把我们的感应带整条盖住 —— 那样边缘唤醒永远不会触发，
///    因为鼠标事件全被任务栏吃掉了。
pub fn layout(window: &WebviewWindow, expanded: bool) -> Result<(), String> {
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("找不到当前显示器")?;

    let scale = monitor.scale_factor();
    let screen = monitor.size().to_logical::<f64>(scale);
    let origin = monitor.position().to_logical::<f64>(scale);

    // 屏幕比面板窄时（小屏/分屏）收缩到屏幕宽度，避免横向溢出
    let w = PANEL_W.min(screen.width);
    let h = if expanded { PANEL_H.min(screen.height) } else { STRIP_H };

    window
        .set_size(LogicalSize::new(w, h))
        .map_err(|e| e.to_string())?;

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

    Ok(())
}

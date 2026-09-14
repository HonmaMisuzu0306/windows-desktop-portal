# Windows Desktop Portal

> 让桌面保持干净，同时不移动任何原始文件。

贴在屏幕底部的 Windows 桌面入口管理器。平时完全不可见，鼠标碰到屏幕底边时滑出，
把桌面上的东西按分类收纳，点一下直接打开。

**它不是文件管理器，也不是桌面美化工具。** 它是一个纯显示层：所有桌面内容通过 Portal 访问，
但文件的真实位置从未改变。

> 📷 **截图待补** —— 想加截图就放到 `docs/screenshot-main.png`（详见 `docs/README.md`）。
> 注意：截图会暴露桌面上安装的软件。

---

## 产品原则

这四条是整个项目的地基，任何版本都不会违反：

1. **不移动任何文件**
2. **不修改任何文件路径**
3. **不复制文件**
4. **不破坏快捷方式**

所有内容只保存**映射关系**。桌面文件永远是真实来源，Portal 只是显示层。

**随时可以退出** —— 删掉配置目录或卸载程序，一切恢复原状；磁盘上的文件从未被触碰。

---

## 功能

### 桌面入口管理

- **自动扫描** Windows 桌面（用户桌面 + 公共桌面），用 `SHGetKnownFolderPath` 解析路径，
  能正确处理 OneDrive 重定向
- **自动导入** 桌面上的文件夹、快捷方式、文件
- **全量同步** —— 桌面新增的自动进来，桌面删掉的自动移除
- **拖拽归类** —— 卡片拖到左侧导航即可改分类，只改映射、不碰文件
- **忽略名单** —— 在 Portal 里移除的桌面条目不再自动导入
- **最近访问** —— 记录最近打开的 20 条

### 图标

**全部使用 Windows 原生图标**，不拿 Emoji 糊弄：

| 类型 | 来源 |
| --- | --- |
| `.lnk` | shell 解析到**快捷方式指向的目标**的图标 |
| `.exe` | 提取可执行文件内嵌的图标资源 |
| 文件夹 | 系统文件夹图标 |
| 其他 | 按扩展名关联的图标 |

### 系统集成

- **隐藏桌面图标** —— 系统级开关，**退出程序时自动恢复**；即使被强杀，下次启动也会对账恢复
- **开机启动**
- 边缘唤醒，收起时窗口整个隐藏，屏幕上零像素残留

### 界面

Windows 11 Fluent 浅色主题。三个独立悬浮模块：**分类导航 / 内容区 / 最近访问**。

---

## 安装

从 [Releases](../../releases) 下载 `Windows.Desktop.Portal_0.1.0_x64-setup.exe` 安装。

安装包会一并装好 WebView2 依赖检测与开机启动项（可在面板里关掉）。

### 手动恢复桌面图标

万一程序异常退出、桌面图标没恢复：

> **右键桌面 → 查看 → 勾上「显示桌面图标」**

---

## 开发

### 环境要求

| 依赖 | 说明 |
| --- | --- |
| Node.js ≥ 18 | 前端构建 |
| Rust (stable) | 后端编译 |
| **MSVC C++ 生成工具** | **Windows 上 Rust 链接器，必需** |
| WebView2 Runtime | Win11 自带 |

Rust 在 Windows 默认用 `x86_64-pc-windows-msvc` 工具链，链接阶段依赖 MSVC 的 `link.exe`。
没装会报 `linker 'link.exe' not found`。**需要管理员权限**：

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools `
  --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

或右键以管理员身份运行 `scripts/install-msvc.bat`。

### 常用命令

```bash
npm install
npm run tauri dev        # 开发
npm run tauri build      # 打包 → src-tauri/target/release/bundle/nsis/

cd src-tauri && cargo test    # 19 个单测
npx tsc --noEmit              # 前端类型检查
```

---

## 配置

`%APPDATA%\com.uidock.desktop\config.json`（点面板左下角 `⋯` 直接打开该目录）

```json
{
  "schema_version": 2,
  "categories": [
    { "id": "desktop", "name": "桌面", "items": [
      { "id": "d1a2b3…", "name": "某程序",
        "path": "C:\\Users\\me\\Desktop\\某程序.lnk",
        "kind": "app", "source": "desktop" }
    ]}
  ],
  "recent": [{ "name": "某程序", "path": "…", "at": 1757000000 }],
  "settings": { "autostart": false, "autoHide": true,
                "hideDesktopIcons": false, "desktopIconsOwnedByUs": false,
                "desktopIconsWereVisible": true },
  "ignored": []
}
```

**字段风格是混合的**：`AppConfig` 没有 `rename_all`，键名就是 Rust 字段原名
（`schema_version`）；只有 `Settings` 是 camelCase（`autoHide`）。`src/types.ts` 是手写的，
两边必须手工同步。

### 数据安全

- **原子写**：写 `.tmp` + `fsync`，留一代 `.bak`，再 `rename` 替换
- **损坏不静默重置**：解析失败时把原文件隔离为 `config.corrupt-<时间戳>.json`，
  所有写操作返回错误、进入只读模式，**用户数据原样躺在磁盘上**
- **迁移快照**：schema 升级前留一份 `config.v<旧版本>.json`

---

## 实现要点

踩过坑之后总结的，改代码前建议先读。

### 窗口与合成

| 问题 | 处理 |
| --- | --- |
| Mica / Acrylic **失焦退化** —— 窗口失焦时 Windows 换成不透明实色 | 不用。观感会在焦点变化时反复横跳 |
| `SetWindowCompositionAttribute` 把模糊铺满整窗、**裁不掉** | 不用。模块之间的缝隙会一直是灰的 |
| `set_background_color(alpha=0)` 会让 **webview 完全不渲染** | 不用 |
| 收起时残留可见像素块 | **隐藏窗口**，不是缩成细带；唤醒改由轮询光标负责 |
| 窗口底边因 DPI 取整探出屏幕 | 用 `outer_size()` 实际外框回推位置 |
| 任务栏重新抢 topmost | 每次布局后重新声明置顶 |

### 同步

- **扫描失败 → 整次中止**，绝不当成"空目录"。否则会以为桌面被清空、删光所有桌面条目
- **批量删除保护**：启动时的隐式同步要删掉超过一半且多于 10 条时放弃本次修改。
  防的是 OneDrive 迁移导致 `FOLDERID_Desktop` 静默指向新目录。口诀：**隐式保守，显式服从**
- 三条不变式写在 `sync.rs` 顶部：同步**永不改写已有条目的 `name`**（用户重命名必须存活）、
  只增删 `source == Desktop` 的条目、`path` 归一化后全局唯一

### 前端

- **副作用不能写在 `setState` 的 updater 里**。React 判定状态未变时会 bail out、
  根本不调用 updater —— 曾因此导致面板永久打不开
- **`pointerup` 必须读 ref 而非 state**。pointermove 的 re-render 和 pointerup 可能同帧
- **`pointerdown` 必须排除 `input, .acts`**，否则重命名框和按钮全失效
- **测量 DOM 的 effect 要把 `config` 放进依赖**。挂载时 config 还是 null，页面上没有元素

### 平台

- `SHGetFileInfoW` 被 `Win32_Storage_FileSystem` feature 门控 —— 不开这个 feature 它就不存在
- `#[tauri::command]`（非 async）在 Tauri 2 里是**主线程内联执行**，读-改-写不会交错、
  不需要 Mutex。**改成 `async fn` 会引入丢失更新**
- `EnumWindows` 回调里零 `unwrap`、零索引 —— 跨 FFI unwind 是 UB

---

## Roadmap

### v0.1 — 当前版本

- [x] 桌面自动扫描与导入
- [x] 全量同步 + 忽略名单
- [x] 拖拽归类
- [x] Windows 原生图标
- [x] 隐藏 / 恢复桌面图标
- [x] 边缘唤醒，收起零像素
- [x] 最近访问
- [x] 开机启动

### 后续

- [ ] **收藏夹** —— 跨分类的置顶集合
- [ ] **搜索** —— 按名称快速定位
- [ ] **真实大图标** —— 改用 `SHIL_EXTRALARGE`（48×48）改善高 DPI 观感
- [ ] **多显示器** —— 目前只处理当前显示器
- [ ] **模块间缝隙完全透明** —— 需要窗口级模糊支持按区域裁剪
- [ ] **条目手动排序**
- [ ] **i18n** —— 目前界面为中文

---

## 目录结构

```
src/                      前端
  App.tsx                 外壳：导航 / 拖拽状态机 / 悬停展开
  api.ts                  invoke 封装
  types.ts                与 Rust 结构体手工对齐的类型
  components/
    ItemGrid.tsx          卡片网格 + 原生图标
    RecentList.tsx        最近访问
  styles.css              Fluent 浅色主题
src-tauri/src/            后端
  main.rs                 命令层 + 数据安全网 + 光标轮询
  config.rs               数据模型 / 迁移 / 归一化
  desktop.rs              桌面扫描
  sync.rs                 全量同步（纯函数，可单测）
  icons.rs                Windows 原生图标提取
  dock.rs                 窗口几何 / 区域裁剪
  desktop_icons.rs        桌面图标隐藏（系统级）
scripts/
  install-msvc.bat        装 MSVC 生成工具
  make-icon.mjs           生成应用图标源图
```

---

## License

[MIT](LICENSE)

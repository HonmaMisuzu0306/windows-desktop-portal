import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, IconEntry } from "./types";

// 所有写操作都在 Rust 侧落盘，并回传最新配置，前端不做本地状态推演。

export const loadConfig = () => invoke<AppConfig>("load_config");

export const addPaths = (categoryId: string, paths: string[]) =>
  invoke<AppConfig>("add_paths", { categoryId, paths });

export const removeItem = (categoryId: string, itemId: string) =>
  invoke<AppConfig>("remove_item", { categoryId, itemId });

export const renameItem = (categoryId: string, itemId: string, name: string) =>
  invoke<AppConfig>("rename_item", { categoryId, itemId, name });

/** 打开目标并记录到最近访问 */
export const openItem = (path: string) => invoke<AppConfig>("open_item", { path });

export const clearRecent = () => invoke<AppConfig>("clear_recent");

export const setSettings = (autoHide: boolean, autostart: boolean) =>
  invoke<AppConfig>("set_settings", { autoHide, autostart });

export const setDockExpanded = (expanded: boolean) =>
  invoke<void>("set_dock_expanded", { expanded });

export const openConfigFolder = () => invoke<void>("open_config_folder");

/** 重新扫描桌面并全量同步。用户主动触发，不走批量删除保护。 */
export const refreshDesktop = () => invoke<AppConfig>("refresh_desktop");

/** 把条目移到另一个分类。只改归属，不碰文件。 */
export const moveItem = (itemId: string, toCategoryId: string) =>
  invoke<AppConfig>("move_item", { itemId, toCategoryId });

/** 隐藏/显示桌面图标。系统级设置，失败会 reject。 */
export const setHideDesktopIcons = (hidden: boolean) =>
  invoke<AppConfig>("set_hide_desktop_icons", { hidden });

/** 挂载时调一次：崩溃恢复 + Explorer 重启后重新施加隐藏。 */
export const reconcileDesktopIcons = () => invoke<AppConfig>("reconcile_desktop_icons");

/** 正常退出。退出时会自动恢复桌面图标。 */
export const quitApp = () => invoke<void>("quit_app");

/** 批量取 Windows 原生图标。按当前分类调用，取不到的 data 为 null。 */
export const getIcons = (paths: string[]) => invoke<IconEntry[]>("get_icons", { paths });

/** 面板矩形（物理像素）。把窗口裁成三块，缝隙处窗口不存在 → 露出未模糊的桌面。 */
export interface PanelRect {
  x: number;
  y: number;
  w: number;
  h: number;
  radius: number;
}

export const setPanelRegions = (rects: PanelRect[]) =>
  invoke<void>("set_panel_regions", { rects });

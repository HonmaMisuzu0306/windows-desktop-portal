export type ItemKind = "folder" | "app" | "file";

/** 条目来源。决定它归谁管：desktop 的由同步引擎掌管，manual 的同步永不触碰。 */
export type ItemSource = "manual" | "desktop";

export interface Item {
  id: string;
  name: string;
  path: string;
  kind: ItemKind;
  source: ItemSource;
}

export interface Category {
  id: string;
  name: string;
  items: Item[];
}

export interface Recent {
  name: string;
  path: string;
  /** Unix 秒 */
  at: number;
}

export interface Settings {
  autostart: boolean;
  autoHide: boolean;
  hideDesktopIcons: boolean;
  desktopIconsOwnedByUs: boolean;
  desktopIconsWereVisible: boolean;
}

// 注意：只有 Settings 是 camelCase，AppConfig 没有 rename 属性，
// 所以键名就是 Rust 字段名原样（schema_version 保持下划线）。
// 两边必须手工保持同步 —— 没有代码生成。
/** get_icons 的返回项。data 为 `data:image/png;base64,...`，取不到时为 null。 */
export interface IconEntry {
  path: string;
  data: string | null;
}

export interface AppConfig {
  schema_version: number;
  categories: Category[];
  recent: Recent[];
  settings: Settings;
  /** 被用户移除、不再自动导入的桌面路径（归一化后存放） */
  ignored: string[];
}

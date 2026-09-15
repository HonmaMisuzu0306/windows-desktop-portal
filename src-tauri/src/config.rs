use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// 桌面映射分类。由同步引擎掌管，始终排在最前。
pub const DESKTOP_CATEGORY: (&str, &str) = ("desktop", "桌面");

/// 四个固定的人工分类。它们和「桌面」一样由 `normalize()` 保证存在，不允许改名或删除。
pub const DEFAULT_CATEGORIES: [(&str, &str); 4] =
    [("study", "学习"), ("project", "项目"), ("mad", "MAD"), ("fun", "娱乐")];

/// 用户自建收藏夹的最大名字长度（**字符数**，不是字节数）。
/// 侧栏只有 196px 宽，放任下去只会让界面变成一坨。
pub const MAX_CATEGORY_NAME: usize = 10;

pub const MAX_RECENT: usize = 20;

/// 当前配置结构版本。旧配置没有这个键，视为 1。
pub const SCHEMA_VERSION: u32 = 2;

fn default_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Folder,
    App,
    File,
}

/// 条目的来源。决定它归谁管。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemSource {
    /// 用户手动拖进来的。同步引擎永不触碰。
    Manual,
    /// 桌面扫描的产物。生命周期由同步引擎掌管。
    Desktop,
}

impl Default for ItemSource {
    /// 旧配置没有 source 字段 → 一律视为 Manual → 同步引擎不会碰它们，条目一条不丢。
    fn default() -> Self {
        ItemSource::Manual
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub path: String,
    pub kind: ItemKind,
    // 注意：只给这一个字段加 default，不要给 Item 加容器级 default。
    // id/path 缺失的条目是垃圾数据，宁可让整个配置走隔离流程，
    // 也不要静默产生一堆空路径条目。
    #[serde(default)]
    pub source: ItemSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recent {
    pub name: String,
    pub path: String,
    pub at: u64,
}

/// 容器级 `default` + `Default` impl：以后再加字段不会再让旧配置整体解析失败。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub autostart: bool,
    pub auto_hide: bool,
    /// 用户是否要求隐藏桌面图标
    pub hide_desktop_icons: bool,
    /// 桌面图标当前被隐藏，是不是我们干的。只有我们实际动过手才为 true。
    pub desktop_icons_owned_by_us: bool,
    /// 我们动手之前，图标本来是可见的吗
    pub desktop_icons_were_visible: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autostart: false,
            auto_hide: true,
            hide_desktop_icons: false,
            desktop_icons_owned_by_us: false,
            desktop_icons_were_visible: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub categories: Vec<Category>,
    pub recent: Vec<Recent>,
    pub settings: Settings,
    /// 被用户从 Dock 移除、不再自动导入的桌面路径（归一化后存放）
    #[serde(default)]
    pub ignored: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            categories: std::iter::once(DESKTOP_CATEGORY)
                .chain(DEFAULT_CATEGORIES)
                .map(|(id, name)| Category {
                    id: id.to_string(),
                    name: name.to_string(),
                    items: Vec::new(),
                })
                .collect(),
            recent: Vec::new(),
            settings: Settings::default(),
            ignored: Vec::new(),
        }
    }
}

impl AppConfig {
    /// 结构性不变式的兜底，每次 `read()` 都会跑，**必须幂等**。
    ///
    /// 只维护两件事：分类齐全且顺序规范、「忽略名单」去重。
    /// 不碰条目内容——排序是同步引擎的事（见 desktop.rs）。
    pub fn normalize(&mut self) {
        // 规范顺序：桌面在最前，随后四个默认分类
        let wanted: Vec<(&str, &str)> = std::iter::once(DESKTOP_CATEGORY)
            .chain(DEFAULT_CATEGORIES)
            .collect();

        // 补齐缺失的（旧配置没有 desktop → 在这里被补出来）
        for (id, name) in &wanted {
            if !self.categories.iter().any(|c| c.id == *id) {
                self.categories.push(Category {
                    id: (*id).to_string(),
                    name: (*name).to_string(),
                    items: Vec::new(),
                });
            }
        }

        // 重排：已知分类按 wanted 顺序抽出，未知分类保持相对顺序跟在后面。
        // 每轮都用 position 重新查找，不缓存下标。
        let mut ordered: Vec<Category> = Vec::with_capacity(self.categories.len());
        for (id, _) in &wanted {
            if let Some(pos) = self.categories.iter().position(|c| c.id == *id) {
                ordered.push(self.categories.remove(pos));
            }
        }
        ordered.append(&mut self.categories);
        self.categories = ordered;

        // 忽略名单去重，保留首次出现的顺序
        let mut seen: HashSet<String> = HashSet::new();
        self.ignored.retain(|p| seen.insert(norm_path(p)));

        self.recent.truncate(MAX_RECENT);
    }
}

/// 桌面条目的身份键。纯字符串处理，**绝不要用 `canonicalize()`**：
/// 它会碰盘、对刚删掉的路径直接失败、还会把 8.3 短名展开。
pub fn norm_path(p: &str) -> String {
    p.replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 桌面条目的 id 由路径派生：删掉再加回来还是同一个 id，
/// 配置 diff 稳定，「最近」列表里的引用也不会失效。
/// 不用 `uid()` 的纳秒时间戳——一次同步批量加 90 条时会有碰撞风险。
pub fn desktop_id(path: &str) -> String {
    format!("d{:016x}", fnv1a64(norm_path(path).as_bytes()))
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

pub fn uid() -> String {
    format!("i{}", nanos())
}

/// 用户自建收藏夹的 id。前缀 `u` 让"这是用户建的"在 config.json 里肉眼可辨，
/// 不必靠跟固定表比对才能看出来。
pub fn category_uid() -> String {
    format!("u{}", nanos())
}

/// 内置分类（「桌面」+ 四个固定分类）。它们由 `normalize()` 保证存在，
/// 因此既不该被删除，也不该被改名 —— 改了下次加载会被 `normalize()` 用回原名，
/// 表现为"改了又自己变回去"。
pub fn is_builtin_category(id: &str) -> bool {
    id == DESKTOP_CATEGORY.0 || DEFAULT_CATEGORIES.iter().any(|(i, _)| *i == id)
}

/// 收藏夹名字的归一化与校验。空名和超长名都在这里挡掉。
/// **只重排空白，不做任何静默截断** —— 截断会让用户看着自己输入的名字被改掉。
fn clean_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("名字不能为空".into());
    }
    if name.chars().count() > MAX_CATEGORY_NAME {
        return Err(format!("名字最多 {MAX_CATEGORY_NAME} 个字"));
    }
    Ok(name.to_string())
}

/// 两个收藏夹同名会让侧栏看起来像出了 bug，所以直接拒绝重名。
fn name_taken(cfg: &AppConfig, name: &str, except_id: Option<&str>) -> bool {
    cfg.categories
        .iter()
        .any(|c| c.name == name && Some(c.id.as_str()) != except_id)
}

/// 新建收藏夹，返回它的 id。**只建一个空的映射容器，不碰文件系统。**
pub fn add_category(cfg: &mut AppConfig, raw_name: &str) -> Result<String, String> {
    let name = clean_name(raw_name)?;
    if name_taken(cfg, &name, None) {
        return Err(format!("已经有叫「{name}」的分类了"));
    }
    // 纳秒理论上可能撞（同一纳秒内连建两个），撞了就再取一次。
    let mut id = category_uid();
    while cfg.categories.iter().any(|c| c.id == id) {
        id = category_uid();
    }
    cfg.categories.push(Category { id: id.clone(), name, items: Vec::new() });
    Ok(id)
}

pub fn rename_category(cfg: &mut AppConfig, id: &str, raw_name: &str) -> Result<(), String> {
    if is_builtin_category(id) {
        return Err("内置分类不能改名".into());
    }
    let name = clean_name(raw_name)?;
    if name_taken(cfg, &name, Some(id)) {
        return Err(format!("已经有叫「{name}」的分类了"));
    }
    let Some(cat) = cfg.categories.iter_mut().find(|c| c.id == id) else {
        return Err("分类不存在".into());
    };
    cat.name = name;
    Ok(())
}

/// 删除收藏夹。**只删映射，不动任何原始文件。**
///
/// 里面的 `Desktop` 条目不需要写进忽略名单：下次同步会发现它们"没被任何分类认领"，
/// 于是重新放回「桌面」分类 —— 那正是"这个收藏夹没了，但文件还在"应有的样子。
/// 写进忽略名单反而会让它们永远回不来。
pub fn remove_category(cfg: &mut AppConfig, id: &str) -> Result<(), String> {
    if is_builtin_category(id) {
        return Err("内置分类不能删除".into());
    }
    let before = cfg.categories.len();
    cfg.categories.retain(|c| c.id != id);
    if cfg.categories.len() == before {
        return Err("分类不存在".into());
    }
    Ok(())
}

pub fn detect_kind(path: &str) -> ItemKind {
    let p = Path::new(path);
    if p.is_dir() {
        return ItemKind::Folder;
    }
    match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "exe" | "lnk" | "bat" | "cmd" | "msi" | "url" => ItemKind::App,
        _ => ItemKind::File,
    }
}

/// 展示名：程序去掉扩展名（`Code.exe` → `Code`），文件夹和文档保留原名。
pub fn display_name(path: &str) -> String {
    let fallback = path.to_string();
    let base = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(fallback);
    if detect_kind(path) == ItemKind::App {
        Path::new(&base)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(base)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全案价值最高的一段测试：把"旧配置缺字段导致整体解析失败、
    /// 用户数据被静默清空"这个最高危的失效模式变成 CI 能抓的错误。
    #[test]
    fn v1_config_migrates_without_data_loss() {
        let v1 = r#"{
            "categories": [
                {"id":"study","name":"学习","items":[
                    {"id":"i1","name":"高数","path":"D:\\课程\\高数","kind":"folder"}
                ]},
                {"id":"project","name":"项目","items":[]},
                {"id":"mad","name":"MAD","items":[]},
                {"id":"fun","name":"娱乐","items":[
                    {"id":"i2","name":"Steam","path":"C:\\x\\steam.exe","kind":"app"}
                ]}
            ],
            "recent": [{"name":"高数","path":"D:\\课程\\高数","at":1757000000}],
            "settings": {"autostart":false,"autoHide":true}
        }"#;

        let mut cfg: AppConfig = serde_json::from_str(v1).expect("v1 配置必须能解析");
        cfg.normalize();

        assert_eq!(cfg.schema_version, 1, "旧配置没有版本号 → 视为 1");
        assert!(cfg.ignored.is_empty());

        let study = cfg.categories.iter().find(|c| c.id == "study").unwrap();
        assert_eq!(study.items.len(), 1, "旧条目一条都不能丢");
        assert_eq!(study.items[0].name, "高数");
        assert_eq!(
            study.items[0].source,
            ItemSource::Manual,
            "旧条目必须视为 Manual，否则会被同步引擎误删"
        );

        let fun = cfg.categories.iter().find(|c| c.id == "fun").unwrap();
        assert_eq!(fun.items.len(), 1, "其他分类的条目同样不能丢");

        assert!(
            cfg.categories.iter().any(|c| c.id == DESKTOP_CATEGORY.0),
            "缺失的「桌面」分类应被补出来"
        );
        assert_eq!(cfg.recent.len(), 1);

        // 新增的 settings 字段必须有默认值，否则整个配置会解析失败
        assert!(!cfg.settings.hide_desktop_icons);
        assert!(!cfg.settings.desktop_icons_owned_by_us);
        assert!(cfg.settings.auto_hide, "原有字段值必须保留");
    }

    #[test]
    fn normalize_is_idempotent_and_puts_desktop_first() {
        let mut cfg = AppConfig::default();
        cfg.normalize();

        assert_eq!(cfg.categories.first().unwrap().id, DESKTOP_CATEGORY.0);

        let before: Vec<String> = cfg.categories.iter().map(|c| c.id.clone()).collect();
        cfg.normalize();
        let after: Vec<String> = cfg.categories.iter().map(|c| c.id.clone()).collect();
        assert_eq!(before, after, "normalize 必须幂等");
    }

    #[test]
    fn normalize_reorders_when_desktop_is_last() {
        // 模拟被手改坏、或旧版本写出来的配置
        let mut cfg = AppConfig::default();
        let pos = cfg.categories.iter().position(|c| c.id == DESKTOP_CATEGORY.0).unwrap();
        let d = cfg.categories.remove(pos);
        cfg.categories.push(d);

        cfg.normalize();
        assert_eq!(cfg.categories.first().unwrap().id, DESKTOP_CATEGORY.0);
    }

    #[test]
    fn normalize_keeps_unknown_categories_after_known_ones() {
        let mut cfg = AppConfig::default();
        cfg.categories.push(Category {
            id: "custom".into(),
            name: "自定义".into(),
            items: Vec::new(),
        });
        cfg.normalize();

        let ids: Vec<&str> = cfg.categories.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids[0], DESKTOP_CATEGORY.0);
        assert_eq!(ids[ids.len() - 1], "custom", "未知分类应保留且排在已知分类之后");
        assert!(ids.contains(&"study"));
    }

    #[test]
    fn user_categories_keep_their_order_through_normalize() {
        let mut cfg = AppConfig::default();
        add_category(&mut cfg, "摄影").unwrap();
        add_category(&mut cfg, "实习").unwrap();
        add_category(&mut cfg, "MAD工程").unwrap();

        cfg.normalize();

        let tail: Vec<&str> = cfg.categories[5..].iter().map(|c| c.name.as_str()).collect();
        assert_eq!(tail, ["摄影", "实习", "MAD工程"], "收藏夹顺序是用户自己排的，不能被重排");
        assert_eq!(cfg.categories.len(), 8, "四个固定 + 桌面 + 三个收藏夹");
    }

    #[test]
    fn add_category_rejects_blank_long_and_duplicate_names() {
        let mut cfg = AppConfig::default();

        assert!(add_category(&mut cfg, "   ").is_err(), "空名要挡掉");
        assert!(add_category(&mut cfg, &"字".repeat(MAX_CATEGORY_NAME + 1)).is_err());
        assert!(add_category(&mut cfg, "学习").is_err(), "不能和内置分类重名");

        add_category(&mut cfg, "摄影").unwrap();
        assert!(add_category(&mut cfg, "摄影").is_err(), "不能重名");
        assert!(add_category(&mut cfg, " 摄影 ").is_err(), "去掉空白后重名同样要挡");
        assert_eq!(cfg.categories.len(), 6, "被拒绝的三次一个都不该进配置");
    }

    #[test]
    fn rename_cannot_touch_builtin_categories() {
        let mut cfg = AppConfig::default();
        for id in ["desktop", "study", "project", "mad", "fun"] {
            assert!(is_builtin_category(id));
            assert!(rename_category(&mut cfg, id, "新名字").is_err());
            assert!(remove_category(&mut cfg, id).is_err());
        }
    }

    /// 删收藏夹只删映射，文件不动；里面的桌面条目下次同步会被放回「桌面」。
    #[test]
    fn removing_category_only_drops_the_mapping() {
        let mut cfg = AppConfig::default();
        let id = add_category(&mut cfg, "摄影").unwrap();
        cfg.categories
            .iter_mut()
            .find(|c| c.id == id)
            .unwrap()
            .items
            .push(Item {
                id: "d1".into(),
                name: "照片".into(),
                path: "C:\\Users\\A\\Desktop\\照片".into(),
                kind: ItemKind::Folder,
                source: ItemSource::Desktop,
            });

        remove_category(&mut cfg, &id).unwrap();

        assert!(!cfg.categories.iter().any(|c| c.id == id));
        assert!(cfg.ignored.is_empty(), "绝不能把桌面条目写进忽略名单，否则它永远回不到「桌面」");

        // 下次同步：没被任何分类认领 → 重新放回「桌面」
        let scanned = vec![crate::desktop::Scanned {
            path: "C:\\Users\\A\\Desktop\\照片".into(),
            name: "照片".into(),
            kind: ItemKind::Folder,
        }];
        crate::sync::sync(&mut cfg, &scanned);
        let desktop = cfg.categories.iter().find(|c| c.id == DESKTOP_CATEGORY.0).unwrap();
        assert_eq!(desktop.items.len(), 1, "文件还在，就应该重新出现在「桌面」里");
    }

    #[test]
    fn norm_path_ignores_separators_case_and_trailing_slash() {
        assert_eq!(
            norm_path("C:\\Users\\A\\Desktop"),
            norm_path("c:/users/a/desktop/")
        );
    }

    #[test]
    fn desktop_id_is_stable_across_path_spellings() {
        let a = desktop_id("C:\\Users\\A\\Desktop\\微信.lnk");
        let b = desktop_id("c:/users/a/desktop/微信.lnk");
        assert_eq!(a, b, "同一路径的不同写法必须得到同一个 id");
        assert_ne!(a, desktop_id("C:\\Users\\A\\Desktop\\QQ.lnk"));
    }

    /// 钉死"线格式"。AppConfig 没有 rename 属性而 Settings 有，
    /// 这种混合风格写错了前端会**静默**读不到值（不报错，只是 undefined）。
    #[test]
    fn wire_format_matches_frontend_types() {
        let mut cfg = AppConfig::default();
        cfg.categories[0].items.push(Item {
            id: "x".into(),
            name: "n".into(),
            path: "p".into(),
            kind: ItemKind::Folder,
            source: ItemSource::Desktop,
        });
        let j = serde_json::to_value(&cfg).unwrap();

        // AppConfig：字段名原样，下划线不变驼峰
        assert!(j.get("schema_version").is_some(), "AppConfig 无 rename → schema_version");
        assert!(j.get("ignored").is_some());
        assert!(j.get("categories").is_some());

        // Settings：camelCase
        let s = j.get("settings").unwrap();
        assert!(s.get("autoHide").is_some(), "Settings 是 camelCase → autoHide");
        assert!(s.get("hideDesktopIcons").is_some());
        assert!(s.get("desktopIconsOwnedByUs").is_some());
        assert!(s.get("desktopIconsWereVisible").is_some());
        assert!(s.get("auto_hide").is_none(), "不该出现下划线写法");

        // Item：source 小写枚举
        assert_eq!(j["categories"][0]["items"][0]["source"], "desktop");
        assert_eq!(j["categories"][0]["items"][0]["kind"], "folder");
    }

    #[test]
    fn ignored_list_is_deduped_by_normalized_path() {
        let mut cfg = AppConfig::default();
        cfg.ignored = vec![
            "C:\\a\\b".into(),
            "c:/A/B/".into(), // 归一化后与上一条相同
            "C:\\c".into(),
        ];
        cfg.normalize();
        assert_eq!(cfg.ignored.len(), 2);
    }
}

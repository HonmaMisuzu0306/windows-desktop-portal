//! 桌面全量同步。纯函数：不碰文件系统、不碰 AppHandle，所以可以直接单测。
//!
//! ## 不变式（改这个文件前先读这三条）
//!
//! 1. **同步永不改写已有条目的 `name`** —— 用户的重命名必须在同步中存活。
//!    不要"顺手刷新一下名字"，那会毁掉用户数据。
//! 2. **同步只增删 `source == Desktop` 的条目** —— `Manual` 条目永不受影响。
//! 3. **`path`（归一化后）在整个 `AppConfig` 内全局唯一**，不是分类内唯一。
//!    有了这条，"从桌面拖到学习之后不被拽回"是自然结果，不需要额外的 pinned 状态。
//!
//! ## 全量同步的语义
//!
//! - 桌面新增 → 进「桌面」分类
//! - 桌面删除 → 从**任意**分类移除（用户拖到别处的也算）
//! - 用户拖到别的分类 → 保持在那里（分类归属记在 Category 里，步骤 1 只看 path 存不存在）

use crate::config::{desktop_id, norm_path, AppConfig, Item, ItemSource, DESKTOP_CATEGORY};
use crate::desktop::Scanned;
use std::collections::HashSet;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub added: usize,
    pub removed: usize,
    pub adopted: usize,
}

impl SyncReport {
    pub fn changed(&self) -> bool {
        self.added + self.removed + self.adopted > 0
    }
}

/// 把 `scanned` 描述的桌面状态合并进 `cfg`。
///
/// 调用方负责先做好扫描失败的判断 —— 这个函数拿到的 `scanned`
/// 必须是一份**可信的**桌面快照，否则步骤 1 会误删。
pub fn sync(cfg: &mut AppConfig, scanned: &[Scanned]) -> SyncReport {
    let mut report = SyncReport::default();

    let live: HashSet<String> = scanned.iter().map(|e| norm_path(&e.path)).collect();

    // ── 步骤 1：自动减 ──────────────────────────────────────────
    // 遍历【所有】分类，不只「桌面」。桌面文件没了，无论当初被拖到哪儿都要移除。
    // 谓词里 Manual 的短路在前，所以手动条目永远不会被这一步碰到。
    for cat in cfg.categories.iter_mut() {
        let before = cat.items.len();
        cat.items.retain(|it| {
            it.source != ItemSource::Desktop || live.contains(&norm_path(&it.path))
        });
        report.removed += before - cat.items.len();
    }

    // ── 步骤 2：建立全局"已认领"索引 ────────────────────────────
    let mut claimed: HashSet<String> = HashSet::new();
    for cat in cfg.categories.iter() {
        for it in &cat.items {
            if it.source == ItemSource::Desktop {
                claimed.insert(norm_path(&it.path));
            }
        }
    }

    // ── 步骤 3：认领 ────────────────────────────────────────────
    for e in scanned {
        let key = norm_path(&e.path);
        if claimed.contains(&key) {
            continue;
        }

        // 3a. 收养：同路径的 Manual 条目 → 原地升级。
        // 旧版本里用户手动把桌面文件拖进「学习」，如果不收养，
        // 同步会在「桌面」再放一份，出现重复。
        let mut adopted = false;
        for cat in cfg.categories.iter_mut() {
            if let Some(it) = cat.items.iter_mut().find(|i| norm_path(&i.path) == key) {
                it.source = ItemSource::Desktop;
                it.kind = e.kind; // 文件夹可能变成了文件，刷新类型
                // 刻意【不动 it.name】—— 见不变式 1
                adopted = true;
                break;
            }
        }
        if adopted {
            claimed.insert(key);
            report.adopted += 1;
            continue;
        }

        // 3b. 新增进「桌面」分类
        let Some(di) = cfg.categories.iter().position(|c| c.id == DESKTOP_CATEGORY.0) else {
            continue; // normalize() 保证存在；这里只是不 panic
        };
        cfg.categories[di].items.push(Item {
            id: desktop_id(&e.path),
            name: e.name.clone(),
            path: e.path.clone(),
            kind: e.kind,
            source: ItemSource::Desktop,
        });
        claimed.insert(key);
        report.added += 1;
    }

    // ── 步骤 4：只给「桌面」排序 ────────────────────────────────
    // 人工分类保持用户自己的顺序，不要去动。
    if let Some(d) = cfg.categories.iter_mut().find(|c| c.id == DESKTOP_CATEGORY.0) {
        d.items.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.path.cmp(&b.path))
        });
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Category, ItemKind};

    fn sc(path: &str, name: &str) -> Scanned {
        Scanned {
            path: path.into(),
            name: name.into(),
            kind: ItemKind::File,
        }
    }

    fn item(id: &str, path: &str, name: &str, source: ItemSource) -> Item {
        Item {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            kind: ItemKind::File,
            source,
        }
    }

    fn cat(id: &str, items: Vec<Item>) -> Category {
        Category { id: id.into(), name: id.into(), items }
    }

    /// 核心不变式：从「桌面」拖到「学习」的条目，同步后必须还在「学习」。
    #[test]
    fn moved_item_stays_in_its_category() {
        let mut cfg = AppConfig::default();
        cfg.categories = vec![
            cat(DESKTOP_CATEGORY.0, vec![]),
            cat("study", vec![item("d1", "C:\\D\\a.txt", "a", ItemSource::Desktop)]),
        ];

        let report = sync(&mut cfg, &[sc("C:\\D\\a.txt", "a")]);

        assert_eq!(report.removed, 0);
        assert_eq!(report.added, 0, "已认领的路径不该在「桌面」再放一份");
        assert_eq!(cfg.categories[1].items.len(), 1, "必须留在「学习」");
        assert!(cfg.categories[0].items.is_empty(), "不该被拽回「桌面」");
    }

    /// 桌面删掉的条目，无论当初被拖到哪个分类，都要移除。
    #[test]
    fn deleted_from_desktop_removes_from_any_category() {
        let mut cfg = AppConfig::default();
        cfg.categories = vec![
            cat(DESKTOP_CATEGORY.0, vec![item("d1", "C:\\D\\gone.txt", "gone", ItemSource::Desktop)]),
            cat("study", vec![item("d2", "C:\\D\\also.txt", "also", ItemSource::Desktop)]),
        ];

        let report = sync(&mut cfg, &[]); // 桌面空了

        assert_eq!(report.removed, 2);
        assert!(cfg.categories[0].items.is_empty());
        assert!(cfg.categories[1].items.is_empty(), "别的分类里也要移除");
    }

    /// Manual 条目永不受同步影响 —— 哪怕它的路径不在桌面上。
    #[test]
    fn manual_items_are_never_touched() {
        let mut cfg = AppConfig::default();
        cfg.categories = vec![
            cat(DESKTOP_CATEGORY.0, vec![]),
            cat("study", vec![item("m1", "D:\\笔记\\x.md", "x", ItemSource::Manual)]),
        ];

        let report = sync(&mut cfg, &[]);

        assert_eq!(report.removed, 0);
        assert_eq!(cfg.categories[1].items.len(), 1, "手动条目不能被同步删掉");
    }

    /// 收养：已有的同路径 Manual 条目原地升级，保留原分类和用户改过的名字。
    #[test]
    fn existing_manual_item_is_adopted_in_place() {
        let mut cfg = AppConfig::default();
        cfg.categories = vec![
            cat(DESKTOP_CATEGORY.0, vec![]),
            cat("study", vec![item("m1", "C:\\D\\a.txt", "我改过的名字", ItemSource::Manual)]),
        ];

        let report = sync(&mut cfg, &[sc("C:\\D\\a.txt", "a.txt")]);

        assert_eq!(report.adopted, 1);
        assert_eq!(report.added, 0, "不该产生重复条目");
        assert_eq!(cfg.categories[1].items.len(), 1);
        assert_eq!(cfg.categories[1].items[0].source, ItemSource::Desktop);
        assert_eq!(
            cfg.categories[1].items[0].name, "我改过的名字",
            "收养不得改写用户重命名过的名字"
        );
    }

    /// 新增的桌面文件进「桌面」分类，并带上路径派生的稳定 id。
    #[test]
    fn new_desktop_file_lands_in_desktop_category() {
        let mut cfg = AppConfig::default();
        let report = sync(&mut cfg, &[sc("C:\\D\\new.txt", "new.txt")]);

        assert_eq!(report.added, 1);
        let d = cfg.categories.iter().find(|c| c.id == DESKTOP_CATEGORY.0).unwrap();
        assert_eq!(d.items.len(), 1);
        assert_eq!(d.items[0].id, desktop_id("C:\\D\\new.txt"), "id 应由路径派生");
        assert_eq!(d.items[0].source, ItemSource::Desktop);
    }

    /// 幂等：同样的扫描结果跑两次，第二次不该有任何变化。
    #[test]
    fn sync_is_idempotent() {
        let mut cfg = AppConfig::default();
        let snap = vec![sc("C:\\D\\b.txt", "b.txt"), sc("C:\\D\\a.txt", "a.txt")];

        let first = sync(&mut cfg, &snap);
        assert_eq!(first.added, 2);

        let second = sync(&mut cfg, &snap);
        assert_eq!(second, SyncReport::default(), "第二次不该有任何变化");
    }

    /// 路径大小写/分隔符不同，必须认作同一个条目。
    #[test]
    fn path_matching_is_case_and_separator_insensitive() {
        let mut cfg = AppConfig::default();
        cfg.categories = vec![
            cat(DESKTOP_CATEGORY.0, vec![]),
            cat("study", vec![item("d1", "C:\\D\\A.txt", "A", ItemSource::Desktop)]),
        ];

        let report = sync(&mut cfg, &[sc("c:/d/a.txt", "A")]);

        assert_eq!(report.removed, 0, "不该因为写法不同就误删");
        assert_eq!(report.added, 0, "也不该重复添加");
    }

    /// 「桌面」分类按名字排序，人工分类保持用户顺序。
    #[test]
    fn only_desktop_category_is_sorted() {
        let mut cfg = AppConfig::default();
        cfg.categories = vec![
            cat(DESKTOP_CATEGORY.0, vec![]),
            cat(
                "study",
                vec![
                    item("m1", "D:\\z", "z", ItemSource::Manual),
                    item("m2", "D:\\a", "a", ItemSource::Manual),
                ],
            ),
        ];

        sync(&mut cfg, &[sc("C:\\D\\z.txt", "z.txt"), sc("C:\\D\\a.txt", "a.txt")]);

        let d = &cfg.categories[0];
        assert_eq!(d.items[0].name, "a.txt", "桌面分类应按名字排序");
        let s = &cfg.categories[1];
        assert_eq!(s.items[0].name, "z", "人工分类不能被动排序");
        assert_eq!(s.items[1].name, "a");
    }
}

import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import type { AppConfig, Category, Item } from "./types";
import ItemGrid, { Glyph } from "./components/ItemGrid";
import RecentList from "./components/RecentList";

const COLLAPSE_DELAY = 700;
/** 超过这个位移才算拖拽，否则算点击。Windows 自己的 SM_CXDRAG 就是 4。 */
const DRAG_THRESHOLD = 4;

/** 侧边栏里人工分类的展示顺序。「桌面」和收藏夹另行处理。 */
const NAV_ORDER = ["study", "project", "mad", "fun"] as const;
const DESKTOP_ID = "desktop";
/** 内置分类。**不在这个集合里的就是用户自建的收藏夹。** */
const BUILTIN = new Set<string>([...NAV_ORDER, DESKTOP_ID]);

// ── 进出动画 ────────────────────────────────────────────────────
/**
 * 动画起点时面板整体下移的距离（逻辑像素）。
 * 40px 足够看出"从屏幕底边升上来"，又不至于让面板飞一大截。
 */
const SLIDE = 40;
/** 展开 / 收起时长。Windows 自己的浮出面板也落在这一档。 */
const ENTER_MS = 200;
const EXIT_MS = 180;

/** 内联的名字输入框（新建 / 重命名收藏夹共用）。
 *
 * `done` 这个闸门是必须的：回车提交之后输入框会失焦，blur 会**再提交一次**；
 * Esc 取消之后失焦又会把草稿提交上去。先落闸，两个入口就都只生效一次。 */
function NameEditor({
  initial,
  placeholder,
  onCommit,
  onCancel,
}: {
  initial: string;
  placeholder: string;
  onCommit: (name: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  const done = useRef(false);

  const finish = (commit: boolean) => {
    if (done.current) return;
    done.current = true;
    const name = value.trim();
    if (commit && name) onCommit(name);
    else onCancel();
  };

  return (
    <input
      className="nav-edit"
      autoFocus
      placeholder={placeholder}
      value={value}
      onChange={(e) => setValue(e.target.value)}
      // 输入框在导航里，不挡住这些事件的话会触发页内拖拽
      onPointerDown={(e) => e.stopPropagation()}
      onBlur={() => finish(true)}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(true);
        if (e.key === "Escape") finish(false);
      }}
    />
  );
}

export default function App() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [tab, setTab] = useState(DESKTOP_ID);
  const [expanded, setExpanded] = useState(true);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState("");
  // 同步结果之类的普通提示，短暂显示后自动消失
  const [notice, setNotice] = useState("");
  // 配置损坏是"需要用户处置"的状态，用常驻横幅而不是会自动消失的 toast
  const [fatal, setFatal] = useState("");
  const [busy, setBusy] = useState(false);
  // path → 原生图标 data URL（null 表示取失败，退回几何图形）
  const [icons, setIcons] = useState<Map<string, string | null>>(new Map());

  // ── 收藏夹的三种内联编辑状态 ──────────────────────────────
  // 只允许同时存在一种，所以用一个联合值就够了
  const [editing, setEditing] = useState<
    { mode: "create" } | { mode: "rename"; id: string } | { mode: "confirm"; id: string } | null
  >(null);

  // ── 面板进出动画 ──────────────────────────────────────────
  const dockRef = useRef<HTMLDivElement | null>(null);
  const rafRef = useRef(0);
  /** 当前位移。收起态 = SLIDE（不可见），展开态 = 0。 */
  const dyRef = useRef(0);

  // ── 页内拖拽状态 ──────────────────────────────────────────
  const [dragItem, setDragItem] = useState<Item | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  // 真相必须在 ref 里：pointermove 触发的 re-render 和 pointerup 可能在同一帧，
  // pointerup 里读 state 会拿到上一帧的落点 → "快速松手时分类落错"。
  const dropTargetRef = useRef<string | null>(null);
  const dragRef = useRef<{
    item: Item;
    from: string;
    startX: number;
    startY: number;
    active: boolean;
  } | null>(null);
  const tabRectsRef = useRef<{ id: string; rect: DOMRect }[]>([]);
  const ghostRef = useRef<HTMLDivElement | null>(null);
  const ghostPos = useRef({ x: 0, y: 0 });
  const ghostRaf = useRef(0);
  // 拖拽结束后要吞掉紧随其后的 click，否则松手会把卡片"打开"
  const suppressClick = useRef(false);
  // 已经请求过图标的路径（含失败的），避免每次切分类都重复 IPC
  const iconRequested = useRef<Set<string>>(new Set());

  const timer = useRef<number | null>(null);
  const tabRef = useRef(tab);
  tabRef.current = tab;
  // 事件回调里要读最新值，但又不想因此重绑监听
  const autoHideRef = useRef(true);
  // 展开状态的真实来源。副作用绝不能写在 setState 的 updater 里——
  // React 要求 updater 是纯函数，可能延迟甚至跳过调用它。
  const expandedRef = useRef(true);

  const apply = useCallback((c: AppConfig) => {
    autoHideRef.current = c.settings.autoHide;
    setConfig(c);
  }, []);

  // 后端进入只读模式后，所有写操作都会带这句话，把它提升为常驻横幅
  const fail = useCallback((e: unknown) => {
    const msg = String(e);
    if (msg.includes("只读模式")) setFatal(msg);
    else setError(msg);
  }, []);

  const run = useCallback((p: Promise<AppConfig>) => p.then(apply).catch(fail), [apply, fail]);

  useEffect(() => {
    api
      .loadConfig()
      .then((c) => {
        apply(c);
        // 配置就绪后再做图标对账：崩溃恢复 + Explorer 重启后重新施加隐藏
        return api.reconcileDesktopIcons();
      })
      .then(apply)
      .catch(fail);
  }, [apply, fail]);

  // 只给当前显示的条目取图标。全量预取 90 张是没必要的 IPC 和内存开销。
  useEffect(() => {
    if (!config) return;
    const items = config.categories.find((c) => c.id === tab)?.items ?? [];
    const need = items.map((i) => i.path).filter((p) => !iconRequested.current.has(p));
    if (need.length === 0) return;

    need.forEach((p) => iconRequested.current.add(p));
    api
      .getIcons(need)
      .then((entries) => {
        setIcons((prev) => {
          const next = new Map(prev);
          for (const e of entries) next.set(e.path, e.data);
          return next;
        });
      })
      .catch(() => {
        // 取图标失败不该打断界面；撤回标记以便下次重试
        need.forEach((p) => iconRequested.current.delete(p));
      });
  }, [config, tab]);

  // 量出三个模块的实际位置交给 Rust 做窗口区域裁剪。
  // 缝隙处窗口会被裁掉 → 那里直接露出**未被模糊的**桌面。
  //
  // 用 DOM 实测而不是在 Rust 里复刻一份 CSS 布局知识 —— 布局改了这里自动跟上。
  useEffect(() => {
    if (!expanded) return;
    const push = () => {
      const dpr = window.devicePixelRatio || 1;
      // 量之前先把动画位移摘掉：getBoundingClientRect 会把 transform 算进去，
      // 而我们要交给 Rust 的是**不含位移的基准位置**（位移另外用 setPanelOffset 叠加）。
      // 改完同一个 tick 内就恢复，合成器看不到这一下。
      const root = dockRef.current;
      const saved = root?.style.transform ?? "";
      if (root) root.style.transform = "none";

      const rects = Array.from(document.querySelectorAll<HTMLElement>(".module")).map((el) => {
        const r = el.getBoundingClientRect();
        // border-radius 的 CSS 值 ×2 才是 CreateRoundRectRgn 要的椭圆直径
        const br = parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0;
        return {
          x: Math.round(r.left * dpr),
          y: Math.round(r.top * dpr),
          w: Math.round(r.width * dpr),
          h: Math.round(r.height * dpr),
          radius: Math.round(2 * br * dpr),
        };
      });

      if (root) root.style.transform = saved;
      if (rects.length) void api.setPanelRegions(rects).catch(() => {});
    };
    // 等窗口 resize 和布局都稳定下来再量
    const t = window.setTimeout(push, 40);
    window.addEventListener("resize", push);
    return () => {
      window.clearTimeout(t);
      window.removeEventListener("resize", push);
    };
    // config 必须在依赖里：挂载时它还是 null，页面上没有 .module 元素，
    // 量出来是空数组、命令不会发。等 config 到位重渲染时若不再触发，
    // 区域裁剪和毛玻璃就永远不会被施加。
  }, [expanded, tab, config]);

  // ── 位移 → 视觉 ─────────────────────────────────────────────
  // 位移走 DOM 的 transform（合成器就能做，不触发重排），透明度走**窗口本身**：
  // CSS 的 opacity 管不到窗口上那层 DWM 玻璃，那层玻璃会一直留到窗口隐藏，
  // 收起末尾就是用户报的"一块黑"。整窗 alpha 才能让玻璃和内容一起淡。
  //
  // DOM 自己的 opacity 退化成一道"要么全遮要么全显"的闸门，只在**完全收起**那一档
  // 落下来。理由：窗口 `show()` 之后约 15 ms 扩展样式才补回来，那期间整窗是不透明的
  // （实测 `layered=0`），DOM 若已经可见就会闪一帧满亮的面板。动画过程一律不遮，
  // 淡出完全交给整窗 alpha，两者不会各走一套曲线。
  const applyVisual = useCallback((dy: number, animating: boolean) => {
    const el = dockRef.current;
    if (el) {
      el.style.willChange = animating ? "transform" : "";
      el.style.transform = dy === 0 ? "" : `translate3d(0, ${dy}px, 0)`;
      el.style.opacity = dy >= SLIDE - 0.5 ? "0" : "1";
    }
    // 原生裁剪区域必须跟 CSS 的位移同帧更新，否则滑动中的面板会被旧的区域切边 ——
    // 那种"被裁掉一条"的违和感，正是以前像网页元素闪现的原因之一。
    const alpha = Math.round(255 * Math.max(0, 1 - dy / SLIDE));
    void api.setPanelOffset(dy, alpha).catch(() => {});
  }, []);

  const tween = useCallback(
    (to: number, ms: number, done?: () => void) => {
      cancelAnimationFrame(rafRef.current);
      const from = dyRef.current;
      if (from === to) {
        applyVisual(to, false);
        done?.();
        return;
      }
      const t0 = performance.now();
      const step = (now: number) => {
        // 夹在 [0,1]：rAF 回调拿到的是**这一帧的开始时刻**，可能早于我们记的 t0。
        // 不夹的话 p 会是负数，缓动曲线把面板推到起点之外（实测多走了 2.7px），
        // 表现成动画第一下先"弹"一下。
        const p = Math.min(1, Math.max(0, (now - t0) / ms));
        // ease-out：起步快、收尾稳，和 Windows 浮出面板同一手感
        const dy = from + (to - from) * (1 - Math.pow(1 - p, 3));
        dyRef.current = dy;
        const running = p < 1;
        applyVisual(dy, running);
        if (running) {
          rafRef.current = requestAnimationFrame(step);
        } else {
          dyRef.current = to;
          applyVisual(to, false);
          done?.();
        }
      };
      rafRef.current = requestAnimationFrame(step);
    },
    [applyVisual],
  );

  const expand = useCallback(() => {
    // 拖拽期间绝不 resize/移动窗口 —— 窗口在光标下动会让缓存 rect 失效、坐标漂移
    if (dragRef.current?.active) return;
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
    if (expandedRef.current) return;
    expandedRef.current = true;
    setExpanded(true);

    // 顺序要紧：先把原生裁剪摆到动画起点，再显示窗口。
    // 反过来窗口会先以"最终位置、接近全亮"的样子露出一帧 —— 那就是闪。
    // DOM 也先落到"完全收起"那一档（见 applyVisual），把 show() 之后那段样式
    // 还没补回来的空窗盖住。
    if (dockRef.current) dockRef.current.style.opacity = "0";
    void api
      .setPanelOffset(SLIDE, 0)
      .then(() => api.setDockExpanded(true))
      .then(() => tween(0, ENTER_MS))
      .catch(fail);
  }, [fail, tween]);

  const collapse = useCallback(() => {
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = null;
    if (!expandedRef.current) return;
    expandedRef.current = false;
    setExpanded(false);
    // 动画跑完才隐藏窗口；收起时窗口不动尺寸、不清区域，
    // 所以下次展开的几何和区域都还是对的
    tween(SLIDE, EXIT_MS, () => void api.setDockExpanded(false).catch(fail));
  }, [fail, tween]);

  const toggle = useCallback(() => {
    if (expandedRef.current) collapse();
    else expand();
  }, [collapse, expand]);

  // 收起时窗口被整个隐藏，收不到任何鼠标事件。
  // Rust 侧轮询到光标贴住屏幕底边时发这个事件，我们负责把它展开。
  useEffect(() => {
    const un = listen("dock-wake", () => expand());
    return () => {
      void un.then((f) => f());
    };
  }, [expand]);

  // 全局快捷键 Ctrl+Alt+Q：面板关着也能唤出，已展开则收起
  useEffect(() => {
    const un = listen("hotkey-toggle", () => toggle());
    return () => {
      void un.then((f) => f());
    };
  }, [toggle]);

  const scheduleCollapse = useCallback(() => {
    if (!autoHideRef.current) return;
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      timer.current = null;
      collapse();
    }, COLLAPSE_DELAY);
  }, [collapse]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // 拖拽中按 Esc 取消拖拽，而不是收起面板
      if (dragRef.current) {
        dragRef.current = null;
        setDragItem(null);
        setDropTarget(null);
        dropTargetRef.current = null;
        document.body.style.cursor = "";
        return;
      }
      collapse();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [collapse]);

  // 从资源管理器拖文件/文件夹进来。dragDropEnabled 已在 tauri.conf.json 打开，
  // 所以走 Tauri 原生事件而不是 HTML5 的 drop。
  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((e) => {
      const p = e.payload;
      if (p.type === "enter" || p.type === "over") {
        setDragging(true);
        expand();
      } else if (p.type === "drop") {
        setDragging(false);
        run(api.addPaths(tabRef.current, p.paths));
      } else {
        setDragging(false);
      }
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [expand, run]);

  useEffect(() => {
    if (!error) return;
    const t = window.setTimeout(() => setError(""), 4000);
    return () => window.clearTimeout(t);
  }, [error]);

  useEffect(() => {
    if (!notice) return;
    const t = window.setTimeout(() => setNotice(""), 3000);
    return () => window.clearTimeout(t);
  }, [notice]);

  // 手动刷新桌面。反馈里报出本次增删了几条，否则用户无从判断同步有没有生效。
  const refresh = () => {
    if (busy || !config) return;
    const count = (c: AppConfig) =>
      c.categories.flatMap((x) => x.items).filter((i) => i.source === "desktop").length;
    const before = count(config);

    setBusy(true);
    api
      .refreshDesktop()
      .then((next) => {
        apply(next);
        const delta = count(next) - before;
        setNotice(
          delta === 0 ? "桌面没有变化" : delta > 0 ? `新增 ${delta} 项` : `移除 ${-delta} 项`,
        );
      })
      .catch(fail)
      .finally(() => setBusy(false));
  };

  // ── 收藏夹 ──────────────────────────────────────────────────
  // 三个操作都只改配置里的映射关系，一个文件都不会被移动、复制或删除。

  const createFavorite = (name: string) => {
    setEditing(null);
    api
      .createCategory(name)
      .then((c) => {
        apply(c);
        // 建完直接跳过去：新收藏夹是空的，正好接着往里拖东西
        const made = c.categories.find((x) => x.name === name);
        if (made) setTab(made.id);
      })
      .catch(fail);
  };

  const renameFavorite = (id: string, name: string) => {
    setEditing(null);
    void run(api.renameCategory(id, name));
  };

  const deleteFavorite = (id: string) => {
    setEditing(null);
    // 正在看的就是它 → 退回「桌面」，否则内容区会空成一片
    if (tabRef.current === id) setTab(DESKTOP_ID);
    void run(api.deleteCategory(id));
  };

  // ── 页内拖拽实现 ────────────────────────────────────────────
  // dragDropEnabled: true 会禁用 WebView 的 HTML5 拖放 API，而"从资源管理器拖入"
  // 必须保留，二者互斥。所以这里用 Pointer Events 手写一套，两者不冲突。

  const setDrop = useCallback((id: string | null) => {
    dropTargetRef.current = id;
    setDropTarget(id);
  }, []);

  const measureTabs = useCallback(() => {
    tabRectsRef.current = Array.from(
      document.querySelectorAll<HTMLElement>("[data-cat]"),
    ).map((el) => ({ id: el.dataset.cat!, rect: el.getBoundingClientRect() }));
  }, []);

  // 用缓存的 rect 而不是 elementFromPoint：更快、不触发布局，
  // 而且天然免疫"ghost 挡住了命中目标"。
  const hitTest = useCallback((x: number, y: number): string | null => {
    const hit = tabRectsRef.current.find(
      ({ rect }) => x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom,
    );
    if (!hit) return null;
    if (hit.id === dragRef.current?.from) return null; // 拖回原分类 = 无操作
    return hit.id;
  }, []);

  const moveGhost = useCallback((x: number, y: number) => {
    ghostPos.current = { x, y };
    if (ghostRaf.current) return;
    ghostRaf.current = requestAnimationFrame(() => {
      ghostRaf.current = 0;
      const el = ghostRef.current;
      if (el) {
        el.style.transform = `translate3d(${ghostPos.current.x + 14}px, ${
          ghostPos.current.y + 14
        }px, 0)`;
      }
    });
  }, []);

  const endDrag = useCallback(
    (commitTo: string | null) => {
      if (ghostRaf.current) {
        cancelAnimationFrame(ghostRaf.current);
        ghostRaf.current = 0;
      }
      const d = dragRef.current;
      dragRef.current = null;
      setDragItem(null);
      setDrop(null);
      document.body.style.cursor = "";

      if (d?.active) {
        // 让 suppressClick 活过紧随其后的 click
        suppressClick.current = true;
        window.setTimeout(() => {
          suppressClick.current = false;
        }, 0);
        if (commitTo) run(api.moveItem(d.item.id, commitTo));
      }
    },
    [run, setDrop],
  );

  const onDragMove = useCallback(
    (e: PointerEvent) => {
      const d = dragRef.current;
      if (!d) return;

      if (!d.active) {
        const dx = e.clientX - d.startX;
        const dy = e.clientY - d.startY;
        if (dx * dx + dy * dy < DRAG_THRESHOLD * DRAG_THRESHOLD) return;
        d.active = true;
        measureTabs(); // 面板尺寸固定、导航不滚动，量一次就够
        setDragItem(d.item);
        document.body.style.cursor = "grabbing";
      }

      moveGhost(e.clientX, e.clientY);
      setDrop(hitTest(e.clientX, e.clientY));
    },
    [hitTest, measureTabs, moveGhost, setDrop],
  );

  const onDragUp = useCallback(() => endDrag(dropTargetRef.current), [endDrag]);
  const onDragCancel = useCallback(() => endDrag(null), [endDrag]);

  const onItemPointerDown = (e: React.PointerEvent, item: Item) => {
    if (e.button !== 0 || !e.isPrimary) return;
    // 必须排除这些，否则重命名输入框点不进去、✎/✕ 按钮全失效
    if ((e.target as HTMLElement).closest("input, .acts")) return;

    // 指针捕获：拖出卡片后事件仍会冒泡到 window，监听才收得到
    try {
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    } catch {
      /* 捕获失败不影响 window 监听 */
    }

    dragRef.current = {
      item,
      from: tabRef.current,
      startX: e.clientX,
      startY: e.clientY,
      active: false,
    };
  };

  // 监听常驻，由 dragRef 决定要不要处理 —— 避免 add/remove 的函数身份问题
  useEffect(() => {
    window.addEventListener("pointermove", onDragMove);
    window.addEventListener("pointerup", onDragUp);
    window.addEventListener("pointercancel", onDragCancel);
    return () => {
      window.removeEventListener("pointermove", onDragMove);
      window.removeEventListener("pointerup", onDragUp);
      window.removeEventListener("pointercancel", onDragCancel);
    };
  }, [onDragMove, onDragUp, onDragCancel]);

  const openPath = (path: string) => {
    // 刚拖完的那一下不要当成点击打开
    if (suppressClick.current) {
      suppressClick.current = false;
      return;
    }
    api
      .openItem(path)
      .then((c) => {
        apply(c);
        // 让位给刚启动的程序
        window.setTimeout(() => {
          if (autoHideRef.current) collapse();
        }, 220);
      })
      .catch(fail);
  };

  // 配置损坏：绝不静默重置，把决定权交给用户
  if (fatal) {
    return (
      <div className="dock" ref={dockRef}>
        <div className="fatal">
          <p className="fatal-title">配置文件有问题，已进入只读模式</p>
          <p className="fatal-msg">{fatal}</p>
          <p className="fatal-hint">
            改动不会被保存，你的数据仍原样在磁盘上。修复后点「重新加载」。
          </p>
          <div className="fatal-actions">
            <button className="chip" onClick={() => void api.openConfigFolder()}>
              打开配置目录
            </button>
            <button className="chip" onClick={() => window.location.reload()}>
              重新加载
            </button>
          </div>
        </div>
      </div>
    );
  }

  // 配置加载完成前也必须能响应悬停——否则若鼠标已经在底部，
  // 加载完成后不会补发 mouseenter，dock 就再也打不开了。
  if (!config) {
    return <div className="dock is-collapsed" ref={dockRef} onMouseEnter={expand} />;
  }

  // 导航顺序按需求定：四个人工分类 → ⭐收藏夹 → 桌面。
  // 「最近访问」已独立成模块，不再占导航位。
  const navCats = NAV_ORDER.map((id) => config.categories.find((c) => c.id === id)).filter(
    (c): c is Category => !!c,
  );
  const desktopCat = config.categories.find((c) => c.id === DESKTOP_ID);
  const favorites = config.categories.filter((c) => !BUILTIN.has(c.id));
  const activeCat = config.categories.find((c) => c.id === tab);
  const showGrid = !!activeCat;
  const quickRecent = config.recent.slice(0, 12);

  /** 一个可拖入的分类按钮。内置分类和收藏夹长得一样，只是后者多了两个小按钮。 */
  const catButton = (c: Category) => (
    <button
      key={c.id}
      className="nav-item"
      data-on={tab === c.id}
      data-cat={c.id}
      data-drop={dropTarget === c.id ? "true" : undefined}
      onClick={() => {
        setTab(c.id);
        expand();
      }}
    >
      <span className="nav-name">{c.name}</span>
      {c.items.length > 0 && <span className="nav-count">{c.items.length}</span>}
    </button>
  );

  return (
    <div
      className={`dock${dragging || dragItem ? " is-dragging" : ""}`}
      ref={dockRef}
      onMouseEnter={expand}
      onMouseLeave={() => {
        // 两种拖拽都要挡住：dragging 是资源管理器拖入，dragItem 是页内拖拽
        if (!dragging && !dragItem) scheduleCollapse();
      }}
    >
      {/* ── 模块一：分类导航 ───────────────────────────── */}
      <aside className="module module--nav">
        <div className="brand">桌面收纳</div>

        <nav className="nav">
          {navCats.map(catButton)}

          <div className="nav-sep" />

          {/* ⭐ 收藏夹：数量不限，新建 / 重命名 / 删除都在这一小块里完成 */}
          <div className="nav-group">
            <span className="nav-group-title">⭐ 收藏夹</span>
            <button
              className="nav-add"
              title="新建收藏夹"
              onClick={() => setEditing({ mode: "create" })}
            >
              ＋
            </button>
          </div>

          {favorites.map((c) =>
            editing?.mode === "rename" && editing.id === c.id ? (
              <NameEditor
                key={c.id}
                initial={c.name}
                placeholder="收藏夹名字"
                onCommit={(name) => renameFavorite(c.id, name)}
                onCancel={() => setEditing(null)}
              />
            ) : editing?.mode === "confirm" && editing.id === c.id ? (
              <div className="nav-confirm" key={c.id}>
                <span className="nav-confirm-text">删除「{c.name}」？</span>
                {/* 这个程序最怕被误解成"会动我的文件"，所以这一句必须在场 */}
                <span className="nav-confirm-note">原文件不会被删除</span>
                <button className="ok" onClick={() => deleteFavorite(c.id)}>
                  删除
                </button>
                <button onClick={() => setEditing(null)}>取消</button>
              </div>
            ) : (
              <div className="fav" key={c.id}>
                {catButton(c)}
                <span className="fav-acts">
                  <button
                    title="重命名"
                    onClick={() => setEditing({ mode: "rename", id: c.id })}
                  >
                    ✎
                  </button>
                  <button
                    className="rm"
                    title="删除收藏夹"
                    onClick={() => setEditing({ mode: "confirm", id: c.id })}
                  >
                    ✕
                  </button>
                </span>
              </div>
            ),
          )}

          {editing?.mode === "create" && (
            <NameEditor
              initial=""
              placeholder="收藏夹名字"
              onCommit={createFavorite}
              onCancel={() => setEditing(null)}
            />
          )}

          {favorites.length === 0 && editing?.mode !== "create" && (
            <p className="nav-hint">点 ＋ 新建一个</p>
          )}

          {desktopCat && (
            <>
              <div className="nav-sep" />
              {catButton(desktopCat)}
            </>
          )}
        </nav>

        <div className="side-foot">
          <button
            className="chip"
            data-on={config.settings.autoHide}
            title="鼠标离开后自动收起"
            onClick={() => run(api.setSettings(!config.settings.autoHide, config.settings.autostart))}
          >
            自动隐藏
          </button>
          <button
            className="chip"
            data-on={config.settings.autostart}
            title="开机时自动启动"
            onClick={() => run(api.setSettings(config.settings.autoHide, !config.settings.autostart))}
          >
            开机启动
          </button>
          <button
            className="chip"
            data-on={config.settings.hideDesktopIcons}
            title="隐藏桌面图标。系统级设置，退出程序时会自动恢复"
            onClick={() => run(api.setHideDesktopIcons(!config.settings.hideDesktopIcons))}
          >
            隐藏图标
          </button>

          <div className="side-icons">
            <button className="icon" title="重新扫描桌面" disabled={busy} onClick={refresh}>
              {busy ? "…" : "⟳"}
            </button>
            <button
              className="icon"
              title="打开配置文件所在目录"
              onClick={() => void api.openConfigFolder()}
            >
              ⋯
            </button>
            {/* 跟光标贴底边的唤醒是一回事，写出来用户才知道有这条路 */}
            <button className="icon" title="收起（Esc / Ctrl+Alt+Q）" onClick={collapse}>
              ⌄
            </button>
            <button
              className="icon icon--quit"
              title="退出（会自动恢复桌面图标）"
              onClick={() => void api.quitApp()}
            >
              <svg
                width="13"
                height="13"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.4"
                strokeLinecap="round"
                aria-hidden="true"
              >
                <path d="M12 3v9" />
                <path d="M6.3 6.3a9 9 0 1 0 11.4 0" />
              </svg>
            </button>
          </div>
        </div>
      </aside>

      {/* ── 模块二：内容区 ─────────────────────────────── */}
      <main className="module module--content">
        <header className="main-head">
          <h1 className="main-title">{activeCat?.name ?? ""}</h1>
          {showGrid && <span className="main-sub">{activeCat.items.length} 项</span>}
        </header>

        {/* key={tab} 让切换分类时重挂载，从而重放进场动画 */}
        <div className="body" key={tab}>
          <ItemGrid
            items={activeCat?.items ?? []}
            draggingId={dragItem?.id ?? null}
            icons={icons}
            onOpen={(it) => openPath(it.path)}
            onRemove={(it) => activeCat && run(api.removeItem(activeCat.id, it.id))}
            onRename={(it, name) => activeCat && run(api.renameItem(activeCat.id, it.id, name))}
            onItemPointerDown={onItemPointerDown}
          />
        </div>
      </main>

      {/* ── 模块三：最近访问 ───────────────────────────── */}
      <aside className="module module--recent">
        <div className="recent-head">
          <span>最近访问</span>
          {config.recent.length > 0 && (
            <button className="chip" onClick={() => run(api.clearRecent())}>
              清空
            </button>
          )}
        </div>
        <div className="recent-body">
          <RecentList recent={quickRecent} onOpen={openPath} />
        </div>
      </aside>

      {dragItem && (
        <div className="ghost" ref={ghostRef}>
          <span className="tile" data-kind={dragItem.kind}>
            {icons.get(dragItem.path) ? (
              <img className="native-icon" src={icons.get(dragItem.path)!} alt="" />
            ) : (
              <Glyph item={dragItem} />
            )}
          </span>
          <span className="ghost-name">{dragItem.name}</span>
        </div>
      )}

      {dragging && <div className="drop" />}
      {error && <div className="toast">{error}</div>}
      {notice && !error && <div className="toast toast--info">{notice}</div>}
    </div>
  );
}

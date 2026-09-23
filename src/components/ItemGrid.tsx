import { useState } from "react";
import type { Item } from "../types";

/// 取不到原生图标时的兜底：文件夹/程序画几何图形，文档显示扩展名。
/// 不做成"统一的通用图标"——扩展名本身是有用信息。
export function Glyph({ item }: { item: Item }) {
  if (item.kind === "folder") {
    return (
      <svg width="20" height="16" viewBox="0 0 20 16" fill="none" aria-hidden="true">
        <path
          d="M1 2.6A1.6 1.6 0 0 1 2.6 1h4.1l1.7 2.1h9A1.6 1.6 0 0 1 19 4.7v8.7A1.6 1.6 0 0 1 17.4 15H2.6A1.6 1.6 0 0 1 1 13.4V2.6Z"
          stroke="currentColor"
          strokeWidth="1.4"
          strokeLinejoin="round"
        />
      </svg>
    );
  }
  if (item.kind === "app") {
    return (
      <svg width="13" height="15" viewBox="0 0 13 15" aria-hidden="true">
        <path
          d="M1.6 1.8c0-.8.9-1.3 1.5-.9l8.3 5.7c.6.4.6 1.3 0 1.7l-8.3 5.7c-.6.4-1.5-.1-1.5-.9V1.8Z"
          fill="currentColor"
        />
      </svg>
    );
  }
  const base = item.path.split(/[\\/]/).pop() ?? "";
  const dot = base.lastIndexOf(".");
  const ext = dot > 0 ? base.slice(dot + 1).toUpperCase().slice(0, 4) : "";
  return <span className="ext">{ext || "—"}</span>;
}

/// 路径提示：只显示父目录，省掉盘符以下的冗长前缀。
/// 完整路径仍在 title 里，也仍是点击时打开的目标。
function parentHint(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (parts.length < 2) return path;
  const parent = parts[parts.length - 2];
  const root = parts[0].endsWith(":") ? parts[0] : "";
  return root && parent !== root ? `${root}\\…\\${parent}` : parent;
}

interface Props {
  items: Item[];
  /** 正在被拖拽的条目 id，用于把源卡片变暗 */
  draggingId: string | null;
  /** path → `data:image/png;base64,...`；值为 null 表示取图标失败，退回几何图形 */
  icons: Map<string, string | null>;
  onOpen: (item: Item) => void;
  onRemove: (item: Item) => void;
  onRename: (item: Item, name: string) => void;
  /** 交给 App 的拖拽状态机。ItemGrid 自己不管拖拽逻辑。 */
  onItemPointerDown: (e: React.PointerEvent, item: Item) => void;
}

export default function ItemGrid({
  items,
  draggingId,
  icons,
  onOpen,
  onRemove,
  onRename,
  onItemPointerDown,
}: Props) {
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  if (items.length === 0) {
    return (
      <div className="empty">
        <span className="empty-code">00 / EMPTY DIRECTORY</span>
        <p className="big">把文件夹或程序拖进来</p>
        <p className="sub">松手就添加，一次可以拖多个</p>
      </div>
    );
  }

  const commit = (item: Item) => {
    setEditing(null);
    if (draft.trim() && draft !== item.name) onRename(item, draft);
  };

  return (
    <div className="grid">
      {items.map((item, index) => {
        const icon = icons.get(item.path);
        return (
          <div
            key={item.id}
            className="card"
            role="button"
            tabIndex={0}
            title={item.path}
            data-drag-src={draggingId === item.id ? "true" : undefined}
            onPointerDown={(e) => onItemPointerDown(e, item)}
            onClick={() => editing !== item.id && onOpen(item)}
            onKeyDown={(e) => {
              if (editing === item.id) return;
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onOpen(item);
              }
            }}
          >
            <span className="card-meta"><span>{String(index + 1).padStart(2, "0")}</span><span>{item.kind === "folder" ? "DIR" : item.kind === "app" ? "APP" : "FILE"}</span></span>
            <span className="acts">
              <button
                title="重命名"
                onClick={(e) => {
                  e.stopPropagation();
                  setDraft(item.name);
                  setEditing(item.id);
                }}
              >
                ✎
              </button>
              <button
                className="rm"
                title="移除"
                onClick={(e) => {
                  e.stopPropagation();
                  onRemove(item);
                }}
              >
                ✕
              </button>
            </span>

            <span className="tile" data-kind={item.kind}>
              {icon ? (
                <img className="native-icon" src={icon} alt="" draggable={false} />
              ) : (
                <Glyph item={item} />
              )}
            </span>

            {editing === item.id ? (
              <input
                className="rename"
                autoFocus
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onClick={(e) => e.stopPropagation()}
                onPointerDown={(e) => e.stopPropagation()}
                onBlur={() => commit(item)}
                onKeyDown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Enter") commit(item);
                  if (e.key === "Escape") setEditing(null);
                }}
              />
            ) : (
              <>
                <span className="name">{item.name}</span>
                <span className="hint">{parentHint(item.path)}</span>
              </>
            )}
          </div>
        );
      })}
    </div>
  );
}

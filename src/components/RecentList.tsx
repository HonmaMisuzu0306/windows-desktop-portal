import type { Recent } from "../types";

function ago(sec: number) {
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - sec);
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  if (diff < 86400 * 30) return `${Math.floor(diff / 86400)} 天前`;
  return new Date(sec * 1000).toLocaleDateString("zh-CN");
}

/// 路径提示：只留末尾两级，避免窄栏里被截得看不清
function shortPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 2) return path;
  return "…\\" + parts.slice(-2).join("\\");
}

interface Props {
  recent: Recent[];
  onOpen: (path: string) => void;
}

export default function RecentList({ recent, onOpen }: Props) {
  if (recent.length === 0) {
    return (
      <div className="empty">
        <span className="empty-code">00 / NO ACTIVITY</span>
        <p className="big">还没有打开过东西</p>
        <p className="sub">打开过的会记在这里</p>
      </div>
    );
  }

  return (
    <div className="recent">
      {recent.map((r, index) => (
        <button key={r.path} className="row" title={r.path} onClick={() => onOpen(r.path)}>
          <span className="row-index">{String(index + 1).padStart(2, "0")}</span>
          <span className="rn">{r.name}</span>
          <span className="rp">{shortPath(r.path)}</span>
          <span className="rt">{ago(r.at)}</span>
        </button>
      ))}
    </div>
  );
}

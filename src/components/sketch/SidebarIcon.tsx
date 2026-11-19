import type { CSSProperties, ReactNode } from "react";

/**
 * 侧栏的手绘小图标 (32×32 画板), 与官网「功能特性」涂鸦同一套笔法:
 * 背后一团错位的柔和色块 + 叠了抖动滤镜的墨线。纯装饰, 读屏跳过, 意思由旁边的文字承担。
 * 虚拟模型 / 用量小票 / 退出登录三枚直接沿用官网的叠放标签、锯齿小票、拱门。
 * 滤镜 #ccr-rough-icon 由根节点的 <SketchDefs /> 提供。
 */
export type SidebarIconName =
  | "guide" | "live" | "agent" | "vm" | "subs" | "logs" | "stats"
  | "receipts" | "updates" | "backup" | "settings" | "about" | "logout" | "whatsnew"
  // 侧栏底部的两枚图案按钮: 明暗模式三态 + 当前主题
  | "mode-system" | "mode-light" | "mode-dark" | "theme";

interface Props {
  name: SidebarIconName;
  /** 渲染尺寸 (px) */
  size?: number;
  /** 背后的色块; 关掉只剩墨线 */
  blob?: boolean;
  className?: string;
}

// 色块轮廓略有不同, 避免一列排下来像盖章; 相邻两项颜色不重复
const BLOB = {
  a: "M4.4 13.8 C 5.1 5.8, 20.4 2.9, 26.9 8.7 C 32 14.5, 28.4 25.5, 20.4 27.6 C 11.6 29.1, 3.6 21.8, 4.4 13.8 Z",
  b: "M5.8 14.5 C 5.1 6.5, 18.2 2.9, 25.5 7.3 C 31.3 11.6, 29.8 24, 21.8 26.9 C 13.8 29.8, 6.5 22.5, 5.8 14.5 Z",
  c: "M6.5 11.6 C 8 4.4, 21.1 2.9, 26.2 8.7 C 30.5 14.5, 28.4 24.7, 21.1 26.9 C 13.1 29.1, 4.4 22.5, 6.5 11.6 Z",
  d: "M5.1 14.5 C 4.4 6.5, 18.9 2.9, 26.2 8 C 32 13.1, 29.1 25.5, 21.1 27.6 C 12.4 29.8, 5.8 22.5, 5.1 14.5 Z",
  e: "M5.1 13.1 C 5.8 5.8, 19.6 3.6, 26.2 8 C 32 13.1, 29.1 25.5, 21.1 27.6 C 12.4 29.8, 4.4 21.1, 5.1 13.1 Z",
  f: "M5.8 12.4 C 6.5 5.1, 20.4 3.6, 26.2 9.5 C 31.3 15.3, 27.6 26.2, 19.6 27.6 C 10.9 29.1, 5.1 20.4, 5.8 12.4 Z",
};

const PAPER: CSSProperties = { fill: "var(--bg)" };
const DOT: CSSProperties = { fill: "var(--stroke)", stroke: "none" };
const ACCENT: CSSProperties = { fill: "var(--accent)" };

interface IconDef {
  blob: string;
  fill: string;
  /** thin / bold 为相对主线宽的细线、粗线样式 */
  draw: (thin: CSSProperties, bold: CSSProperties) => ReactNode;
}

const ICONS: Record<SidebarIconName, IconDef> = {
  // 翻开的书 + 陶土色书签
  guide: {
    blob: BLOB.a,
    fill: "var(--fill-butter)",
    draw: (thin) => (
      <>
        <path style={PAPER} d="M16 9.2 C 12.2 7.2, 7.6 7.1, 4.8 8.6 L 5 24.4 C 8.2 23.2, 12.4 23.4, 16 25.4 Z" />
        <path style={PAPER} d="M16 9.2 C 19.8 7.1, 24.4 7.2, 27.2 8.4 L 27 24.2 C 23.8 23.2, 19.6 23.3, 16 25.4 Z" />
        <path style={ACCENT} d="M21.6 8 L21.7 14.6 L23.4 13 L25.1 14.5 L25 7.6" />
        <path style={thin} d="M8 12.6 L12.8 12.9 M8 16.2 L12.2 16.4 M8 19.8 L11.6 19.9 M19 17.8 L23.6 17.6 M19 21.2 L22.6 21" />
      </>
    ),
  },
  // 一个起点, 分叉成两条路
  live: {
    blob: BLOB.b,
    fill: "var(--fill-coral)",
    draw: () => (
      <>
        <circle style={DOT} cx="6" cy="16" r="2.4" />
        <path d="M8.6 16 L12.5 16 C 16.5 16, 16.8 9.2, 21 9.2 L 26 9.2" />
        <path d="M12.5 16 C 16.5 16, 16.8 22.8, 21 22.8 L 26 22.8" />
        <path d="M23.2 6.6 L26.4 9.2 L23.2 11.8 M23.2 20.2 L26.4 22.8 L23.2 25.4" />
      </>
    ),
  },
  agent: {
    blob: BLOB.c,
    fill: "var(--fill-cactus)",
    draw: () => (
      <>
        <rect style={PAPER} x="7" y="10" width="18" height="15" rx="4" />
        <path d="M16 10 V6 M4 16 V21 M28 16 V21 M13 21 H19" />
        <circle style={DOT} cx="12" cy="16" r="1.4" />
        <circle style={DOT} cx="20" cy="16" r="1.4" />
      </>
    ),
  },
  // 两张叠放的标签 (官网 01)
  vm: {
    blob: BLOB.c,
    fill: "var(--fill-heather)",
    draw: (thin) => (
      <>
        <path style={PAPER} transform="rotate(-10 16 16)" d="M5.5 7.6 L20.4 7.6 L24.6 11.8 L20.4 16 L5.5 16 Z" />
        <path style={PAPER} transform="rotate(8 16 16)" d="M7.6 16 L22.6 16 L26.8 20.2 L22.6 24.4 L7.6 24.4 Z" />
        <circle style={thin} transform="rotate(8 16 16)" cx="22" cy="20.2" r="1.1" />
        <path style={thin} transform="rotate(8 16 16)" d="M10.6 20.2 L16.8 20.2" />
      </>
    ),
  },
  // 一把手画钥匙
  subs: {
    blob: BLOB.d,
    fill: "var(--fill-cactus)",
    draw: (thin) => (
      <>
        <path d="M14.2 15.2 L25.8 26.4 M20.6 21.4 L23.2 18.8 M23.4 24.1 L25.8 21.7" />
        <path style={PAPER} d="M10.5 6.2 C 13.7 6.1, 15.9 8.5, 15.8 11.6 C 15.7 14.7, 13.3 16.9, 10.3 16.8 C 7.3 16.7, 5.1 14.4, 5.2 11.4 C 5.3 8.5, 7.5 6.3, 10.9 6.1 Z" />
        <circle style={thin} cx="9.6" cy="10.6" r="1.4" />
      </>
    ),
  },
  // 线圈笔记本
  logs: {
    blob: BLOB.e,
    fill: "var(--fill-sky)",
    draw: (thin) => (
      <>
        <path style={PAPER} d="M7.8 7.2 C 13 6.9, 19.5 7, 24.6 6.8 L 25 26.8 C 19.5 27.1, 13.5 27, 7.8 27.3 Z" />
        <path d="M11.4 4.6 L11.5 9.4 M15.6 4.5 L15.6 9.3 M19.8 4.6 L19.7 9.4" />
        <path style={thin} d="M11.2 13.8 L21.4 13.6 M11.2 17.6 L20 17.6 M11.2 21.4 L17.4 21.6" />
      </>
    ),
  },
  // 三根柱子, 中间一根上色
  stats: {
    blob: BLOB.f,
    fill: "var(--fill-butter)",
    draw: () => (
      <>
        <path style={PAPER} d="M7.6 25.6 L7.6 17.4 L11.8 17.2 L12 25.6" />
        <path style={ACCENT} d="M14.2 25.6 L14 9.6 L18.4 9.8 L18.2 25.6" />
        <path style={PAPER} d="M20.6 25.6 L20.8 19.8 L25 20 L24.8 25.6" />
        <path d="M4.6 25.8 C 11 25.2, 20 26.4, 27.6 25.6" />
      </>
    ),
  },
  // 锯齿边小票 (官网 08)
  receipts: {
    blob: BLOB.c,
    fill: "var(--fill-oat)",
    draw: (thin) => (
      <>
        <path style={PAPER} d="M9.5 4.6 L22.5 4.6 L22.5 26.9 L20.7 25.1 L18.9 26.9 L17.1 25.1 L15.3 26.9 L13.5 25.1 L11.6 26.9 L9.5 25.1 Z" />
        <path style={thin} d="M12.4 9.2 L19.8 9.2 M12.4 12.4 L18 12.4 M12.4 15.6 L19.8 15.6" />
        <path d="M12.4 20.2 L19.8 20" />
      </>
    ),
  },
  // 首尾相接的两支箭头
  updates: {
    blob: BLOB.b,
    fill: "var(--fill-sky)",
    draw: () => (
      <>
        <path d="M24.6 12.2 C 22.6 7.6, 16.8 5.6, 12.2 7.9 C 8.8 9.6, 7 13, 7.3 16.2" />
        <path d="M25.4 8.2 L24.8 12.4 L20.8 11.6" />
        <path d="M7.4 19.8 C 9.4 24.4, 15.2 26.4, 19.8 24.1 C 23.2 22.4, 25 19, 24.7 15.8" />
        <path d="M6.6 23.8 L7.2 19.6 L11.2 20.4" />
      </>
    ),
  },
  // 软盘: 顶上一块金属挡板 (陶土色读写窗), 底下一张写了字的标签
  backup: {
    blob: BLOB.f,
    fill: "var(--fill-cactus)",
    draw: (thin) => (
      <>
        <path style={PAPER} d="M6.4 6 L22.6 5.8 L26.2 9.4 L26 26.2 L6.2 26.4 Z" />
        <path d="M10.4 6 L10.6 12.4 L20.8 12.2 L20.6 5.9" />
        <path style={{ ...ACCENT, strokeWidth: 0 }} d="M16.8 7.6 L18.8 7.5 L18.9 10.7 L16.9 10.8 Z" />
        <path d="M9.6 26.3 L9.8 17.4 L22.6 17.2 L22.8 26.2" />
        <path style={thin} d="M12.2 20.4 L20.2 20.2 M12.2 23.2 L18 23.3" />
      </>
    ),
  },
  // 粗齿手画齿轮
  settings: {
    blob: BLOB.e,
    fill: "var(--fill-heather)",
    draw: (_thin, bold) => (
      <>
        <path style={bold} d="M23.8 17.6 L26.8 18.2 M20.4 22.7 L22.1 25.2 M14.4 23.8 L13.8 26.8 M9.3 20.4 L6.8 22.1 M8.2 14.4 L5.2 13.8 M11.6 9.3 L9.9 6.8 M17.6 8.2 L18.2 5.2 M22.7 11.6 L25.2 9.9" />
        <path style={PAPER} d="M16 8 C 20.6 7.9, 24.1 11.4, 24 16 C 23.9 20.6, 20.4 24.1, 15.8 24 C 11.3 23.9, 7.9 20.4, 8 15.8 C 8.1 11.4, 11.6 8, 16.4 7.9 Z" />
        <path d="M16 12.9 C 17.8 12.9, 19.1 14.3, 19.1 16 C 19.1 17.8, 17.7 19.1, 16 19.1 C 14.2 19.1, 12.9 17.7, 12.9 16 C 12.9 14.3, 14.3 12.9, 16.3 12.9" />
      </>
    ),
  },
  // 没闭合的圆圈里一个 i
  about: {
    blob: BLOB.d,
    fill: "var(--fill-coral)",
    draw: () => (
      <>
        <path style={PAPER} d="M16 5.5 C 22.5 5.2, 26.8 10, 26.5 16.2 C 26.2 22.4, 21.6 26.8, 15.6 26.5 C 9.6 26.2, 5.3 21.6, 5.6 15.6 C 5.9 9.8, 10.2 5.8, 16.6 5.2" />
        <circle style={DOT} cx="16" cy="10.8" r="1.5" />
        <path d="M14.6 15 L16.1 14.8 L15.8 21.6 M13.8 21.8 C 15.2 21.5, 16.8 21.6, 18.2 21.4" />
      </>
    ),
  },
  // 拱门 + 向外的箭头 (官网 04)
  logout: {
    blob: BLOB.a,
    fill: "var(--fill-oat)",
    draw: () => (
      <>
        <path style={PAPER} d="M6.6 26.6 L6.6 11.4 C 6.6 5.6, 16.4 5.6, 16.4 11.4 L16.4 26.6 Z" />
        <circle style={DOT} cx="13.6" cy="18" r="1.1" />
        <path d="M3.6 26.8 C 11 26.3, 20 27.2, 28.4 26.6" />
        <path d="M19.2 17.2 L27.4 17 M24.4 13.8 L27.6 17 L24.4 20.2" />
      </>
    ),
  },
  // 礼花筒 (侧栏底部「更新内容」入口): 筒身一道陶土色条纹, 筒口两条彩带、三颗彩屑
  whatsnew: {
    blob: BLOB.f,
    fill: "var(--fill-butter)",
    draw: (thin) => (
      <>
        <path style={PAPER} d="M4.8 27.2 L10.4 13.4 C 12.6 17.4, 14.6 19.4, 18.6 21.6 Z" />
        <path style={{ ...ACCENT, strokeWidth: 0 }} d="M7.32 21 L11.01 24.68 L13.77 23.56 L8.44 18.23 Z" />
        <path style={thin} d="M7.32 21 L11.01 24.68 M8.44 18.23 L13.77 23.56" />
        <path d="M13.4 16.4 C 13 12.4, 17.4 12.8, 16.8 9.4 C 16.4 7.2, 18.4 5.4, 21 6.2" />
        <path d="M16.6 19.4 C 19.8 17.2, 22.4 20.8, 25.4 18.6 C 26.8 17.6, 27.8 17.8, 28.6 18.8" />
        <circle style={DOT} cx="21.8" cy="12.2" r="1.2" />
        <circle style={{ ...ACCENT, stroke: "none" }} cx="25.8" cy="8.6" r="1.35" />
        <path style={thin} d="M24.4 13.6 L26.4 14.4 M27.6 4.4 L26.8 6.2 M19.8 3 L20.4 4.8" />
      </>
    ),
  },
  // 跟随系统: 一半涂黑的圆 (昼夜各半)
  "mode-system": {
    blob: BLOB.c,
    fill: "var(--fill-sky)",
    draw: () => (
      <>
        <path style={PAPER} d="M16 7.4 C 20.8 7.3, 24.6 11.2, 24.5 16 C 24.4 20.8, 20.6 24.6, 16 24.5 C 11.2 24.4, 7.4 20.8, 7.5 16 C 7.6 11.2, 11.4 7.5, 16.6 7.4" />
        <path style={DOT} d="M16 7.6 C 11.4 7.8, 7.7 11.4, 7.7 16 C 7.8 20.6, 11.4 24.3, 16 24.3 Z" />
        <path d="M16.1 7.4 C 15.9 13, 16.2 18.8, 16 24.5" />
      </>
    ),
  },
  // 浅色: 太阳, 八道短光芒
  "mode-light": {
    blob: BLOB.a,
    fill: "var(--fill-butter)",
    draw: () => (
      <>
        <path style={PAPER} d="M16 10.4 C 19.3 10.3, 21.7 12.8, 21.6 16 C 21.5 19.2, 19.1 21.7, 16 21.6 C 12.8 21.5, 10.4 19.1, 10.4 15.9 C 10.5 12.9, 12.8 10.5, 16.6 10.4" />
        <path d="M16 4.4 L16.1 7.2 M16 24.8 L15.9 27.6 M4.4 16.1 L7.2 16 M24.8 15.9 L27.6 16 M7.8 7.9 L9.8 9.8 M22.2 22.1 L24.1 24.2 M7.9 24.1 L9.9 22.2 M22.1 9.8 L24.2 7.9" />
      </>
    ),
  },
  // 暗色: 弯月 + 一颗十字星
  "mode-dark": {
    blob: BLOB.e,
    fill: "var(--fill-heather)",
    draw: (thin) => (
      <>
        <path style={PAPER} d="M18.6 5.8 C 12.6 6.6, 8.2 11.4, 8.5 17.2 C 8.8 23.2, 13.8 27.2, 19.6 26.6 C 22.8 26.2, 25.4 24.5, 26.8 22 C 21.2 23.2, 15.4 18.8, 15.4 12.8 C 15.4 10, 16.6 7.6, 18.6 5.8 Z" />
        <path style={thin} d="M23.4 7.4 L23.5 11.6 M21.4 9.5 L25.6 9.4" />
      </>
    ),
  },
  // 主题 (手绘): 斜放的铅笔, 笔尾一道陶土色箍, 笔下一小段划痕
  theme: {
    blob: BLOB.d,
    fill: "var(--fill-coral)",
    draw: (thin) => (
      <>
        <path style={PAPER} d="M6.8 25.2 L8.6 19.6 L21.4 6.8 C 22.6 5.6, 24.6 5.6, 25.8 6.8 C 27 8, 27 10, 25.8 11.2 L13 24 Z" />
        <path style={{ ...ACCENT, strokeWidth: 0 }} d="M19.6 8.6 L23.9 12.9 L25.8 11.2 L21.4 6.8 Z" />
        <path d="M19.6 8.6 L23.9 12.9 M8.6 19.6 L13 24" />
        <path style={DOT} d="M6.8 25.2 L7.6 22.8 L9.2 24.4 Z" />
        <path style={thin} d="M12 28.2 C 15.4 27, 18.2 29.2, 21.6 27.8 C 23 27.2, 24.4 27.4, 25.6 28" />
      </>
    ),
  },
};

export function SidebarIcon({ name, size = 28, blob = true, className }: Props) {
  const def = ICONS[name];
  // 屏幕上的线宽: 侧栏尺寸 1.7px, 展示尺寸 2.3px; 换算回 32 画板的单位
  const sw = ((size <= 36 ? 1.7 : 2.3) * 32) / size;
  const thin: CSSProperties = { strokeWidth: sw * 0.72 };
  const bold: CSSProperties = { strokeWidth: sw * 1.7 };
  return (
    <svg
      className={className}
      viewBox="0 0 32 32"
      width={size}
      height={size}
      aria-hidden="true"
      focusable="false"
      style={{ overflow: "visible", flexShrink: 0 }}
    >
      <path d={def.blob} style={{ fill: def.fill, opacity: blob ? 1 : 0, transition: "opacity .15s" }} />
      <g className="sk-ink" filter="url(#ccr-rough-icon)" style={{ strokeWidth: sw }}>
        {def.draw(thin, bold)}
      </g>
    </svg>
  );
}

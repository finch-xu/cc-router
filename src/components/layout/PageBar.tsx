import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { WindowControls } from "./WindowControls";

interface PageBarProps {
  title: ReactNode;
  /** 标题右侧、分隔线之后的状态文案 (mono 小字), 例如代理运行状态 */
  status?: ReactNode;
  /** 标题左侧的前置控件 (返回箭头等) */
  lead?: ReactNode;
  /** 标题组内、状态之前的附属标记 (状态徽章等) */
  badges?: ReactNode;
  /** 右侧操作区 */
  actions?: ReactNode;
  /** 标题组套 .page-col, 与居中内容列左对齐 (订阅详情页) */
  col?: boolean;
}

/**
 * 每个页面的顶栏 = 窗口标题栏。整条是 data-tauri-drag-region:
 * Tauri 只在事件目标就是挂了该属性的元素本身时才开始拖窗, 所以按钮 / 输入框照常可点;
 * h1 与标题组也挂上, 让可拖区域不止于空白处。
 * Windows / Linux 末端由 WindowControls 补三键; macOS 上它渲染为空。
 */
export function PageBar({ title, status, lead, badges, actions, col }: PageBarProps) {
  return (
    <div className="page-bar" data-tauri-drag-region>
      <div className={cn("page-bar-lead", col && "page-col")} data-tauri-drag-region>
        {lead}
        <h1 data-tauri-drag-region>{title}</h1>
        {badges}
        {status && (
          <>
            <span className="sep" />
            <div className="page-bar-status">{status}</div>
          </>
        )}
      </div>
      <div className="page-bar-right">
        {actions}
        <WindowControls />
      </div>
    </div>
  );
}

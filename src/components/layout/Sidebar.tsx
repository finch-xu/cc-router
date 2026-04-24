import { NavLink } from "react-router-dom";
import { Layers, Key, ScrollText, Settings as SettingsIcon } from "lucide-react";
import { cn } from "@/lib/utils";

const items = [
  { to: "/virtual-models", label: "虚拟模型", icon: Layers },
  { to: "/subscriptions", label: "订阅管理", icon: Key },
  { to: "/request-logs", label: "请求日志", icon: ScrollText },
  { to: "/settings", label: "设置", icon: SettingsIcon },
];

export function Sidebar() {
  return (
    <aside className="w-56 border-r bg-muted/30 px-3 py-4 flex flex-col">
      <div className="px-3 pb-6">
        <div className="text-lg font-semibold tracking-tight">cc-router</div>
        <div className="text-xs text-muted-foreground">多订阅聚合代理</div>
      </div>
      <nav className="flex flex-col gap-1">
        {items.map(({ to, label, icon: Icon }) => (
          <NavLink
            key={to}
            to={to}
            className={({ isActive }) =>
              cn(
                "flex items-center gap-3 rounded-md px-3 py-2 text-sm transition-colors",
                isActive
                  ? "bg-primary text-primary-foreground"
                  : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
              )
            }
          >
            <Icon className="h-4 w-4" />
            {label}
          </NavLink>
        ))}
      </nav>
    </aside>
  );
}

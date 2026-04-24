import { Github } from "lucide-react";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import logoUrl from "@/assets/logo.png";

const REPO_URL = "https://github.com/finch-xu/cc-router";
const VERSION = "0.1.0";

export function AboutPage() {
  return (
    <div className="p-8 space-y-6">
      <div>
        <h1 className="text-2xl font-semibold">关于</h1>
        <p className="text-sm text-muted-foreground">项目信息与版权</p>
      </div>

      <Card className="max-w-xl">
        <CardContent className="pt-8 pb-6 flex flex-col items-center text-center space-y-4">
          <img
            src={logoUrl}
            alt="cc-router logo"
            className="h-24 w-24 rounded-xl shadow-sm"
          />
          <div className="space-y-1">
            <div className="text-xl font-semibold tracking-tight">cc-router</div>
            <div className="text-xs font-mono text-muted-foreground">
              v{VERSION}
            </div>
          </div>
          <p className="text-sm text-muted-foreground max-w-sm">
            本地 HTTP 代理，将多家大模型订阅聚合为单一 Anthropic Messages API
            端点，供 Claude Code 透明切换。
          </p>

          <Button
            variant="outline"
            size="sm"
            onClick={() => openShell(REPO_URL).catch(() => {})}
          >
            <Github className="h-4 w-4" />
            GitHub 仓库
          </Button>

          <div className="w-full border-t pt-4 text-xs text-muted-foreground">
            Copyright © 2026 finch-xu · 以 MIT 许可发布
          </div>
        </CardContent>
      </Card>
    </div>
  );
}

import { useState, useEffect } from "react";
import { AlertTriangle, Loader2 } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { CopyableBlock } from "@/components/CopyableBlock";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { api } from "@/api/tauri";
import { useProxyStatus, useSettings, useUpdateSettings, useEnvSnippet } from "@/hooks/useSettings";

export function SettingsPage() {
  const settings = useSettings();
  const proxy = useProxyStatus();
  const env = useEnvSnippet();
  const updateMut = useUpdateSettings();

  const [port, setPort] = useState<number>(23456);
  const [listenAll, setListenAll] = useState(false);
  const [autostart, setAutostart] = useState(false);
  const [retentionDays, setRetentionDays] = useState(30);
  const [dbLimitMb, setDbLimitMb] = useState(500);
  const [resetDialog, setResetDialog] = useState(false);
  const [resetting, setResetting] = useState(false);

  useEffect(() => {
    if (settings.data) {
      setPort(settings.data.proxy_port);
      setListenAll(settings.data.listen_all);
      setAutostart(settings.data.autostart);
      setRetentionDays(settings.data.log_retention_days);
      setDbLimitMb(settings.data.db_size_limit_mb);
    }
  }, [settings.data?.proxy_port, settings.data?.listen_all]);

  const needsRestart =
    settings.data !== undefined &&
    (settings.data.proxy_port !== port || settings.data.listen_all !== listenAll);

  async function save() {
    await updateMut.mutateAsync({
      proxy_port: port,
      listen_all: listenAll,
      autostart,
      log_retention_days: retentionDays,
      db_size_limit_mb: dbLimitMb,
    });
  }

  async function confirmReset() {
    setResetting(true);
    try {
      await api.factoryReset();
      // app 会自动重启; 这里不会真正 resolve
    } catch (e) {
      setResetting(false);
      setResetDialog(false);
      alert(`恢复出厂失败: ${e}`);
    }
  }

  return (
    <div className="p-8 max-w-3xl space-y-6">
      <h1 className="text-2xl font-semibold">设置</h1>

      <Card>
        <CardHeader>
          <CardTitle>代理服务</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid grid-cols-[140px_1fr] gap-3 items-center">
            <Label>监听端口</Label>
            <div className="flex items-center gap-2">
              <Input
                type="number"
                value={port}
                onChange={(e) => setPort(Number(e.target.value) || 23456)}
                className="max-w-[120px]"
              />
              <span className="text-xs text-muted-foreground">
                当前实际: {proxy.data?.port ?? "-"}
              </span>
            </div>
          </div>

          <div className="grid grid-cols-[140px_1fr] gap-3 items-start">
            <Label className="mt-2">监听地址</Label>
            <div className="space-y-2">
              <div className="flex items-center gap-3">
                <Switch checked={listenAll} onCheckedChange={setListenAll} />
                <span className="text-sm">
                  {listenAll ? "局域网可访问 (0.0.0.0)" : "仅本机 (127.0.0.1)"}
                </span>
              </div>
              <p className="text-xs text-muted-foreground">
                {listenAll
                  ? "⚠️ 开启后，同网段的任何设备都能调用代理。代理不做鉴权，慎用于不受信任的网络。"
                  : "默认仅本机回环。开启「局域网可访问」后其他设备的 Claude Code 可以指向本机 IP。"}
              </p>
            </div>
          </div>

          <div className="grid grid-cols-[140px_1fr] gap-3 items-center">
            <Label>运行状态</Label>
            <div className="flex items-center gap-2 text-sm">
              <span
                className={`h-2 w-2 rounded-full ${
                  proxy.data?.running ? "bg-status-healthy" : "bg-status-disabled"
                }`}
              />
              {proxy.data?.running ? "运行中" : "未运行"}
            </div>
          </div>
          <div className="grid grid-cols-[140px_1fr] gap-3 items-center">
            <Label>开机自启动</Label>
            <Switch checked={autostart} onCheckedChange={setAutostart} />
          </div>

          {needsRestart && (
            <Alert variant="warning">
              <AlertTriangle className="h-4 w-4" />
              <AlertDescription>
                修改端口或监听地址需要重启 app 才生效。保存后请手动退出 cc-router 再启动。
              </AlertDescription>
            </Alert>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Claude Code 配置</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-sm text-muted-foreground">
            把下面的环境变量加到 Claude Code 的配置里（或 ~/.claude/settings.json 的 env 字段）
          </p>
          {env.data && <CopyableBlock text={env.data} />}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>数据存储</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid grid-cols-[160px_1fr] gap-3 items-center">
            <Label>请求日志保留期</Label>
            <Select
              value={String(retentionDays)}
              onValueChange={(v) => setRetentionDays(Number(v))}
            >
              <SelectTrigger className="max-w-[200px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="7">7 天</SelectItem>
                <SelectItem value="30">30 天</SelectItem>
                <SelectItem value="90">90 天</SelectItem>
                <SelectItem value="36500">永久</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="grid grid-cols-[160px_1fr] gap-3 items-center">
            <Label>数据库大小上限</Label>
            <Select
              value={String(dbLimitMb)}
              onValueChange={(v) => setDbLimitMb(Number(v))}
            >
              <SelectTrigger className="max-w-[200px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="100">100 MB</SelectItem>
                <SelectItem value="500">500 MB</SelectItem>
                <SelectItem value="1024">1 GB</SelectItem>
                <SelectItem value="10240">10 GB (近似无限)</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </CardContent>
      </Card>

      <div className="flex justify-end">
        <Button onClick={save} disabled={updateMut.isPending}>
          保存设置
        </Button>
      </div>

      <Card className="border-destructive/50">
        <CardHeader>
          <CardTitle className="text-destructive">危险区域</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <Alert variant="destructive">
            <AlertTriangle className="h-4 w-4" />
            <AlertTitle>恢复出厂设置</AlertTitle>
            <AlertDescription>
              清空所有订阅、Keychain 中的 API Key、虚拟模型绑定、请求日志、设置文件。
              app 会自动重启并进入初始欢迎页。此操作不可撤销。
            </AlertDescription>
          </Alert>
          <Button variant="destructive" onClick={() => setResetDialog(true)}>
            恢复出厂设置
          </Button>
        </CardContent>
      </Card>

      <Dialog open={resetDialog} onOpenChange={(v) => !resetting && setResetDialog(v)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              <div className="flex items-center gap-2">
                <AlertTriangle className="h-4 w-4 text-destructive" />
                确认恢复出厂设置?
              </div>
            </DialogTitle>
            <DialogDescription>
              下面的数据将被永久删除：
              <ul className="mt-2 list-disc pl-5 text-sm">
                <li>所有订阅（包括 Keychain 中存储的 API Key）</li>
                <li>虚拟模型绑定</li>
                <li>请求日志和模型列表缓存</li>
                <li>app 设置（端口、保留期等）</li>
              </ul>
              <p className="mt-3">操作完成后 app 会自动重启。</p>
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setResetDialog(false)} disabled={resetting}>
              取消
            </Button>
            <Button variant="destructive" onClick={confirmReset} disabled={resetting}>
              {resetting && <Loader2 className="h-4 w-4 animate-spin" />}
              确认清空并重启
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

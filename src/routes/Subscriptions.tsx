import { Link } from "react-router-dom";
import { Plus, Key } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { StatusBadge } from "@/components/StatusBadge";
import { ProviderIcon } from "@/components/ProviderIcon";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useProviders } from "@/hooks/useProviders";

export function SubscriptionsPage() {
  const subs = useSubscriptions();
  const providers = useProviders();

  const providerOf = (id: string) => providers.data?.find((p) => p.id === id);

  return (
    <div className="p-8 space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold">订阅管理</h1>
          <p className="text-sm text-muted-foreground">
            每个订阅对应一个厂商的 API Key
          </p>
        </div>
        <Button asChild>
          <Link to="/subscriptions/new">
            <Plus className="h-4 w-4" /> 添加订阅
          </Link>
        </Button>
      </div>

      {subs.isLoading && <div className="text-sm text-muted-foreground">加载中…</div>}

      {subs.data && subs.data.length === 0 && (
        <Card>
          <CardContent className="py-12 text-center space-y-3">
            <Key className="h-8 w-8 mx-auto text-muted-foreground" />
            <div className="text-sm text-muted-foreground">
              还没有订阅。点击"添加订阅"开始。
            </div>
            <Button asChild size="sm">
              <Link to="/subscriptions/new">
                <Plus className="h-4 w-4" /> 添加第一个订阅
              </Link>
            </Button>
          </CardContent>
        </Card>
      )}

      {subs.data && subs.data.length > 0 && (
        <div className="rounded-lg border">
          <table className="w-full text-sm">
            <thead className="border-b bg-muted/40 text-xs uppercase text-muted-foreground">
              <tr>
                <th className="px-4 py-2 text-left font-medium">状态</th>
                <th className="px-4 py-2 text-left font-medium">厂商</th>
                <th className="px-4 py-2 text-left font-medium">备注</th>
                <th className="px-4 py-2 text-left font-medium">引用</th>
                <th className="px-4 py-2"></th>
              </tr>
            </thead>
            <tbody>
              {subs.data.map((sub) => (
                <tr key={sub.id} className="border-b last:border-b-0 hover:bg-muted/20">
                  <td className="px-4 py-3">
                    <StatusBadge state={sub.state} />
                  </td>
                  <td className="px-4 py-3">
                    <div className="flex items-center gap-2">
                      <ProviderIcon iconId={providerOf(sub.provider_id)?.icon} size={18} />
                      <span>{providerOf(sub.provider_id)?.display_name ?? sub.provider_id}</span>
                    </div>
                  </td>
                  <td className="px-4 py-3 font-medium">{sub.display_name}</td>
                  <td className="px-4 py-3 text-muted-foreground">
                    {sub.referenced_by.length > 0
                      ? `used: ${sub.referenced_by.length}`
                      : "未使用"}
                  </td>
                  <td className="px-4 py-3 text-right">
                    <Button asChild variant="ghost" size="sm">
                      <Link to={`/subscriptions/${sub.id}`}>查看</Link>
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

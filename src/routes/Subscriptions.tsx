import { Link } from "react-router-dom";
import { Plus, Key } from "lucide-react";
import { StatusBadge } from "@/components/StatusBadge";
import { ProviderLogo } from "@/components/ProviderLogo";
import { EmptyState } from "@/components/EmptyState";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { fmtTimeShort } from "@/lib/format";

// SubscriptionDto 不带 api_key 预览(CLAUDE.md), 用固定遮罩占位
const MASKED_KEY = "•••••••••••••••";

export function SubscriptionsPage() {
  const subs = useSubscriptions();

  return (
    <>
      <div className="page-actions">
        <div className="page-header" style={{ margin: 0 }}>
          <h1>订阅管理</h1>
          <div className="subtitle">每个订阅对应一个厂商的 API Key。状态实时反映健康检查结果。</div>
        </div>
        <Link className="btn primary" to="/subscriptions/new">
          <Plus size={12} /> 添加订阅
        </Link>
      </div>

      {subs.isLoading && <div className="field-hint">加载中…</div>}

      {subs.data && subs.data.length === 0 && (
        <EmptyState
          icon={Key}
          message="还没有订阅。点击「添加订阅」开始。"
          action={
            <Link className="btn primary sm" to="/subscriptions/new">
              <Plus size={12} /> 添加第一个订阅
            </Link>
          }
        />
      )}

      {subs.data && subs.data.length > 0 && (
        <div className="card">
          <table className="table">
            <thead>
              <tr>
                <th style={{ width: 100 }}>状态</th>
                <th>厂商</th>
                <th>备注</th>
                <th style={{ width: 160 }}>API Key</th>
                <th style={{ width: 90 }}>引用</th>
                <th style={{ width: 100 }}>更新时间</th>
                <th style={{ width: 80 }}></th>
              </tr>
            </thead>
            <tbody>
              {subs.data.map((sub) => {
                return (
                  <tr key={sub.id}>
                    <td>
                      <StatusBadge state={sub.state} />
                    </td>
                    <td>
                      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                        <ProviderLogo iconId={sub.provider_icon} size={24} />
                        <span style={{ fontWeight: 500, color: "var(--ink)" }}>
                          {sub.provider_display_name}
                        </span>
                        {sub.is_user_defined && (
                          <span
                            style={{
                              fontSize: 10,
                              padding: "1px 6px",
                              borderRadius: 4,
                              background: "var(--bg-muted, #f0f0f0)",
                              color: "var(--ink-3)",
                            }}
                          >
                            🔧 自定义
                          </span>
                        )}
                      </div>
                    </td>
                    <td>{sub.display_name}</td>
                    <td className="mono" style={{ color: "var(--ink-3)", fontSize: 12 }}>
                      {MASKED_KEY}
                    </td>
                    <td>
                      {sub.referenced_by.length > 0 ? (
                        <span className="pill tag mono">used: {sub.referenced_by.length}</span>
                      ) : (
                        <span className="field-hint" style={{ marginTop: 0, fontSize: 11.5 }}>
                          未使用
                        </span>
                      )}
                    </td>
                    <td className="mono" style={{ color: "var(--ink-3)", fontSize: 12 }}>
                      {fmtTimeShort(sub.updated_at)}
                    </td>
                    <td>
                      <Link className="btn sm" to={`/subscriptions/${sub.id}`}>
                        查看
                      </Link>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

import { useNavigate, useSearchParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { CopyableBlock } from "@/components/CopyableBlock";
import { api } from "@/api/tauri";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useVirtualModels, useUpdateVirtualModel } from "@/hooks/useVirtualModels";
import { useEnvSnippet } from "@/hooks/useSettings";
import type { VirtualModelName } from "@/types";

type Step = 1 | 2 | 3;

function parseStep(raw: string | null): Step {
  if (raw === "2") return 2;
  if (raw === "3") return 3;
  return 1;
}

export function OnboardingPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [searchParams, setSearchParams] = useSearchParams();
  const step = parseStep(searchParams.get("step"));

  const subs = useSubscriptions();
  const vms = useVirtualModels();
  const env = useEnvSnippet();
  const updateVmMut = useUpdateVirtualModel();

  const hasSubscription = (subs.data?.length ?? 0) > 0;

  function goToStep(n: Step) {
    setSearchParams({ step: String(n) }, { replace: false });
  }

  async function assignAllToLastCreated() {
    const firstSub = subs.data?.[0];
    if (!firstSub) return;
    const names: VirtualModelName[] = ["model-opus", "model-sonnet", "model-haiku"];
    for (const name of names) {
      const current = vms.data?.find((v) => v.name === name);
      const existing = new Set(current?.subscription_ids ?? []);
      const merged = [...(current?.subscription_ids ?? []), firstSub.id].filter(
        (id, i, arr) => existing.has(id) || arr.indexOf(id) === i,
      );
      await updateVmMut.mutateAsync({
        name,
        input: {
          mode: current?.mode ?? "sequential",
          subscription_ids: Array.from(new Set(merged)),
        },
      });
    }
    goToStep(3);
  }

  async function finish() {
    await api.completeOnboarding();
    queryClient.invalidateQueries({ queryKey: ["onboarding-state"] });
    navigate("/virtual-models");
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-muted/30 p-8">
      <div className="w-full max-w-xl space-y-6">
        <div className="space-y-1 text-center">
          <h1 className="text-3xl font-semibold">欢迎使用 cc-router</h1>
          <p className="text-sm text-muted-foreground">
            只需三步即可开始为 Claude Code 聚合多家订阅
          </p>
        </div>

        <div className="flex items-center justify-center gap-2 text-xs text-muted-foreground">
          {[1, 2, 3].map((n) => (
            <span
              key={n}
              className={`h-1.5 w-8 rounded-full ${
                step >= (n as Step) ? "bg-primary" : "bg-muted-foreground/30"
              }`}
            />
          ))}
        </div>

        <div className="rounded-xl border bg-card p-8 space-y-4">
          {step === 1 && (
            <>
              <h2 className="text-lg font-semibold">第 1 步 · 添加订阅</h2>
              <p className="text-sm text-muted-foreground">
                你需要至少一个厂商订阅才能开始使用。
              </p>
              <div className="flex gap-2 pt-2">
                <Button
                  onClick={() =>
                    navigate(
                      `/subscriptions/new?returnTo=${encodeURIComponent("/onboarding?step=2")}`,
                    )
                  }
                >
                  去添加订阅
                </Button>
                {hasSubscription && (
                  <Button variant="outline" onClick={() => goToStep(2)}>
                    已有订阅，继续
                  </Button>
                )}
              </div>
              {hasSubscription && (
                <p className="text-xs text-muted-foreground">
                  已检测到 {subs.data?.length} 个订阅
                </p>
              )}
            </>
          )}

          {step === 2 && (
            <>
              <h2 className="text-lg font-semibold">第 2 步 · 分配虚拟模型</h2>
              <p className="text-sm text-muted-foreground">
                把第一个订阅一键绑定到三个虚拟模型（可稍后在"虚拟模型"页调整）
              </p>
              <div className="flex gap-2 pt-2">
                <Button onClick={assignAllToLastCreated} disabled={!hasSubscription}>
                  一键分配到全部
                </Button>
                <Button variant="outline" onClick={() => goToStep(3)}>
                  跳过 / 手动配置
                </Button>
                <Button variant="ghost" onClick={() => goToStep(1)}>
                  上一步
                </Button>
              </div>
            </>
          )}

          {step === 3 && (
            <>
              <h2 className="text-lg font-semibold">第 3 步 · 配置 Claude Code</h2>
              <p className="text-sm text-muted-foreground">
                把下面这几行加到 Claude Code 的环境配置里
              </p>
              {env.data && <CopyableBlock text={env.data} />}
              <p className="text-xs text-muted-foreground">
                或写入 ~/.claude/settings.json 的 "env" 字段
              </p>
              <div className="flex gap-2 pt-2">
                <Button variant="ghost" onClick={() => goToStep(2)}>
                  上一步
                </Button>
                <Button onClick={finish}>完成</Button>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

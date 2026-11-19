import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useCustomVirtualModels } from "@/hooks/useVirtualModels";
import { useRouteFlashState } from "@/hooks/useRouteFlash";
import { useT } from "@/i18n";
import type { CustomVirtualModelDto } from "@/types";

export function CustomModelMappings() {
  const { t } = useT();
  const models = useCustomVirtualModels();
  const subscriptions = useSubscriptions();
  if (!models.data?.length) return null;
  const byId = new Map(subscriptions.data?.map((sub) => [sub.id, sub.display_name]));
  return (
    <section className="card">
      <div className="card-head">
        <h2 className="card-title">{t("customModels.title")}</h2>
      </div>
      <div className="overflow-x-auto">
        <table className="table">
          <thead><tr>
            <th>{t("customModels.clientName")}</th>
            <th>{t("customModels.subscription")}</th>
            <th>{t("customModels.upstream")}</th>
          </tr></thead>
          <tbody>{models.data.flatMap((model) => model.targets.length
            ? model.targets.map((target) => (
              <ModelMappingRow key={model.name + "/" + target.subscription_id} name={model.name}
                subscriptionId={target.subscription_id} subscriptionName={byId.get(target.subscription_id)}
                upstream={target.upstream_model_id} />
            ))
            : <tr key={model.name}><td><code>{model.name}</code></td>
                <td colSpan={2}>{t("customModels.noTargets")}</td></tr>
          )}</tbody>
        </table>
      </div>
    </section>
  );
}

function ModelMappingRow({ name, subscriptionId, subscriptionName, upstream }: {
  name: CustomVirtualModelDto["name"];
  subscriptionId: string;
  subscriptionName?: string;
  upstream: string;
}) {
  const flash = useRouteFlashState(name, subscriptionId);
  const background = flash?.kind === "success" ? "var(--ok-bg)"
    : flash?.kind === "error" ? "var(--err-bg)"
    : flash?.kind === "attempt" ? "var(--accent-bg)" : undefined;
  return (
    <tr style={{ background }}>
      <td><code>{name}</code></td>
      <td>{subscriptionName ?? subscriptionId}</td>
      <td><code>{upstream}</code></td>
    </tr>
  );
}

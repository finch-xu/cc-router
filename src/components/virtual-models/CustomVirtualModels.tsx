import { useId, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { SUBSCRIPTIONS_KEY, SUBSCRIPTION_DETAIL_KEY } from "@/hooks/useSubscriptions";
import { CUSTOM_VIRTUAL_MODELS_KEY, useCustomVirtualModels } from "@/hooks/useVirtualModels";
import { useT } from "@/i18n";
import { MODE_LABEL_KEY, VM_ORDER } from "@/lib/virtualModels";
import { runtime } from "@/runtime";
import type { CustomVirtualModelDto, CustomVirtualModelInput, CustomVirtualModelName, CustomVirtualModelTarget, RoutingMode, SubscriptionDto } from "@/types";

const emptyTarget = (): CustomVirtualModelTarget => ({ subscription_id: "", upstream_model_id: "" });

export function CustomVirtualModels({ subscriptions }: { subscriptions: SubscriptionDto[] }) {
  const { t } = useT();
  const query = useCustomVirtualModels();
  const queryClient = useQueryClient();
  const [editor, setEditor] = useState<"new" | CustomVirtualModelDto | null>(null);
  const [remove, setRemove] = useState<CustomVirtualModelDto | null>(null);
  const [copyFeedback, setCopyFeedback] = useState("");
  const eligible = subscriptions.filter((sub) => sub.endpoint_protocol !== "systemone");
  const byId = new Map(subscriptions.map((sub) => [sub.id, sub]));
  function refresh() {
    void queryClient.invalidateQueries({ queryKey: CUSTOM_VIRTUAL_MODELS_KEY });
    void queryClient.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY });
    void queryClient.invalidateQueries({ queryKey: SUBSCRIPTION_DETAIL_KEY });
  }
  const deletion = useMutation({
    mutationFn: api.deleteCustomVirtualModel,
    onSuccess: () => { setRemove(null); refresh(); },
  });

  if (editor) return (
    <ModelEditor model={editor === "new" ? undefined : editor} subscriptions={eligible}
      knownNames={(query.data ?? []).map((model) => model.name)}
      onClose={() => setEditor(null)} onSaved={() => { setEditor(null); refresh(); }} />
  );

  return (
    <section className="grid gap-4">
      <div className="flex flex-wrap justify-between items-center gap-2">
        <p className="field-hint">{t("customModels.hint")}</p>
        <button type="button" className="btn primary" onClick={() => setEditor("new")}
          disabled={eligible.length === 0 || query.isPending || query.isError}>
          {t("customModels.new")}
        </button>
      </div>
      {eligible.length === 0 && <p className="field-hint">{t("customModels.noSubscriptions")}</p>}
      {query.isPending && <p>{t("common.loading")}</p>}
      {query.isError && <p role="alert">{t("customModels.loadError")}</p>}
      {query.data && query.data.length === 0 && <p>{t("customModels.empty")}</p>}
      {!!query.data?.length && (
        <div className="overflow-x-auto">
          <table className="table">
            <thead><tr>
              <th>{t("customModels.clientName")}</th>
              <th>{t("customModels.subscription")}</th>
              <th>{t("customModels.upstream")}</th>
              <th />
            </tr></thead>
            <tbody>
              {query.data.flatMap((model) => {
                const targets = model.targets.length ? model.targets : [null];
                return targets.map((target, index) => (
                  <tr key={model.name + "/" + (target?.subscription_id ?? "")}>
                    {index === 0 && <td rowSpan={targets.length}><code>{model.name}</code></td>}
                    <td>{target ? byId.get(target.subscription_id)?.display_name ?? target.subscription_id : t("customModels.noTargets")}</td>
                    <td>{target && <code>{target.upstream_model_id}</code>}</td>
                    {index === 0 && <td rowSpan={targets.length}>
                      <div className="flex flex-wrap gap-1.5">
                        <button type="button" className="btn sm" onClick={async () => {
                          try {
                            await runtime.copyText(model.name);
                            setCopyFeedback(t("customModels.copied", { name: model.name }));
                          } catch {
                            setCopyFeedback(t("customModels.manualCopy", { name: model.name }));
                          }
                        }}>{t("common.copy")}</button>
                        <button type="button" className="btn sm" onClick={() => setEditor(model)}>
                          {t("customModels.edit")}
                        </button>
                        <button type="button" className="btn sm" onClick={() => {
                          deletion.reset();
                          setRemove(model);
                        }}>{t("common.delete")}</button>
                      </div>
                    </td>}
                  </tr>
                ));
              })}
            </tbody>
          </table>
        </div>
      )}
      {copyFeedback && <p role="status">{copyFeedback}</p>}
      <Dialog open={remove !== null} onOpenChange={(open) => {
        if (!open && !deletion.isPending) setRemove(null);
      }}>
        <DialogContent className="cc-dialog">
          <DialogHeader><DialogTitle>{remove?.name}</DialogTitle></DialogHeader>
          <p>{t("customModels.deleteHint")}</p>
          {deletion.isError && <p role="alert">{t("customModels.deleteError")}</p>}
          <DialogFooter>
            <button type="button" className="btn" onClick={() => setRemove(null)} disabled={deletion.isPending}>
              {t("common.cancel")}
            </button>
            <button type="button" className="btn danger" disabled={deletion.isPending}
              onClick={() => { if (remove) deletion.mutate(remove.name); }}>{t("common.delete")}</button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}

function ModelEditor({ model, subscriptions, knownNames, onClose, onSaved }: {
  model?: CustomVirtualModelDto;
  subscriptions: SubscriptionDto[];
  knownNames: string[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useT();
  const [name, setName] = useState<string>(model?.name ?? "model-");
  const [mode, setMode] = useState<RoutingMode>(model?.mode ?? "sequential");
  const [targets, setTargets] = useState(model?.targets.length ? model.targets : [emptyTarget()]);
  const usedNames: readonly string[] = [...VM_ORDER, ...knownNames];
  const duplicate = !model && usedNames.includes(name);
  const ready = /^model-[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name) && !duplicate &&
    targets.every((target) => target.subscription_id && target.upstream_model_id.trim()) &&
    new Set(targets.map((target) => target.subscription_id)).size === targets.length;
  const save = useMutation({
    mutationFn: (input: CustomVirtualModelInput) => model
      ? api.updateCustomVirtualModel(model.name, input)
      : api.createCustomVirtualModel({ name: name as CustomVirtualModelName, ...input }),
    onSuccess: onSaved,
  });
  function update(index: number, target: CustomVirtualModelTarget) {
    setTargets((current) => current.map((row, i) => i === index ? target : row));
  }
  function moveUp(index: number) {
    setTargets((current) => {
      const next = [...current];
      [next[index - 1], next[index]] = [next[index], next[index - 1]];
      return next;
    });
  }
  const first = targets[0];
  return (
    <form className="grid gap-4 max-w-3xl" onSubmit={(event) => {
      event.preventDefault();
      if (ready && !save.isPending) save.mutate({ mode, targets });
    }}>
      <h2>{t(model ? "customModels.edit" : "customModels.new")}</h2>
      <label className="grid gap-1">
        {t("customModels.clientName")}
        <input className="input mono" value={name} maxLength={64} readOnly={!!model}
          disabled={save.isPending} onChange={(event) => setName(event.target.value)} />
        <span className="field-hint">{t(model ? "customModels.fixedName" : "customModels.nameHint")}</span>
        {duplicate && <span role="alert">{t("customModels.duplicate")}</span>}
      </label>
      <TargetFields target={first} subscriptions={subscriptions} targets={targets}
        disabled={save.isPending} onChange={(target) => update(0, target)} />
      <details open={targets.length > 1 || mode !== "sequential" ? true : undefined}>
        <summary>{t("customModels.advanced")}</summary>
        <div className="mt-3 grid gap-3">
          <label className="grid gap-1 w-fit">
            {t("virtualModels.mode.label")}
            <select className="select" value={mode} disabled={save.isPending}
              onChange={(event) => setMode(event.target.value as RoutingMode)}>
              {(Object.keys(MODE_LABEL_KEY) as RoutingMode[]).map((item) => (
                <option key={item} value={item}>{t(MODE_LABEL_KEY[item])}</option>
              ))}
            </select>
          </label>
          {targets.slice(1).map((target, offset) => {
            const index = offset + 1;
            return <div key={index} className="grid gap-2">
              <TargetFields target={target} subscriptions={subscriptions} targets={targets}
                disabled={save.isPending} onChange={(value) => update(index, value)} />
              <div className="flex gap-2 justify-end">
                <button type="button" className="btn sm" disabled={save.isPending}
                  onClick={() => moveUp(index)}>{t("customModels.moveUp")}</button>
                <button type="button" className="btn sm" disabled={save.isPending}
                  onClick={() => setTargets((current) => current.filter((_, i) => i !== index))}>
                  {t("common.delete")}
                </button>
              </div>
            </div>;
          })}
          <button type="button" className="btn sm w-fit" disabled={save.isPending || targets.length >= subscriptions.length}
            onClick={() => setTargets((current) => [...current, emptyTarget()])}>{t("customModels.addTarget")}</button>
        </div>
      </details>
      <p className="p-3 border rounded-md" style={{ background: "var(--accent-bg)" }}>
        <code>{name}</code> → {subscriptions.find((sub) => sub.id === first.subscription_id)?.display_name || t("customModels.subscription")}
        {" → "}<code>{first.upstream_model_id || t("customModels.upstream")}</code>
      </p>
      {save.isError && <p role="alert">{t("customModels.saveError")}</p>}
      <div className="flex gap-2 justify-end">
        <button type="button" className="btn" disabled={save.isPending} onClick={onClose}>{t("common.cancel")}</button>
        <button type="submit" className="btn primary" disabled={!ready || save.isPending}>{t("common.save")}</button>
      </div>
    </form>
  );
}

function TargetFields({ target, subscriptions, targets, disabled, onChange }: {
  target: CustomVirtualModelTarget;
  subscriptions: SubscriptionDto[];
  targets: CustomVirtualModelTarget[];
  disabled: boolean;
  onChange: (value: CustomVirtualModelTarget) => void;
}) {
  const { t } = useT();
  const listId = useId();
  const client = useQueryClient();
  const discovery = useMutation({
    mutationFn: api.refreshModelList,
    onSuccess: () => { void client.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY }); },
  });
  const sub = subscriptions.find((item) => item.id === target.subscription_id);
  const models = new Set([...(sub?.model_cache?.models.map((model) => model.id) ?? []),
    ...(sub?.model_discovery.example_models ?? [])]);
  return (
    <div className="grid gap-3 sm:grid-cols-2">
      <label className="grid gap-1">
        {t("customModels.subscription")}
        <select className="select" value={target.subscription_id} disabled={disabled}
          onChange={(event) => {
            discovery.reset();
            onChange({ subscription_id: event.target.value, upstream_model_id: "" });
          }}>
          <option value="">{t("customModels.selectSubscription")}</option>
          {subscriptions.map((item) => (
            <option key={item.id} value={item.id}
              disabled={item.id !== target.subscription_id && targets.some((row) => row.subscription_id === item.id)}>
              {item.display_name}
            </option>
          ))}
        </select>
      </label>
      <div className="grid gap-1">
        <label className="grid gap-1">
          {t("customModels.upstream")}
          <input className="input mono" list={listId} maxLength={160} value={target.upstream_model_id}
            disabled={disabled || !sub}
            onChange={(event) => onChange({ ...target, upstream_model_id: event.target.value })} />
          <datalist id={listId}>
            {[...models].map((id) => <option key={id} value={id} />)}
          </datalist>
        </label>
        {sub?.model_discovery.enabled && (
          <div>
            <button type="button" className="btn sm" disabled={disabled || discovery.isPending}
              onClick={() => discovery.mutate(sub.id)}>
              {t(discovery.isPending ? "common.loading" : "customModels.discover")}
            </button>
            {(discovery.isError || discovery.data?.kind === "manual_fallback") && (
              <p className="field-hint" role="status">{t("customModels.discoveryFallback")}</p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

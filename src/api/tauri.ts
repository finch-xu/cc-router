import { invoke } from "@tauri-apps/api/core";
import type {
  CreateSubscriptionInput,
  EventFilters,
  ListEventsResult,
  ListRequestsResult,
  OnboardingState,
  ProviderInfo,
  ProxyStatus,
  RefreshModelListResult,
  RequestLogFilters,
  Settings,
  SettingsPatch,
  SubscriptionDto,
  SubscriptionPatch,
  TestConnectionResult,
  UpdateInfo,
  UpdateVirtualModelInput,
  VirtualModelDto,
  VirtualModelName,
} from "@/types";

export const api = {
  // providers
  listProviders: () => invoke<ProviderInfo[]>("list_providers"),

  // subscriptions
  listSubscriptions: () => invoke<SubscriptionDto[]>("list_subscriptions"),
  getSubscription: (id: string) =>
    invoke<SubscriptionDto>("get_subscription", { id }),
  createSubscription: (input: CreateSubscriptionInput) =>
    invoke<SubscriptionDto>("create_subscription", { input }),
  updateSubscription: (id: string, patch: SubscriptionPatch) =>
    invoke<SubscriptionDto>("update_subscription", { id, patch }),
  updateSubscriptionKey: (id: string, newKey: string) =>
    invoke<void>("update_subscription_key", { id, newKey }),
  deleteSubscription: (id: string) =>
    invoke<void>("delete_subscription", { id }),
  setSubscriptionEnabled: (id: string, enabled: boolean) =>
    invoke<void>("set_subscription_enabled", { id, enabled }),
  testConnection: (id: string) =>
    invoke<TestConnectionResult>("test_connection", { id }),
  refreshModelList: (id: string) =>
    invoke<RefreshModelListResult>("refresh_model_list", { id }),

  // virtual models
  listVirtualModels: () => invoke<VirtualModelDto[]>("list_virtual_models"),
  updateVirtualModel: (name: VirtualModelName, input: UpdateVirtualModelInput) =>
    invoke<void>("update_virtual_model", { name, input }),

  // request logs
  listRequests: (
    page: number,
    pageSize: number,
    filters?: RequestLogFilters,
  ) =>
    invoke<ListRequestsResult>("list_requests", { page, pageSize, filters }),

  // event stream (kind=request / subscription_state_change / system_error)
  listEvents: (
    page: number,
    pageSize: number,
    filters?: EventFilters,
  ) =>
    invoke<ListEventsResult>("list_events", { page, pageSize, filters }),

  // settings / proxy
  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (patch: SettingsPatch) =>
    invoke<Settings>("update_settings", { patch }),
  generateNewToken: () => invoke<Settings>("generate_new_token"),
  proxyStatus: () => invoke<ProxyStatus>("proxy_status"),
  envSnippet: () => invoke<string>("env_snippet"),

  // onboarding
  getOnboardingState: () => invoke<OnboardingState>("get_onboarding_state"),
  completeOnboarding: () => invoke<void>("complete_onboarding"),

  // app
  factoryReset: () => invoke<void>("factory_reset"),

  // updater (运行时按 settings.update_source 切换 manifest 源)
  checkForUpdate: () => invoke<UpdateInfo | null>("check_for_update"),
  downloadInstallUpdate: () => invoke<void>("download_install_update"),
};

import {
  createContext,
  useContext,
  useCallback,
  useMemo,
  type ReactNode,
} from "react";
import { useSettings } from "@/hooks/useSettings";
import zh from "./locales/zh.json";
import en from "./locales/en.json";
import ja from "./locales/ja.json";

export type Locale = "zh" | "en" | "ja";
export type LanguagePref = "system" | "zh" | "en" | "ja";

const dictionaries: Record<Locale, Record<string, string>> = {
  zh: zh as Record<string, string>,
  en: en as Record<string, string>,
  ja: ja as Record<string, string>,
};

/** Read system locale via webview navigator. zh* → zh, ja* → ja, otherwise → en. */
export function detectSystemLocale(): Locale {
  if (typeof navigator === "undefined") return "en";
  const lang =
    navigator.language || (navigator.languages && navigator.languages[0]) || "en";
  const lower = lang.toLowerCase();
  if (lower.startsWith("zh")) return "zh";
  if (lower.startsWith("ja")) return "ja";
  return "en";
}

function resolveLocale(pref: LanguagePref | undefined): Locale {
  if (!pref || pref === "system") return detectSystemLocale();
  return pref;
}

export type TParams = Record<string, string | number>;
export type TFunction = (key: string, params?: TParams) => string;

type I18nValue = {
  t: TFunction;
  locale: Locale;
};

const I18nContext = createContext<I18nValue>({
  t: (k) => k,
  locale: "en",
});

function applyParams(template: string, params: TParams): string {
  let out = template;
  for (const [k, v] of Object.entries(params)) {
    out = out.split(`{${k}}`).join(String(v));
  }
  return out;
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const { data: settings } = useSettings();
  const locale = resolveLocale(
    settings?.preferred_language as LanguagePref | undefined,
  );
  const dict = dictionaries[locale];

  const t = useCallback<TFunction>(
    (key, params) => {
      const tpl = dict[key] ?? key;
      return params ? applyParams(tpl, params) : tpl;
    },
    [dict],
  );

  const value = useMemo<I18nValue>(() => ({ t, locale }), [t, locale]);
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useT() {
  return useContext(I18nContext);
}

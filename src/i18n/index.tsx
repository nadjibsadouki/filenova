import { createContext, useContext, useEffect, useMemo, type ReactNode } from "react";
import { useSettings } from "../settings";
import { ar } from "./ar";
import { en } from "./en";
import { fr } from "./fr";

export type Language = "en" | "fr" | "ar";
export type TranslationKey = keyof typeof en;
const dictionaries = { en, fr, ar };

interface I18nValue {
  language: Language;
  setLanguage: (language: Language) => void;
  t: (key: TranslationKey, variables?: Record<string, string | number>) => string;
}

const I18nContext = createContext<I18nValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const { settings, updateSettings } = useSettings();
  const language = settings.language;

  useEffect(() => {
    document.documentElement.lang = language;
    document.documentElement.dir = language === "ar" ? "rtl" : "ltr";
  }, [language]);

  const value = useMemo<I18nValue>(() => ({
    language,
    setLanguage: (next) => updateSettings({ language: next }),
    t: (key, variables = {}) => Object.entries(variables).reduce(
      (text, [name, value]) => text.split(`{${name}}`).join(String(value)),
      dictionaries[language][key],
    ),
  }), [language, updateSettings]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n() {
  const value = useContext(I18nContext);
  if (!value) throw new Error("useI18n must be used inside I18nProvider");
  return value;
}

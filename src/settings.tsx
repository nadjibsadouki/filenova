import { invoke } from "@tauri-apps/api/core";
import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useState, type ReactNode } from "react";
import type { Language } from "./i18n";

export type Appearance = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";
const APPEARANCE_STORAGE_KEY = "filenova-appearance";

function storedAppearance(): Appearance | null {
  try {
    const value = localStorage.getItem(APPEARANCE_STORAGE_KEY);
    return value === "system" || value === "light" || value === "dark" ? value : null;
  } catch {
    return null;
  }
}

export function resolveTheme(appearance: Appearance, systemIsDark: boolean): ResolvedTheme {
  return appearance === "system" ? (systemIsDark ? "dark" : "light") : appearance;
}

export interface AppSettings {
  language: Language;
  appearance: Appearance;
  defaultOutputFolder: string | null;
  keepOriginalFiles: true;
  neverOverwriteExistingFiles: true;
  openOutputFolderAfterProcessing: boolean;
  enableProcessingHistory: boolean;
}

export const DEFAULT_SETTINGS: AppSettings = {
  language: "en",
  appearance: "system",
  defaultOutputFolder: null,
  keepOriginalFiles: true,
  neverOverwriteExistingFiles: true,
  openOutputFolderAfterProcessing: false,
  enableProcessingHistory: true,
};

interface SettingsContextValue {
  settings: AppSettings;
  loaded: boolean;
  updateSettings: (patch: Partial<AppSettings>) => void;
  saveSettings: () => Promise<void>;
  resetSettings: () => Promise<void>;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<AppSettings>(() => ({
    ...DEFAULT_SETTINGS,
    appearance: storedAppearance() ?? DEFAULT_SETTINGS.appearance,
  }));
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    void invoke<AppSettings>("load_settings")
      .then((saved) => setSettings({ ...saved, appearance: storedAppearance() ?? saved.appearance }))
      .catch(() => undefined)
      .finally(() => setLoaded(true));
  }, []);

  useLayoutEffect(() => {
    const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
    const applyTheme = () => {
      const resolvedTheme = resolveTheme(settings.appearance, systemTheme.matches);
      document.documentElement.dataset.appearance = settings.appearance;
      document.documentElement.dataset.theme = resolvedTheme;
      document.documentElement.style.colorScheme = resolvedTheme;
    };

    applyTheme();
    systemTheme.addEventListener("change", applyTheme);
    return () => systemTheme.removeEventListener("change", applyTheme);
  }, [settings.appearance]);

  useEffect(() => {
    if (!loaded) return;
    try { localStorage.setItem(APPEARANCE_STORAGE_KEY, settings.appearance); }
    catch { /* The Rust settings file remains the durable fallback. */ }
  }, [loaded, settings.appearance]);

  const value = useMemo<SettingsContextValue>(() => ({
    settings,
    loaded,
    updateSettings: (patch) => setSettings((current) => ({ ...current, ...patch })),
    saveSettings: async () => setSettings(await invoke<AppSettings>("save_settings", { settings })),
    resetSettings: async () => {
      const defaults = await invoke<AppSettings>("save_settings", { settings: DEFAULT_SETTINGS });
      setSettings(defaults);
    },
  }), [loaded, settings]);

  return <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>;
}

export function useSettings() {
  const value = useContext(SettingsContext);
  if (!value) throw new Error("useSettings must be used inside SettingsProvider");
  return value;
}

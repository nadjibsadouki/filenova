import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./App.css";
import { I18nProvider } from "./i18n";
import { SettingsProvider } from "./settings";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SettingsProvider><I18nProvider><App /></I18nProvider></SettingsProvider>
  </React.StrictMode>,
);

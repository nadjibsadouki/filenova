import { LockKeyhole } from "lucide-react";
import { isVideoToolPage, navigationItems, type PageId } from "../navigation";
import { Brand } from "./Brand";
import { useI18n } from "../i18n";

interface SidebarProps {
  activePage: PageId;
  onNavigate: (page: PageId) => void;
}

export function Sidebar({ activePage, onNavigate }: SidebarProps) {
  const { t } = useI18n();
  const workspaceItems = navigationItems.filter((item) => item.group === "workspace");
  const accountItems = navigationItems.filter((item) => item.group === "account");

  const renderItem = (item: (typeof navigationItems)[number]) => {
    const Icon = item.icon;
    const labelKeys = { home: "navHome", "compress-images": "navCompressImages", "convert-images": "navConvertImages", "resize-images": "navResizeImages", "pdf-tools": "navPdfTools", "video-tools": "videoToolsNav", history: "navHistory", settings: "navSettings" } as const;
    const label = item.id in labelKeys ? t(labelKeys[item.id as keyof typeof labelKeys]) : item.label;
    const selected = activePage === item.id || (item.id === "video-tools" && isVideoToolPage(activePage));
    return (
      <button
        className={`nav-item${selected ? " active" : ""}`}
        key={item.id}
        onClick={() => onNavigate(item.id)}
        type="button"
        aria-current={selected ? "page" : undefined}
        title={label}
      >
        <Icon size={19} strokeWidth={1.8} />
        <span>{label}</span>
      </button>
    );
  };

  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <Brand />
        <p>{t("sidebarTagline")}</p>
      </div>

      <nav className="navigation" aria-label="Main navigation">
        <div className="nav-group">{workspaceItems.map(renderItem)}</div>
        <div className="nav-divider" />
        <div className="nav-group">{accountItems.map(renderItem)}</div>
      </nav>

      <div className="privacy-badge">
        <span className="privacy-icon"><LockKeyhole size={15} /></span>
        <span><strong>{t("offlinePrivate")}</strong><small>{t("privateLabel")}</small></span>
      </div>
    </aside>
  );
}

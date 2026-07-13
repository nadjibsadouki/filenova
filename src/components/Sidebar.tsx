import { LockKeyhole } from "lucide-react";
import { navigationItems, type PageId } from "../navigation";
import { Brand } from "./Brand";

interface SidebarProps {
  activePage: PageId;
  onNavigate: (page: PageId) => void;
}

export function Sidebar({ activePage, onNavigate }: SidebarProps) {
  const workspaceItems = navigationItems.filter((item) => item.group === "workspace");
  const accountItems = navigationItems.filter((item) => item.group === "account");

  const renderItem = (item: (typeof navigationItems)[number]) => {
    const Icon = item.icon;
    return (
      <button
        className={`nav-item${activePage === item.id ? " active" : ""}`}
        key={item.id}
        onClick={() => onNavigate(item.id)}
        type="button"
        aria-current={activePage === item.id ? "page" : undefined}
        title={item.label}
      >
        <Icon size={19} strokeWidth={1.8} />
        <span>{item.label}</span>
      </button>
    );
  };

  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <Brand />
        <p>Your files. Your privacy. Your control.</p>
      </div>

      <nav className="navigation" aria-label="Main navigation">
        <div className="nav-group">{workspaceItems.map(renderItem)}</div>
        <div className="nav-divider" />
        <div className="nav-group">{accountItems.map(renderItem)}</div>
      </nav>

      <div className="privacy-badge">
        <span className="privacy-icon"><LockKeyhole size={15} /></span>
        <span><strong>100% Offline</strong><small>&amp; Private</small></span>
      </div>
    </aside>
  );
}

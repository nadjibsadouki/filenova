import { ArrowLeft, Sparkles } from "lucide-react";
import type { NavigationItem } from "../navigation";

interface PlaceholderPageProps {
  page: NavigationItem;
}

export function PlaceholderPage({ page }: PlaceholderPageProps) {
  const Icon = page.icon;

  return (
    <div className="page placeholder-page">
      <header className="page-header compact">
        <div>
          <span className="eyebrow">FILENOVA TOOL</span>
          <h1>{page.label}</h1>
          <p>A private workspace for your files, built directly into FileNova.</p>
        </div>
      </header>
      <section className="coming-soon">
        <div className="coming-icon"><Icon size={32} strokeWidth={1.6} /></div>
        <span className="coming-label"><Sparkles size={14} /> In development</span>
        <h2>{page.label}</h2>
        <p>Coming in the next development step</p>
        <div className="coming-hint"><ArrowLeft size={15} /> Select Home from the sidebar to return</div>
      </section>
    </div>
  );
}

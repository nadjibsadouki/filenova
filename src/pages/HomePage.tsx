import { useRef, type DragEvent } from "react";
import {
  ArrowRight,
  Clapperboard,
  FileArchive,
  FileText,
  FolderOpen,
  ImageDown,
  RefreshCw,
  ShieldCheck,
  UploadCloud,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { PageId } from "../navigation";

interface HomePageProps {
  onNavigate: (page: PageId) => void;
}

interface ToolCard {
  id: PageId;
  title: string;
  description: string;
  icon: LucideIcon;
  tone: string;
}

const tools: ToolCard[] = [
  { id: "compress-images", title: "Compress Images", description: "Reduce image size while preserving quality", icon: ImageDown, tone: "violet" },
  { id: "convert-images", title: "Convert Images", description: "Convert between popular image formats", icon: RefreshCw, tone: "blue" },
  { id: "pdf-tools", title: "PDF Tools", description: "Merge, split, optimize and manage PDFs", icon: FileText, tone: "rose" },
  { id: "video-tools", title: "Video Tools", description: "Optimize and convert your video files", icon: Clapperboard, tone: "amber" },
];

export function HomePage({ onNavigate }: HomePageProps) {
  const fileInput = useRef<HTMLInputElement>(null);
  const keepDropLocal = (event: DragEvent<HTMLDivElement>) => event.preventDefault();

  return (
    <div className="page home-page">
      <header className="page-header">
        <div>
          <span className="eyebrow">PRIVATE FILE WORKSPACE</span>
          <h1>Everything your files need.</h1>
          <p>Fast, private tools that work entirely on your device.</p>
        </div>
        <div className="header-status"><ShieldCheck size={17} /><span>Local processing</span></div>
      </header>

      <section
        className="drop-zone"
        onDragEnter={keepDropLocal}
        onDragOver={keepDropLocal}
        onDrop={keepDropLocal}
        aria-label="File drop zone"
      >
        <div className="drop-icon"><UploadCloud size={30} strokeWidth={1.7} /></div>
        <h2>Drop your files here</h2>
        <p>Drag and drop files to get started, or choose them manually</p>
        <input ref={fileInput} type="file" multiple hidden />
        <button className="primary-button" type="button" onClick={() => fileInput.current?.click()}>
          <FolderOpen size={17} /> Browse Files
        </button>
        <div className="local-note"><ShieldCheck size={14} />Your files are processed locally and never leave your device</div>
      </section>

      <section className="section-block">
        <div className="section-heading">
          <div><h2>Quick tools</h2><p>Choose what you want to do</p></div>
        </div>
        <div className="tool-grid">
          {tools.map((tool) => {
            const Icon = tool.icon;
            return (
              <button className="tool-card" key={tool.id} type="button" onClick={() => onNavigate(tool.id)}>
                <span className={`tool-icon ${tool.tone}`}><Icon size={22} /></span>
                <span className="tool-copy"><strong>{tool.title}</strong><small>{tool.description}</small></span>
                <ArrowRight className="tool-arrow" size={18} />
              </button>
            );
          })}
        </div>
      </section>

      <section className="section-block recent-section">
        <div className="section-heading"><div><h2>Recent Files</h2><p>Your latest processed files will appear here</p></div></div>
        <div className="empty-recent">
          <span><FileArchive size={24} /></span>
          <div><strong>No recent files</strong><p>Files you process will be listed here for quick access.</p></div>
        </div>
      </section>
    </div>
  );
}

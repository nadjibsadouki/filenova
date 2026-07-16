import { ArrowLeft, LayoutGrid } from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";
import { videoToolItems, type PageId, type VideoToolPageId } from "../navigation";

interface VideoToolNavigationProps {
  activePage: VideoToolPageId;
  onNavigate: (page: PageId) => void;
}

const shortLabels: Record<VideoToolPageId, TranslationKey> = {
  "compress-video": "videoSubnavCompress",
  "convert-video": "videoSubnavConvert",
  "extract-audio": "videoSubnavExtract",
  "trim-video": "videoSubnavTrim",
  "video-to-gif": "videoSubnavGif",
  "social-media-tools": "videoSubnavSocial",
};

export function VideoToolNavigation({ activePage, onNavigate }: VideoToolNavigationProps) {
  const { t } = useI18n();
  return <nav className="video-tool-subnav" aria-label={t("videoToolsSecondaryNavigation")}>
    <button type="button" className="video-tools-back" onClick={() => onNavigate("video-tools")}><ArrowLeft size={14}/><span>{t("backToVideoTools")}</span></button>
    <div className="video-tool-subnav-links">
      {videoToolItems.map((item) => {
        const Icon = item.icon;
        const active = item.id === activePage;
        return <button type="button" key={item.id} className={active ? "active" : ""} aria-current={active ? "page" : undefined} onClick={() => onNavigate(item.id)}><Icon size={13}/><span>{t(shortLabels[item.id])}</span></button>;
      })}
    </div>
    <LayoutGrid className="video-tool-subnav-mark" size={15}/>
  </nav>;
}

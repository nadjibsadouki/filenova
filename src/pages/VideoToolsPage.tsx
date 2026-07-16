import { ArrowRight, Clapperboard } from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";
import { videoToolItems, type PageId, type VideoToolPageId } from "../navigation";

interface VideoToolsPageProps {
  onNavigate: (page: PageId) => void;
}

const content: Record<VideoToolPageId, { title: TranslationKey; description: TranslationKey; formats: string }> = {
  "compress-video": { title: "videoHubCompressTitle", description: "videoHubCompressDescription", formats: "MP4 · AVI · MOV · MKV · WebM · M4V" },
  "convert-video": { title: "videoHubConvertTitle", description: "videoHubConvertDescription", formats: "MP4 · MOV · WebM · MKV · AVI · M4V" },
  "extract-audio": { title: "videoHubExtractTitle", description: "videoHubExtractDescription", formats: "MP3 · WAV · M4A · FLAC · OGG" },
  "trim-video": { title: "videoHubTrimTitle", description: "videoHubTrimDescription", formats: "MP4 · MOV · MKV · WebM" },
  "video-to-gif": { title: "videoHubGifTitle", description: "videoHubGifDescription", formats: "MP4 · MOV · MKV · AVI · WebM · M4V → GIF" },
  "social-media-tools": { title: "videoHubSocialTitle", description: "videoHubSocialDescription", formats: "MP4 · TikTok · Instagram · YouTube · Facebook · LinkedIn · X" },
};

export function VideoToolsPage({ onNavigate }: VideoToolsPageProps) {
  const { t } = useI18n();
  return <div className="page video-tools-hub-page">
    <header className="page-header video-tools-hub-header"><div><span className="eyebrow">{t("videoToolsHubLabel")}</span><h1>{t("videoToolsHubTitle")}</h1><p>{t("videoToolsHubSubtitle")}</p></div><div className="video-tools-hub-emblem"><Clapperboard size={27}/></div></header>
    <section className="video-tools-card-grid" aria-label={t("videoToolsHubTitle")}>
      {videoToolItems.map((item) => {
        const Icon = item.icon;
        const card = content[item.id];
        return <article className="video-tool-card" key={item.id}>
          <div className="video-tool-card-icon"><Icon size={24}/></div>
          <div className="video-tool-card-copy"><h2>{t(card.title)}</h2><p>{t(card.description)}</p></div>
          <div className="video-tool-card-formats"><small>{t("videoHubFormats")}</small><strong dir="ltr">{card.formats}</strong></div>
          <button type="button" className="primary-button video-tool-open" onClick={() => onNavigate(item.id)}>{t("openTool")}<ArrowRight size={14}/></button>
        </article>;
      })}
    </section>
  </div>;
}

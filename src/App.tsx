import { useCallback, useEffect, useRef, useState } from "react";
import { useI18n } from "./i18n";
import { Sidebar } from "./components/Sidebar";
import { VideoToolNavigation } from "./components/VideoToolNavigation";
import { isVideoToolPage, navigationItems, type PageId, type VideoToolPageId, type VideoToolRuntimeChange } from "./navigation";
import { HomePage } from "./pages/HomePage";
import { CompressImagesPage } from "./pages/CompressImagesPage";
import { ConvertImagesPage } from "./pages/ConvertImagesPage";
import { ResizeImagesPage } from "./pages/ResizeImagesPage";
import { PdfToolsPage } from "./pages/PdfToolsPage";
import { ProcessingHistoryPage } from "./pages/ProcessingHistoryPage";
import { PlaceholderPage } from "./pages/PlaceholderPage";
import { CompressVideoPage } from "./pages/CompressVideoPage";
import { ConvertVideoPage } from "./pages/ConvertVideoPage";
import { ExtractAudioPage } from "./pages/ExtractAudioPage";
import { TrimVideoPage } from "./pages/TrimVideoPage";
import { VideoToGifPage } from "./pages/VideoToGifPage";
import { SocialMediaToolsPage } from "./pages/SocialMediaToolsPage";
import { VideoToolsPage } from "./pages/VideoToolsPage";
import { SettingsPage } from "./pages/SettingsPage";

function App() {
  const { t } = useI18n();
  const [activePage, setActivePage] = useState<PageId>("home");
  const [videoWorkspaceMounted, setVideoWorkspaceMounted] = useState(false);
  const [processing, setProcessing] = useState<Partial<Record<VideoToolPageId, boolean>>>({});
  const [pendingNavigation, setPendingNavigation] = useState<{ from: VideoToolPageId; to: PageId } | null>(null);
  const cancelHandlers = useRef<Partial<Record<VideoToolPageId, () => Promise<void>>>>({});
  const currentPage = navigationItems.find((item) => item.id === activePage);

  const registerRuntime = useCallback<VideoToolRuntimeChange>((page, running, cancel) => {
    cancelHandlers.current[page] = cancel;
    setProcessing((current) => current[page] === running ? current : { ...current, [page]: running });
  }, []);

  const navigate = useCallback((page: PageId) => {
    if (page === activePage || pendingNavigation) return;
    if (page === "video-tools" || isVideoToolPage(page)) setVideoWorkspaceMounted(true);
    if (isVideoToolPage(activePage) && processing[activePage]) {
      if (!window.confirm(t("videoNavigationActiveConfirmation"))) return;
      setPendingNavigation({ from: activePage, to: page });
      void cancelHandlers.current[activePage]?.();
      return;
    }
    setActivePage(page);
  }, [activePage, pendingNavigation, processing, t]);

  useEffect(() => {
    if (!pendingNavigation || processing[pendingNavigation.from]) return;
    setActivePage(pendingNavigation.to);
    setPendingNavigation(null);
  }, [pendingNavigation, processing]);

  const videoPages: VideoToolPageId[] = ["compress-video", "convert-video", "extract-audio", "trim-video", "video-to-gif", "social-media-tools"];

  return (
    <div className="app-shell">
      <Sidebar activePage={activePage} onNavigate={navigate} />
      <main className="main-content">
        {activePage === "home" ? (
          <HomePage onNavigate={navigate} />
        ) : activePage === "compress-images" ? (
          <CompressImagesPage />
        ) : activePage === "convert-images" ? (
          <ConvertImagesPage />
        ) : activePage === "resize-images" ? (
          <ResizeImagesPage />
        ) : activePage === "pdf-tools" ? (
          <PdfToolsPage />
        ) : activePage === "history" ? (
          <ProcessingHistoryPage />
        ) : activePage === "settings" ? (
          <SettingsPage />
        ) : activePage === "video-tools" || isVideoToolPage(activePage) ? null
        : currentPage ? (
          <PlaceholderPage page={currentPage} />
        ) : (
          <HomePage onNavigate={navigate} />
        )}
        {videoWorkspaceMounted && <div className="video-tools-workspace">
          <div className="video-tool-route" hidden={activePage !== "video-tools"}><VideoToolsPage onNavigate={navigate}/></div>
          {videoPages.map((page) => <div className="video-tool-route" hidden={activePage !== page} key={page}>
            <VideoToolNavigation activePage={page} onNavigate={navigate}/>
            {page === "compress-video" ? <CompressVideoPage active={activePage === page} onRuntimeChange={registerRuntime}/>
              : page === "convert-video" ? <ConvertVideoPage active={activePage === page} onRuntimeChange={registerRuntime}/>
              : page === "extract-audio" ? <ExtractAudioPage active={activePage === page} onRuntimeChange={registerRuntime}/>
              : page === "trim-video" ? <TrimVideoPage active={activePage === page} onRuntimeChange={registerRuntime}/>
              : page === "video-to-gif" ? <VideoToGifPage active={activePage === page} onRuntimeChange={registerRuntime}/>
              : <SocialMediaToolsPage active={activePage === page} onRuntimeChange={registerRuntime}/>}</div>)}
        </div>}
      </main>
    </div>
  );
}

export default App;

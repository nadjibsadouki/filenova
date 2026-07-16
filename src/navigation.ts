import {
  Clapperboard,
  Clock3,
  FileText,
  FileAudio2,
  Images,
  House,
  ImageDown,
  RefreshCw,
  Scaling,
  Settings2,
  type LucideIcon,
} from "lucide-react";

export type PageId =
  | "home"
  | "compress-images"
  | "convert-images"
  | "resize-images"
  | "pdf-tools"
  | "video-tools"
  | "convert-video"
  | "extract-audio"
  | "trim-video"
  | "video-to-gif"
  | "history"
  | "settings";

export interface NavigationItem {
  id: PageId;
  label: string;
  icon: LucideIcon;
  group: "workspace" | "account";
}

export const navigationItems: NavigationItem[] = [
  { id: "home", label: "Home", icon: House, group: "workspace" },
  { id: "compress-images", label: "Compress Images", icon: ImageDown, group: "workspace" },
  { id: "convert-images", label: "Convert Images", icon: RefreshCw, group: "workspace" },
  { id: "resize-images", label: "Resize Images", icon: Scaling, group: "workspace" },
  { id: "pdf-tools", label: "PDF Tools", icon: FileText, group: "workspace" },
  { id: "video-tools", label: "Video Tools", icon: Clapperboard, group: "workspace" },
  { id: "convert-video", label: "Convert Video", icon: RefreshCw, group: "workspace" },
  { id: "extract-audio", label: "Extract Audio", icon: FileAudio2, group: "workspace" },
  { id: "trim-video", label: "Trim Video", icon: Clapperboard, group: "workspace" },
  { id: "video-to-gif", label: "Video to GIF", icon: Images, group: "workspace" },
  { id: "history", label: "Processing History", icon: Clock3, group: "account" },
  { id: "settings", label: "Settings", icon: Settings2, group: "account" },
];

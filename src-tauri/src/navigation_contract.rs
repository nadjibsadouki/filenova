#[cfg(test)]
mod tests {
    const VIDEO_IDS: [&str; 6] = [
        "compress-video",
        "convert-video",
        "extract-audio",
        "trim-video",
        "video-to-gif",
        "social-media-tools",
    ];

    #[test]
    fn sidebar_has_one_video_entry_and_hub_routes_each_tool_once() {
        let navigation = include_str!("../../src/navigation.ts");
        let sidebar_start = navigation.find("export const navigationItems").unwrap();
        let sidebar_end = navigation.find("export type VideoToolPageId").unwrap();
        let sidebar = &navigation[sidebar_start..sidebar_end];
        assert_eq!(sidebar.matches("\"video-tools\"").count(), 1);
        for id in VIDEO_IDS {
            assert!(!sidebar.contains(&format!("id: \"{id}\"")));
        }

        let hub = include_str!("../../src/pages/VideoToolsPage.tsx");
        assert!(hub.contains("videoToolItems.map"));
        assert!(hub.contains("onNavigate(item.id)"));
        assert!(hub.contains("openTool"));
        assert!(!hub.to_ascii_lowercase().contains("coming soon"));

        let video_items = &navigation[sidebar_end..];
        for id in VIDEO_IDS {
            assert_eq!(video_items.matches(&format!("id: \"{id}\"")).count(), 1);
        }
    }

    #[test]
    fn app_mounts_one_persistent_instance_and_guards_active_processing() {
        let app = include_str!("../../src/App.tsx");
        for component in [
            "CompressVideoPage",
            "ConvertVideoPage",
            "ExtractAudioPage",
            "TrimVideoPage",
            "VideoToGifPage",
            "SocialMediaToolsPage",
        ] {
            assert_eq!(
                app.matches(&format!("<{component} ")).count(),
                1,
                "{component}"
            );
        }
        for required in [
            "window.confirm(t(\"videoNavigationActiveConfirmation\"))",
            "cancelHandlers.current[activePage]",
            "pendingNavigation",
            "processing[pendingNavigation.from]",
            "videoWorkspaceMounted",
        ] {
            assert!(
                app.contains(required),
                "missing navigation guard: {required}"
            );
        }

        let subnav = include_str!("../../src/components/VideoToolNavigation.tsx");
        assert!(subnav.contains("onNavigate(\"video-tools\")"));
        assert!(subnav.contains("videoToolItems.map"));
        assert!(subnav.contains("aria-current={active ? \"page\" : undefined}"));
    }

    #[test]
    fn every_video_page_reports_runtime_and_only_active_page_handles_drops() {
        for (name, source, runtime_id) in [
            (
                "Compress",
                include_str!("../../src/pages/CompressVideoPage.tsx"),
                "compress-video",
            ),
            (
                "Convert",
                include_str!("../../src/pages/ConvertVideoPage.tsx"),
                "convert-video",
            ),
            (
                "Extract",
                include_str!("../../src/pages/ExtractAudioPage.tsx"),
                "extract-audio",
            ),
            (
                "Trim",
                include_str!("../../src/pages/TrimVideoPage.tsx"),
                "trim-video",
            ),
            (
                "GIF",
                include_str!("../../src/pages/VideoToGifPage.tsx"),
                "video-to-gif",
            ),
            (
                "Social",
                include_str!("../../src/pages/SocialMediaToolsPage.tsx"),
                "social-media-tools",
            ),
        ] {
            assert!(source.contains("if (!active) return;"), "{name} drop guard");
            assert!(
                source.contains(&format!("onRuntimeChange?.(\"{runtime_id}\"")),
                "{name} runtime"
            );
        }
    }
}

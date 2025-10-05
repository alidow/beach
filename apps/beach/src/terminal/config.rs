pub fn cursor_sync_enabled() -> bool {
    std::env::var("BEACH_CURSOR_SYNC")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(true)
}

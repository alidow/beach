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

#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct KeyConfig {
    #[serde(default)]
    pub scroll_toggle: Option<Vec<String>>,
    #[serde(default)]
    pub copy_shortcuts: Option<Vec<String>>,
    #[serde(default)]
    pub double_esc: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct ClientConfig {
    #[serde(default)]
    pub keys: Option<KeyConfig>,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct UserConfig {
    #[serde(default)]
    pub client: Option<ClientConfig>,
}

/// Load user configuration from ~/.beach/config (TOML)
/// Returns None if the file is missing or invalid.
pub fn load_user_config() -> Option<UserConfig> {
    use std::fs;
    let base = directories::BaseDirs::new()?;
    let path = base.home_dir().join(".beach").join("config");
    let raw = fs::read_to_string(path).ok()?;
    toml::from_str::<UserConfig>(&raw).ok()
}

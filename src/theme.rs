use napi_derive::napi;

/// Theme configuration.
///
/// In theme mode this is the `themeConfigObject` the theme package
/// exports as its default export (Jekyll-gem style: the package itself
/// is the source of truth, `bagan-theme.json` is no longer read). In
/// local mode (no `theme` configured) the values come from the cache
/// configuration instead.
#[napi(object)]
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaganThemeConfig {
    /// Theme package name.
    #[serde(default)]
    pub name: Option<String>,
    /// Theme package version.
    #[serde(default)]
    pub version: Option<String>,
    /// Optional theme author.
    #[serde(default)]
    pub author: Option<String>,
    /// Relative path to the theme's HTML layout templates (default
    /// `"_layouts"`), resolved against the theme root.
    #[serde(default, alias = "layouts_dir")]
    pub layouts_dir: Option<String>,
    /// Relative path to the theme's include templates (default
    /// `"_includes"`), resolved against the theme root.
    #[serde(default, alias = "includes_dir")]
    pub includes_dir: Option<String>,
    /// Relative path to the theme's static assets (default `"_assets"`),
    /// resolved against the theme root.
    #[serde(default, alias = "assets_dir")]
    pub assets_dir: Option<String>,
    /// Relative path to the theme's data files (default `"_data"`),
    /// resolved against the theme root.
    #[serde(default, alias = "data_dir")]
    pub data_dir: Option<String>,
}

/// Default layouts directory name.
pub const DEFAULT_LAYOUTS_DIR: &str = "_layouts";
/// Default includes directory name.
pub const DEFAULT_INCLUDES_DIR: &str = "_includes";
/// Default theme assets directory name.
pub const DEFAULT_ASSETS_DIR: &str = "_assets";
/// Default data directory name.
pub const DEFAULT_DATA_DIR: &str = "_data";

impl BaganThemeConfig {
    /// Layouts directory name, or the default `"_layouts"`.
    pub fn layouts_dir_or_default(&self) -> &str {
        self.layouts_dir.as_deref().unwrap_or(DEFAULT_LAYOUTS_DIR)
    }

    /// Includes directory name, or the default `"_includes"`.
    pub fn includes_dir_or_default(&self) -> &str {
        self.includes_dir.as_deref().unwrap_or(DEFAULT_INCLUDES_DIR)
    }

    /// Theme assets directory name, or the default `"_assets"`.
    pub fn assets_dir_or_default(&self) -> &str {
        self.assets_dir.as_deref().unwrap_or(DEFAULT_ASSETS_DIR)
    }

    /// Data directory name, or the default `"_data"`.
    pub fn data_dir_or_default(&self) -> &str {
        self.data_dir.as_deref().unwrap_or(DEFAULT_DATA_DIR)
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default)]
/// Resolved theme directories, as absolute paths.
pub struct ThemeDirs {
    /// Directory containing the theme's HTML layout templates.
    pub layouts_dir: String,
    /// Directory containing the theme's static assets.
    pub assets_dir: String,
    /// Directory containing the theme's data files.
    pub data_dir: String,
    /// Directory containing the theme's HTML includes files.
    pub includes_dir: String,
}

use napi_derive::napi;
use serde::{Deserialize, Serialize};

/// Static site hosting providers with known `baseUrl` behavior.
#[napi(string_enum, skip_typescript)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostingProvider {
    /// GitHub Pages. User/org sites (`<user>.github.io`) serve at `/`;
    /// project sites at `/<repo>`. Pass the repository name via
    /// `repo` to get the `/<repo>/` prefix.
    GithubPages,
    /// Vercel: always `/` (subpath deployments use rewrites, not paths).
    Vercel,
    /// Netlify: always `/`.
    Netlify,
    /// Cloudflare Pages: always `/`.
    CloudflarePages,
    /// Self-hosted or custom setup: use the `site_base_url` string as given.
    Custom,
}

/// Resolved hosting configuration: the single source of truth for every
/// URL the pipeline emits (page slugs, asset links, absolute URLs).
#[napi(object)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseDirInfo {
    /// Hosting provider in effect.
    #[napi(ts_type = "BaganHostingProvider")]
    pub provider: HostingProvider,
    /// Repository name (GitHub Pages project sites): `/<repo>/` prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Custom site URL (`https://example.com` or `https://user.github.io/repo`).
    /// Used verbatim as the URL root for [`Self::site_url`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
    /// Extra path prefix under the host (e.g. `subpath` for
    /// `example.com/subpath/`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
}

/// Default page-served path prefix by provider (used when no explicit
/// path_prefix/site URL is configured).
impl HostingProvider {
    /// Human-readable identifier (used in error messages and logs).
    pub fn id(&self) -> &'static str {
        match self {
            HostingProvider::GithubPages => "github-pages",
            HostingProvider::Vercel => "vercel",
            HostingProvider::Netlify => "netlify",
            HostingProvider::CloudflarePages => "cloudflare-pages",
            HostingProvider::Custom => "custom",
        }
    }
}
impl BaseDirInfo {
    /// Create hosting info for a provider, with an optional GitHub repo
    /// name and a custom site URL.
    pub fn new(provider: HostingProvider, repo: Option<String>, site_url: Option<String>) -> Self {
        Self {
            provider,
            repo,
            site_url,
            path_prefix: None,
        }
    }

    /// The path prefix every internal link/asset must be prefixed with,
    /// always starting with `/` and ending with `/`. Examples: `/`,
    /// `/repo/`, `/subpath/`.
    pub fn base_url(&self) -> String {
        // Provider-derived prefix: GitHub Pages project sites live at
        // /<repo>; user sites (<user>.github.io) and the platform hosts
        // live at the domain root.
        let mut segments: Vec<String> = Vec::new();
        if self.provider == HostingProvider::GithubPages
            && let Some(repo) = &self.repo
        {
            let repo = repo.trim_matches('/');
            // A repo named "<user>.github.io" IS the user site —
            // it serves at the root, not under /<user>.github.io/.
            if !repo.is_empty() && !repo.ends_with(".github.io") {
                segments.push(repo.to_string());
            }
        }
        // Explicit path prefix always wins (subpath deployments).
        if let Some(prefix) = &self.path_prefix {
            for seg in prefix.trim_matches('/').split('/') {
                if !seg.is_empty() && !segments.iter().any(|s| s == seg) {
                    segments.push(seg.to_string());
                }
            }
        }
        if segments.is_empty() {
            "/".to_string()
        } else {
            format!("/{}/", segments.join("/"))
        }
    }

    /// Absolute origin URL for canonical URLs / feeds. When `site_url` is
    /// set it is returned normalized (trailing slash stripped);
    /// GitHub Pages derives `https://user.github.io[/repo]` from the repo
    /// name when it looks like `<user>.github.io`.
    pub fn site_url(&self) -> Option<String> {
        if let Some(url) = &self.site_url {
            return Some(url.trim_end_matches('/').to_string());
        }
        match &self.provider {
            HostingProvider::GithubPages => match &self.repo {
                Some(r) if r.ends_with(".github.io") => {
                    Some(format!("https://{}", r.trim_end_matches('/')))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Join `path` (like `docs/intro`) with the [`Self::base_url`] prefix
    /// into an absolute site URL: `https://example.com/repo/docs/intro`.
    pub fn absolute_url(&self, path: &str) -> String {
        let origin = self.site_url().unwrap_or_default();
        let base = self.base_url();
        let path = path.trim_start_matches('/');
        format!("{origin}{base}{path}")
    }

    /// Detect the provider from a URL, best-effort (used when the consumer
    /// only knows a deployment URL):
    ///
    /// - `*.github.io` → `GithubPages` (with the repo extracted)
    /// - `*.vercel.app` → `Vercel`
    /// - `*.netlify.app` → `Netlify`
    /// - `*.pages.dev` → `CloudflarePages`
    /// - anything else → `Custom`
    pub fn detect(url: &str) -> Self {
        let lower = url.to_lowercase();
        if lower.contains("github.io") {
            // https://user.github.io/repo/ → repo is the first path segment.
            let path = url
                .split("github.io")
                .nth(1)
                .unwrap_or("")
                .trim_matches('/');
            let repo = path.split('/').next().filter(|s| !s.is_empty());
            return Self::new(HostingProvider::GithubPages, repo.map(str::to_string), None);
        }
        let provider = if lower.contains("vercel.app") {
            HostingProvider::Vercel
        } else if lower.contains("netlify.app") {
            HostingProvider::Netlify
        } else if lower.contains("pages.dev") {
            HostingProvider::CloudflarePages
        } else {
            HostingProvider::Custom
        };
        Self::new(provider, None, Some(url.to_string()))
    }
}

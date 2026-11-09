//! File discovery for the cache module: the extension allow-lists and the
//! recursive directory walks that feed
//! [`PeisarCache`](super::PeisarCache).

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// File extensions recognized as Markdown, mirroring the
/// [`markdown-extensions`](https://github.com/sindresorhus/markdown-extensions)
/// list. Files with one of these extensions are cached as UTF-8 text.
pub const MARKDOWN_EXTENSIONS: &[&str] = &[
    "md", "markdown", "mdown", "mkdn", "mkd", "mdwn", "mkdown", "ron",
];

/// Asset extensions discovered under the assets directory: images,
/// scripts, styles, fonts, and audio/video files.
pub const ASSET_EXTENSIONS: &[&str] = &[
    // images
    "png", "jpg", "jpeg", "gif", "svg", "webp", "avif", "ico", "bmp", "tiff",
    // scripts & styles
    "js", "css", "map", // fonts
    "woff", "woff2", "ttf", "eot", // audio/video
    "mp3", "wav", "ogg", "mp4", "webm",
];

/// File extensions recognized as Liquid templates. Files with one of
/// these extensions are cached as UTF-8 text and — when the consumer
/// passes a `liquidRender` callback — rendered to HTML on the JS side.
/// (`liquid.html` is matched through the trailing `html` extension.)
pub const LIQUID_EXTENSIONS: &[&str] = &["html", "liquid"];

/// Subset of [`ASSET_EXTENSIONS`] considered binary: such files are cached
/// as raw bytes and never read as UTF-8. Asset extensions not listed here
/// (`js`, `css`, `map`, `svg`) are cached as text.
pub const ASSET_BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "bmp", "tiff", "woff", "woff2", "ttf",
    "eot", "mp3", "wav", "ogg", "mp4", "webm",
];

/// Recursively collect every Markdown file under `entry_dir`.
///
/// Relative directories are resolved against the current working
/// directory. Returns absolute paths; a missing or non-directory path
/// yields an empty vector.
pub fn collect_markdown_files<P: AsRef<Path>>(entry_dir: P) -> io::Result<Vec<PathBuf>> {
    let current_dir = env::current_dir().unwrap_or(PathBuf::from("."));
    let dir_path = current_dir.join(entry_dir.as_ref());
    let mut markdown_files: Vec<PathBuf> = Vec::new();

    if dir_path.is_dir() {
        for entry in fs::read_dir(&dir_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Recurse into subdirectory; PathBuf implements AsRef<Path>
                let nested = collect_markdown_files(path)?;
                markdown_files.extend(nested);
            } else if path.is_file()
                && let Some(ext) = path.extension().and_then(|e| e.to_str())
                && MARKDOWN_EXTENSIONS.contains(&ext)
            {
                markdown_files.push(path);
            }
        }
    }

    Ok(markdown_files)
}

/// Recursively collect every asset file under `entry_dir` (see
/// [`ASSET_EXTENSIONS`]).
///
/// Relative directories are resolved against the current working
/// directory. Returns absolute paths; a missing or non-directory path
/// yields an empty vector.
pub fn collect_asset_files<P: AsRef<Path>>(entry_dir: P) -> io::Result<Vec<PathBuf>> {
    let current_dir = env::current_dir().unwrap_or(PathBuf::from("."));
    let dir_path = current_dir.join(entry_dir.as_ref());
    let mut asset_files: Vec<PathBuf> = Vec::new();

    if dir_path.is_dir() {
        for entry in fs::read_dir(&dir_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Recurse
                let nested = collect_asset_files(path)?;
                asset_files.extend(nested);
            } else if path.is_file()
                && let Some(ext) = path.extension().and_then(|e| e.to_str())
                && ASSET_EXTENSIONS.contains(&ext)
            {
                asset_files.push(path);
            }
        }
    }

    Ok(asset_files)
}

/// `true` when `path` has a Liquid extension (`html`, `liquid`,
/// `liquid.html`, case-insensitive).
pub fn has_liquid_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext_l = ext.to_lowercase();
    LIQUID_EXTENSIONS.contains(&ext_l.as_str())
        || path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.to_lowercase().ends_with(".liquid.html"))
}

/// Recursively collect every Liquid file (`.html`/`.liquid`) under
/// `dir` (see [`LIQUID_EXTENSIONS`]).
///
/// Returns absolute paths; a missing or non-directory path yields an
/// empty vector. Directories are flattened, so collections configured
/// as directories expand to their files and merge with files configured
/// directly.
pub fn collect_liquid_files<P: AsRef<Path>>(dir: P) -> Vec<PathBuf> {
    let mut liquid_files: Vec<PathBuf> = Vec::new();
    let dir_path = dir.as_ref();

    if dir_path.is_dir()
        && let Ok(entries) = fs::read_dir(dir_path)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let nested = collect_liquid_files(path);
                liquid_files.extend(nested);
            } else if path.is_file() && has_liquid_extension(&path) {
                liquid_files.push(path);
            }
        }
    }

    liquid_files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liquid_extension_detection() {
        assert!(has_liquid_extension(Path::new("a/page.html")));
        assert!(has_liquid_extension(Path::new("a/page.HTML")));
        assert!(has_liquid_extension(Path::new("a/page.liquid")));
        assert!(has_liquid_extension(Path::new("a/page.LIQUID.html")));
        assert!(has_liquid_extension(Path::new("a/page.liquid.html")));
        assert!(!has_liquid_extension(Path::new("a/page.md")));
        assert!(!has_liquid_extension(Path::new("a/page.css")));
        assert!(!has_liquid_extension(Path::new("a/.html.swp")));
    }

    #[test]
    fn collect_liquid_files_flattens_recursively() {
        let tmp = env::temp_dir().join(format!(
            "bagan_liquid_{:?}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
        ));
        fs::create_dir_all(tmp.join("nested/deep")).unwrap();
        fs::write(tmp.join("top.html"), "<p>top</p>").unwrap();
        fs::write(tmp.join("nested/mid.liquid"), "<p>mid</p>").unwrap();
        fs::write(tmp.join("nested/deep/bottom.liquid.html"), "<p>b</p>").unwrap();
        fs::write(tmp.join("nested/deep/ignored.txt"), "plain").unwrap();

        let files = collect_liquid_files(&tmp);
        assert_eq!(files.len(), 3);
        assert!(files.iter().any(|p| p.ends_with("top.html")));
        assert!(files.iter().any(|p| p.ends_with("mid.liquid")));
        assert!(files.iter().any(|p| p.ends_with("bottom.liquid.html")));
        assert!(!files.iter().any(|p| p.ends_with("ignored.txt")));

        // Missing directories yield an empty vector.
        assert!(collect_liquid_files(tmp.join("missing")).is_empty());

        fs::remove_dir_all(&tmp).unwrap();
    }
}

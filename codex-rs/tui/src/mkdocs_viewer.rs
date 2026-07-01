use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

const MKDOCS_CONFIG_NAMES: [&str; 2] = ["mkdocs.yml", "mkdocs.yaml"];
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_PAGE_BYTES: u64 = 128 * 1024;
const MAX_DISCOVERY_ANCESTORS: usize = 16;
const MAX_DISCOVERY_DEPTH: usize = 8;
const MAX_PAGES: usize = 200;
const MAX_DIR_ENTRIES: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MkDocsSite {
    pub(crate) title: String,
    pub(crate) project_root: PathBuf,
    pub(crate) config_path: PathBuf,
    pub(crate) docs_dir: PathBuf,
    pub(crate) pages: Vec<MkDocsPage>,
    pub(crate) selected_index: usize,
}

impl MkDocsSite {
    pub(crate) fn overlay_title(&self) -> String {
        format!("D O C S  {}", self.title)
    }

    pub(crate) fn read_page_source(&self, page_index: usize) -> Result<String, MkDocsViewerError> {
        let page = self.pages.get(page_index).ok_or_else(|| {
            MkDocsViewerError::new(format!("No MkDocs page at index {page_index}."))
        })?;
        read_limited_to_string(&page.abs_path, MAX_PAGE_BYTES).map_err(|err| {
            MkDocsViewerError::new(format!(
                "Failed to read MkDocs page {}: {err}",
                page.abs_path.display()
            ))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MkDocsPage {
    pub(crate) rel_path: PathBuf,
    pub(crate) abs_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MkDocsConfig {
    path: PathBuf,
    project_root: PathBuf,
    site_name: String,
    docs_dir: PathBuf,
    nav_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MkDocsViewerError(String);

impl MkDocsViewerError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for MkDocsViewerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for MkDocsViewerError {}

pub(crate) fn load_mkdocs_site(
    cwd: &Path,
    args: Option<&str>,
) -> Result<MkDocsSite, MkDocsViewerError> {
    let (config, page_hint) = resolve_mkdocs_request(cwd, args)?;
    let mut pages = discover_pages(&config.docs_dir)?;
    sort_pages(&mut pages, &config.nav_paths);
    if pages.is_empty() {
        return Err(MkDocsViewerError::new(format!(
            "No markdown pages found under {}.",
            config.docs_dir.display()
        )));
    }

    let selected_index = resolve_page_index(&pages, page_hint.as_deref()).ok_or_else(|| {
        let target = page_hint.as_deref().unwrap_or("index.md");
        MkDocsViewerError::new(format!(
            "No MkDocs page matched `{target}` under {}.",
            config.docs_dir.display()
        ))
    })?;

    Ok(MkDocsSite {
        title: config.site_name,
        project_root: config.project_root,
        config_path: config.path,
        docs_dir: config.docs_dir,
        pages,
        selected_index,
    })
}

fn resolve_mkdocs_request(
    cwd: &Path,
    args: Option<&str>,
) -> Result<(MkDocsConfig, Option<String>), MkDocsViewerError> {
    let trimmed = args.map(str::trim).filter(|value| !value.is_empty());
    let Some(args) = trimmed else {
        let config_path = find_mkdocs_config(cwd).ok_or_else(|| {
            MkDocsViewerError::new(format!(
                "No mkdocs.yml found from {} upward.",
                cwd.display()
            ))
        })?;
        return parse_mkdocs_config(&config_path).map(|config| (config, None));
    };

    if let Some(rest) = args.strip_prefix("--config ") {
        let (path, page_hint) = split_path_arg(rest)?;
        return parse_mkdocs_config(&resolve_path(cwd, &path)).map(|config| (config, page_hint));
    }

    if let Some(rest) = args.strip_prefix("--docs-dir ") {
        let (path, page_hint) = split_path_arg(rest)?;
        return synthetic_docs_dir_config(&resolve_path(cwd, &path))
            .map(|config| (config, page_hint));
    }

    let (first_arg, remaining) = split_path_arg(args)?;
    let first_path = resolve_path(cwd, &first_arg);
    if first_path.is_dir() {
        if let Some(config_path) = find_mkdocs_config(&first_path) {
            return parse_mkdocs_config(&config_path).map(|config| (config, remaining));
        }
        return synthetic_docs_dir_config(&first_path).map(|config| (config, remaining));
    }

    if first_path.is_file() && is_mkdocs_config_path(&first_path) {
        return parse_mkdocs_config(&first_path).map(|config| (config, remaining));
    }

    if first_path.is_file()
        && first_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        && let Some(config_path) = first_path.parent().and_then(find_mkdocs_config)
    {
        let config = parse_mkdocs_config(&config_path)?;
        let page_hint = first_path
            .strip_prefix(&config.docs_dir)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| first_arg.clone());
        return Ok((config, Some(page_hint)));
    }

    let config_path = find_mkdocs_config(cwd).ok_or_else(|| {
        MkDocsViewerError::new(format!(
            "No mkdocs.yml found from {} upward.",
            cwd.display()
        ))
    })?;
    parse_mkdocs_config(&config_path).map(|config| (config, Some(args.to_string())))
}

fn split_path_arg(args: &str) -> Result<(String, Option<String>), MkDocsViewerError> {
    let parts = shlex::split(args).ok_or_else(|| {
        MkDocsViewerError::new(format!("Failed to parse /docs arguments: {args}"))
    })?;
    let Some(first) = parts.first() else {
        return Err(MkDocsViewerError::new(
            "Expected a path after /docs option.",
        ));
    };
    let remaining = (!parts[1..].is_empty()).then(|| parts[1..].join(" "));
    Ok((first.clone(), remaining))
}

fn resolve_path(cwd: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn is_mkdocs_config_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| MKDOCS_CONFIG_NAMES.contains(&name))
}

fn find_mkdocs_config(cwd: &Path) -> Option<PathBuf> {
    let start = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    for dir in start.ancestors().take(MAX_DISCOVERY_ANCESTORS) {
        for name in MKDOCS_CONFIG_NAMES {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn parse_mkdocs_config(config_path: &Path) -> Result<MkDocsConfig, MkDocsViewerError> {
    let source = read_limited_to_string(config_path, MAX_CONFIG_BYTES).map_err(|err| {
        MkDocsViewerError::new(format!(
            "Failed to read MkDocs config {}: {err}",
            config_path.display()
        ))
    })?;
    let project_root = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let site_name = scalar_config_value(&source, "site_name")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            project_root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("MkDocs")
                .to_string()
        });
    let docs_dir_value = scalar_config_value(&source, "docs_dir").unwrap_or_else(|| "docs".into());
    let docs_dir = normalize_config_relative_path(&project_root, &docs_dir_value);

    if !docs_dir.is_dir() {
        return Err(MkDocsViewerError::new(format!(
            "MkDocs docs_dir does not exist: {}",
            docs_dir.display()
        )));
    }

    if !path_stays_under(&docs_dir, &project_root) {
        return Err(MkDocsViewerError::new(format!(
            "MkDocs docs_dir must stay inside the project root: {}",
            docs_dir.display()
        )));
    }

    Ok(MkDocsConfig {
        path: config_path.to_path_buf(),
        project_root,
        site_name,
        docs_dir,
        nav_paths: nav_paths(&source),
    })
}

fn synthetic_docs_dir_config(docs_dir: &Path) -> Result<MkDocsConfig, MkDocsViewerError> {
    if !docs_dir.is_dir() {
        return Err(MkDocsViewerError::new(format!(
            "MkDocs docs directory does not exist: {}",
            docs_dir.display()
        )));
    }
    let docs_dir = docs_dir
        .canonicalize()
        .unwrap_or_else(|_| docs_dir.to_path_buf());
    let project_root = docs_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| docs_dir.clone());
    let site_name = project_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("MkDocs")
        .to_string();
    Ok(MkDocsConfig {
        path: docs_dir.join("(explicit docs dir)"),
        project_root,
        site_name,
        docs_dir,
        nav_paths: Vec::new(),
    })
}

fn scalar_config_value(source: &str, key: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let without_comment = line.split_once('#').map_or(line, |(value, _)| value);
        let trimmed = without_comment.trim();
        if trimmed.starts_with('-') {
            return None;
        }
        let (candidate, value) = trimmed.split_once(':')?;
        (candidate.trim() == key).then(|| clean_yaml_scalar(value.trim()))
    })
}

fn clean_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    let quoted = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        });
    quoted.unwrap_or(trimmed).trim().to_string()
}

fn normalize_config_relative_path(project_root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    }
}

fn path_stays_under(path: &Path, root: &Path) -> bool {
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    canonical_path.starts_with(canonical_root)
}

fn nav_paths(source: &str) -> Vec<String> {
    let mut in_nav = false;
    let mut paths = Vec::new();
    for line in source.lines() {
        let indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
        let trimmed = line.trim();
        if trimmed == "nav:" {
            in_nav = true;
            continue;
        }
        if in_nav && indent == 0 && !trimmed.starts_with('-') && trimmed.contains(':') {
            break;
        }
        if in_nav && let Some(path) = nav_path_from_line(trimmed) {
            paths.push(path);
        }
    }
    paths
}

fn nav_path_from_line(trimmed: &str) -> Option<String> {
    let value = trimmed.strip_prefix('-')?.trim();
    let candidate = value
        .rsplit_once(':')
        .map_or(value, |(_, path)| path)
        .trim();
    let candidate = clean_yaml_scalar(candidate);
    candidate.ends_with(".md").then_some(candidate)
}

fn discover_pages(docs_dir: &Path) -> Result<Vec<MkDocsPage>, MkDocsViewerError> {
    let mut pages = Vec::new();
    let mut visited_entries = 0usize;
    discover_pages_inner(
        docs_dir,
        docs_dir,
        /*depth*/ 0,
        &mut visited_entries,
        &mut pages,
    )?;
    pages.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(pages)
}

fn discover_pages_inner(
    root: &Path,
    dir: &Path,
    depth: usize,
    visited_entries: &mut usize,
    pages: &mut Vec<MkDocsPage>,
) -> Result<(), MkDocsViewerError> {
    if depth > MAX_DISCOVERY_DEPTH || pages.len() >= MAX_PAGES {
        return Ok(());
    }

    let entries = fs::read_dir(dir).map_err(|err| {
        MkDocsViewerError::new(format!(
            "Failed to read MkDocs docs directory {}: {err}",
            dir.display()
        ))
    })?;

    for entry in entries {
        *visited_entries += 1;
        if *visited_entries > MAX_DIR_ENTRIES || pages.len() >= MAX_PAGES {
            break;
        }
        let entry = entry.map_err(|err| {
            MkDocsViewerError::new(format!(
                "Failed to inspect MkDocs docs directory {}: {err}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|err| {
            MkDocsViewerError::new(format!("Failed to inspect {}: {err}", path.display()))
        })?;
        if file_type.is_dir() {
            discover_pages_inner(root, &path, depth + 1, visited_entries, pages)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            let rel_path = path
                .strip_prefix(root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| path.clone());
            pages.push(MkDocsPage {
                rel_path,
                abs_path: path,
            });
        }
    }
    Ok(())
}

fn sort_pages(pages: &mut Vec<MkDocsPage>, nav_paths: &[String]) {
    if nav_paths.is_empty() {
        return;
    }

    let mut seen = HashSet::new();
    let mut ordered = Vec::with_capacity(pages.len());
    for nav_path in nav_paths {
        if let Some(index) = pages
            .iter()
            .position(|page| paths_equal_str(&page.rel_path, nav_path))
        {
            let page = pages.remove(index);
            seen.insert(page.rel_path.clone());
            ordered.push(page);
        }
    }
    ordered.extend(
        pages
            .drain(..)
            .filter(|page| !seen.contains(&page.rel_path)),
    );
    *pages = ordered;
}

fn resolve_page_index(pages: &[MkDocsPage], page_hint: Option<&str>) -> Option<usize> {
    let hint = page_hint.map(str::trim).filter(|hint| !hint.is_empty());
    match hint {
        Some(hint) => {
            let hint_path = Path::new(hint);
            pages
                .iter()
                .position(|page| {
                    page.abs_path == hint_path
                        || paths_equal(&page.rel_path, hint_path)
                        || page.rel_path.ends_with(hint_path)
                })
                .or_else(|| {
                    pages.iter().position(|page| {
                        page.rel_path
                            .to_string_lossy()
                            .to_ascii_lowercase()
                            .contains(&hint.to_ascii_lowercase())
                    })
                })
        }
        None => pages
            .iter()
            .position(|page| paths_equal_str(&page.rel_path, "index.md"))
            .or(Some(0)),
    }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left.components().eq(right.components())
}

fn paths_equal_str(left: &Path, right: &str) -> bool {
    paths_equal(left, Path::new(right))
}

fn read_limited_to_string(path: &Path, max_bytes: u64) -> io::Result<String> {
    let metadata = fs::metadata(path)?;
    let mut source = fs::read_to_string(path)?;
    if metadata.len() > max_bytes {
        let max_bytes = max_bytes as usize;
        let cutoff = source
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= max_bytes)
            .last()
            .unwrap_or(/*default*/ 0);
        source.truncate(cutoff);
        source.push_str("\n\n<!-- truncated by /docs -->\n");
    }
    Ok(source)
}

#[cfg(test)]
#[path = "mkdocs_viewer_tests.rs"]
mod tests;

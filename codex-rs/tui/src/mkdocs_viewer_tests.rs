use super::*;
use pretty_assertions::assert_eq;

#[test]
fn loads_index_from_parent_mkdocs_project() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    fs::create_dir_all(root.join("docs/guide")).expect("docs dirs");
    fs::create_dir_all(root.join("src/deep")).expect("cwd dirs");
    fs::write(
        root.join("mkdocs.yml"),
        r#"
site_name: Terminal Docs
docs_dir: docs
nav:
  - Home: index.md
  - Install: guide/install.md
"#,
    )
    .expect("mkdocs config");
    fs::write(root.join("docs/index.md"), "# Home\n\nWelcome.").expect("index");
    fs::write(root.join("docs/guide/install.md"), "# Install\n\nRun it.").expect("install");

    let site = load_mkdocs_site(&root.join("src/deep"), /*args*/ None).expect("site");

    assert_eq!(site.title, "Terminal Docs");
    assert_eq!(site.project_root, root);
    assert_eq!(site.docs_dir, root.join("docs"));
    assert_eq!(
        site.pages[site.selected_index].abs_path,
        root.join("docs/index.md")
    );
    assert!(
        site.pages
            .iter()
            .position(|page| page.rel_path == Path::new("index.md"))
            .expect("index listed")
            < site
                .pages
                .iter()
                .position(|page| page.rel_path == Path::new("guide/install.md"))
                .expect("install listed")
    );
    assert_eq!(
        site.read_page_source(site.selected_index).expect("source"),
        "# Home\n\nWelcome."
    );
}

#[test]
fn resolves_page_hint_by_suffix() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    fs::create_dir_all(root.join("docs/reference")).expect("docs dirs");
    fs::write(root.join("mkdocs.yml"), "site_name: Docs\n").expect("mkdocs config");
    fs::write(root.join("docs/index.md"), "# Home").expect("index");
    fs::write(root.join("docs/reference/api.md"), "# API").expect("api");

    let site = load_mkdocs_site(root, Some("api.md")).expect("site");

    assert_eq!(
        site.pages[site.selected_index].abs_path,
        root.join("docs/reference/api.md")
    );
    assert_eq!(
        site.pages[site.selected_index].rel_path,
        Path::new("reference/api.md")
    );
    assert_eq!(
        site.read_page_source(site.selected_index).expect("source"),
        "# API"
    );
}

#[test]
fn can_open_explicit_repo_path_and_page_hint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(repo.join("docs/reference")).expect("docs dirs");
    fs::write(repo.join("mkdocs.yml"), "site_name: Repo Docs\n").expect("mkdocs config");
    fs::write(repo.join("docs/index.md"), "# Home").expect("index");
    fs::write(repo.join("docs/reference/exec.md"), "# Exec").expect("exec");

    let args = format!("{} exec.md", repo.display());
    let site = load_mkdocs_site(&workspace, Some(&args)).expect("site");

    assert_eq!(site.title, "Repo Docs");
    assert_eq!(site.project_root, repo);
    assert_eq!(
        site.pages[site.selected_index].rel_path,
        Path::new("reference/exec.md")
    );
}

#[test]
fn can_open_explicit_docs_dir_without_mkdocs_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let docs = temp.path().join("loose-docs");
    fs::create_dir_all(&docs).expect("docs dir");
    fs::write(docs.join("index.md"), "# Loose").expect("index");
    fs::write(docs.join("exec.md"), "# Exec").expect("exec");

    let args = format!("--docs-dir {} exec.md", docs.display());
    let site = load_mkdocs_site(temp.path(), Some(&args)).expect("site");

    assert_eq!(site.docs_dir, docs);
    assert_eq!(
        site.pages[site.selected_index].rel_path,
        Path::new("exec.md")
    );
}

#[test]
fn rejects_docs_dir_outside_project_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("project");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&root).expect("root");
    fs::create_dir_all(&outside).expect("outside");
    fs::write(root.join("mkdocs.yml"), "docs_dir: ../outside\n").expect("mkdocs config");

    let error = load_mkdocs_site(&root, /*args*/ None).expect_err("error");

    assert!(
        error
            .to_string()
            .contains("docs_dir must stay inside the project root")
    );
}

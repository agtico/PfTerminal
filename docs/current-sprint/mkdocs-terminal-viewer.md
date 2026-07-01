# MkDocs Terminal Viewer

Status: Milestone 1 source navigator implemented; live reload, search,
agent-facing docs APIs, and optional `mkdocs serve` remain planned.

## Problem Statement

PFTerminal repos increasingly use MkDocs as the standard project documentation
surface, but the current review loop is browser-shaped:

1. edit Markdown in a repo;
2. run `mkdocs serve` or build a static site;
3. expose the rendered docs through a browser, Docker, or another local/public
   web surface;
4. ask an agent to work against docs it cannot directly open, scroll, or
   navigate in the user's terminal pane.

That breaks the terminal-native workflow. The user wants a tmux pane that acts
like a scrollable MkDocs reader: live-updating, navigable, agent-addressable,
and safe to run without exposing a site.

## Product Goal

Add a terminal-native MkDocs viewer to PFTerminal. From inside PFTerminal:

```text
/docs
```

The implemented source navigator discovers the current MkDocs project, displays
a page index beside the rendered Markdown page, supports direct page hints, and
supports explicit config/docs-dir selection. The longer-term viewer should add
live reload and expose an agent-control API so agents can open docs pages or
search docs without shelling out to broad filesystem commands.

## Non-Goals

- Do not build a full browser.
- Do not require Docker, public hosting, SSH forwarding, or an exposed web
  server for normal use.
- Do not attempt full `mkdocs-material` browser parity in the MVP.
- Do not execute arbitrary JavaScript from generated docs.
- Do not make broad recursive searches across `$HOME` part of the docs
  workflow.

## User Workflows

### 1. Human opens docs beside an editor

```bash
tmux new -s docs-work
cd ~/repos/PfTerminal
pfterminal docs
```

Expected result:

- left panel shows MkDocs navigation;
- right panel shows the rendered current page;
- edits to `docs/*.md` refresh the page automatically;
- scroll position is preserved when possible;
- parse/build errors show in a status panel without killing the viewer.

### 2. Agent opens the right docs page

User says:

```text
Let's work on provider auth docs.
```

Expected result:

- the agent searches only the MkDocs docs tree using the docs viewer API;
- the viewer opens the best matching page;
- PFTerminal switches or creates the docs pane if needed;
- the agent can cite the page path and heading it opened.

### 3. Agent asks for a search

```text
/docs search "vault provider key"
```

Expected result:

- search is scoped to `docs_dir` from `mkdocs.yml`;
- results show page title, path, heading context, and line number;
- selecting a result opens the page at the matching heading or line.

### 4. Optional browser validation

For final MkDocs-theme validation only:

```text
/docs serve
```

Expected result:

- PFTerminal runs `mkdocs serve --dev-addr 127.0.0.1:<port>`;
- the process is bound to loopback only;
- status shows the local URL;
- stopping the docs pane kills the process group.

Normal reading and agent navigation must not require this mode.

## Product Contract

The viewer is source-first and safe by default:

- It reads `mkdocs.yml`.
- It respects `docs_dir`.
- It uses the configured `nav` if present.
- If `nav` is absent, it discovers Markdown files under `docs_dir` with
  MkDocs-like ordering: index pages first, then alphanumeric order.
- It renders Markdown source directly into a terminal view.
- It watches only `mkdocs.yml`, `docs_dir`, and explicitly configured watch
  paths.
- It exposes bounded open/search operations to agents.
- It never falls back to unbounded `grep -r` over the repo or home directory.

Rendered HTML mode can be added later for plugin-generated pages, but the MVP
must not depend on a browser or a generated static site.

## UX Sketch

```text
┌ MkDocs: PFTerminal ───────────────┬ docs/current-sprint/mkdocs-terminal-viewer.md ─────┐
│ Home                              │ # MkDocs Terminal Viewer                           │
│ Getting Started                   │                                                     │
│ Current Sprint                    │ Status: proposed.                                  │
│   Credential Store                │                                                     │
│   Tool Call Runaway Remedy        │ ## Problem Statement                               │
│   MkDocs Terminal Viewer          │                                                     │
│ Integrations                      │ PFTerminal repos increasingly use MkDocs...        │
│   Ambient                         │                                                     │
│   Z.AI GLM 5.2                    │                                                     │
├───────────────────────────────────┴─────────────────────────────────────────────────────┤
│ / search  o open  b back  r reload  s serve  q close  status: watching docs/            │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

Recommended keybindings:

| Key | Action |
| --- | --- |
| `j` / `k` | scroll page down/up |
| `PgDown` / `PgUp` | page down/up |
| `g` / `G` | top/bottom |
| `Tab` | switch focus between nav and page |
| `Enter` | open selected nav item or link |
| `b` | back |
| `/` | search docs |
| `n` / `N` | next/previous search hit |
| `r` | reload current project |
| `s` | start/stop optional loopback `mkdocs serve` |
| `q` | close docs viewer |

## Commands

### CLI

```bash
pfterminal docs
pfterminal docs --config mkdocs.yml
pfterminal docs open docs/config.md
pfterminal docs open "Configuration"
pfterminal docs open "config#provider-choices"
pfterminal docs search "provider auth"
pfterminal docs serve --dev-addr 127.0.0.1:8123
```

### Slash Commands

```text
/docs
/docs exec.md
/docs current-sprint/mkdocs-terminal-viewer.md
/docs /home/pfrpc/repos/PfTerminal exec.md
/docs --config /home/pfrpc/repos/PfTerminal/mkdocs.yml exec.md
/docs --docs-dir /home/pfrpc/repos/PfTerminal/docs exec.md
```

The viewer has two focus modes. In index mode, use `j`/`k` or arrow keys to
move through the page index, `/` to filter the page index, and `Enter` or
Right-arrow to read the selected page. In page mode, use `j`/`k` or arrow keys
to scroll the rendered page by one line, `Ctrl+d`/`Ctrl+u` to scroll by half a
page, `Ctrl+f`/`Ctrl+b` or Space to scroll by a page, and `Esc`, Left-arrow, or
Tab to return to the page index. `q` closes the viewer.

### Agent API

Expose a structured internal API instead of relying on shell commands:

```text
docs/discover
docs/open
docs/search
docs/current
docs/reload
docs/serve/start
docs/serve/stop
```

Every API response must be bounded and structured. Search results should return
only page metadata and short snippets, not whole files.

## Architecture

### Project discovery

Discovery starts from the current workspace and walks upward until it finds
`mkdocs.yml` or `mkdocs.yaml`.

The parsed project state should include:

```text
DocsProject {
  root_dir
  config_path
  docs_dir
  site_name
  use_directory_urls
  nav_nodes
  pages
  watched_paths
}
```

If multiple MkDocs configs exist below the workspace, `/docs` should offer a
picker rather than guessing.

### Nav loading

MVP nav support:

- string nav entries;
- title-to-path mappings;
- nested sections;
- omitted `nav` using filesystem discovery;
- `index.md` preferred over `README.md` in the same directory.

Unsupported nav nodes should be shown as disabled rows with an explanatory
status message, not silently dropped.

### Markdown rendering

PFTerminal is a Rust TUI, so the implementation should prefer a Rust-native
renderer that converts Markdown into PFTerminal/Ratatui lines and widgets.

MVP rendering must cover:

- headings;
- paragraphs;
- emphasis and inline code;
- fenced code blocks with language labels;
- ordered and unordered lists;
- blockquotes;
- tables;
- horizontal rules;
- links;
- anchors/headings;
- common admonition syntax as readable blocks.

The implementation may use a Markdown parser such as `pulldown-cmark` and a
small PFTerminal render layer. A Python/Textual prototype is acceptable for
exploration, but the production feature should not require a second TUI runtime
unless that is explicitly accepted.

### Link resolution

The viewer must resolve:

- relative Markdown links: `../config.md`;
- extensionless MkDocs links where possible;
- directory-style links when `use_directory_urls: true`;
- anchors: `config.md#provider-choices`;
- same-page anchors: `#provider-choices`;
- absolute site-local links: `/current-sprint/tool-call-runaway-remedy/`.

External links should open through the normal terminal/browser policy, not
inside the MkDocs viewer.

### Live reload

Use a bounded file watcher for:

- `mkdocs.yml` / `mkdocs.yaml`;
- `docs_dir`;
- `watch` entries from MkDocs config.

Reload behavior:

- debounce rapid changes;
- reparse nav only when config or page set changes;
- rerender current page on Markdown edits;
- preserve current page and approximate scroll offset;
- show a recoverable error panel on invalid YAML or unreadable files.

### Search

Search is a viewer capability, not arbitrary shell execution.

Requirements:

- search only `docs_dir` plus explicitly configured docs watch paths;
- use `ripgrep` if available, with hard excludes and timeouts;
- if `ripgrep` is missing, use a bounded in-process search over known page
  files;
- never run `grep -r` over the repo or `$HOME`;
- cap number of files scanned, bytes scanned, result count, and snippet size.

### Optional rendered validation mode

Some MkDocs projects rely on plugins or theme behavior that cannot be captured
from raw Markdown. For that case, add an optional validation mode:

- run `mkdocs build` into a temp directory or `mkdocs serve` on loopback;
- keep the process in its own process group;
- kill the process group on timeout, close, interrupt, or PFTerminal exit;
- show generated-page status in the docs viewer;
- do not make this mode required for normal reading.

## Safety Requirements

- No public listener by default.
- Any `mkdocs serve` process must bind `127.0.0.1`.
- All child processes must run with timeouts and process-group cleanup.
- Search must be scoped to known docs files.
- Agent calls must use `docs/*` APIs, not invented shell commands.
- Missing optional dependencies must fail closed with clear install guidance.
- The viewer must remain responsive with large docs trees.

## Acceptance Gates

### Milestone 1: implemented source navigator

- From a repo root with `mkdocs.yml`, `/docs` opens a terminal viewer without
  starting a web server.
- Opening `/docs` from a nested working directory discovers the nearest parent
  MkDocs project.
- The viewer shows a page index and current page side by side.
- Page ordering respects explicit `nav` path order when present and falls back
  to bounded Markdown discovery under `docs_dir`.
- Direct page hints such as `/docs exec.md` select matching pages by exact path,
  suffix, or substring.
- Explicit roots work with `/docs /path/to/repo [page]`.
- Explicit configs work with `/docs --config /path/to/mkdocs.yml [page]`.
- Explicit docs directories work with `/docs --docs-dir /path/to/docs [page]`.
- The rendered page is scrollable independently of page-index selection.
- After selecting a page, arrow keys and `j`/`k` scroll the page by one line
  until the user returns to the page index.
- Page-index filtering is interactive and does not run broad shell searches.
- Missing MkDocs config, missing `docs_dir`, or invalid page hints produce a
  concise error in the chat view.

The remaining gates describe the full target product and are not all satisfied
by the Milestone 1 implementation.

### P0: source-mode viewer

- From a repo root with `mkdocs.yml`, `/docs` opens a terminal viewer without
  starting a web server.
- The viewer shows a nav tree and current page side by side.
- The current page is scrollable with keyboard controls.
- Opening `/docs` from a nested working directory still discovers the nearest
  parent MkDocs project.
- If no MkDocs project exists, the UI shows a concise "no mkdocs.yml found"
  message and exits cleanly.

### P0: MkDocs nav fidelity

- Explicit `nav` entries in `mkdocs.yml` render in configured order.
- Nested nav sections render with hierarchy.
- Projects without `nav` auto-discover Markdown files under `docs_dir`.
- `index.md` sorts before sibling pages.
- `README.md` is ignored when `index.md` exists in the same directory.
- Pages omitted from `nav` are still openable by direct path/search.

### P0: page rendering

- Headings, paragraphs, lists, code blocks, tables, blockquotes, links, and
  admonition-like blocks render readably in the terminal.
- Code blocks do not resize or corrupt surrounding layout when long lines are
  present.
- Long words/URLs wrap or horizontally scroll without overlapping UI chrome.
- Render failures are shown as recoverable page errors.

### P0: navigation and links

- Selecting a nav item opens the correct page.
- Relative links between Markdown pages resolve correctly.
- Same-page and cross-page anchors scroll to the correct heading when possible.
- Broken links produce a visible status message instead of crashing.
- Back navigation returns to the prior page and approximate scroll position.

### P0: live reload

- Editing the currently open Markdown file updates the viewer within two
  seconds after save.
- Editing `mkdocs.yml` reloads the nav without restarting PFTerminal.
- Invalid YAML shows an error and keeps the last good nav/page visible.
- Fixing the YAML clears the error and reloads the project.
- Scroll position is preserved on small edits when the current heading still
  exists.

### P0: agent control

- An agent can call `docs/open` with a page path and the visible docs pane opens
  that page.
- An agent can call `docs/open` with a nav title and the viewer opens the best
  exact or fuzzy match.
- An agent can call `docs/search` and receives bounded structured results:
  title, path, heading, line, and short snippet.
- Agent docs operations do not execute arbitrary shell commands.
- If no docs pane is open, agent `docs/open` creates or focuses one according to
  PFTerminal pane policy.

### P0: resource safety

- Search is scoped to MkDocs docs files and never recursively scans `$HOME`.
- Search has wall-clock and output caps.
- File watching is scoped to docs/config paths and uses debounce.
- Closing the viewer stops watchers and any optional child processes.
- Interrupting a docs pane does not kill unrelated PFTerminal panes.

### P1: optional MkDocs serve validation

- `/docs serve` starts `mkdocs serve` on `127.0.0.1:<port>`.
- The selected port is shown in the status bar.
- The process group is killed on `/docs serve stop`, pane close, timeout, or
  PFTerminal exit.
- If the port is occupied, the viewer selects another loopback port or shows an
  actionable error.
- This mode is not required for source-mode viewing.

### P1: plugin-generated pages

- The viewer detects pages present in generated MkDocs output but absent from
  source Markdown.
- Generated-only pages are visible as "rendered-only" entries when rendered
  validation mode is enabled.
- If generated pages cannot be rendered terminal-natively, the viewer shows a
  clear limitation and local URL.

### P1: tmux workflow

- A user can keep one tmux pane open as the MkDocs viewer and another as the
  agent/editor.
- Agent `docs/open` focuses or updates the existing docs pane instead of
  spawning duplicates.
- `/docs status` reports project root, current page, watcher state, serve state,
  and last reload time.

## Test Plan

Create fixture MkDocs projects under the appropriate test fixture directory:

1. `simple_nav`: explicit two-page nav.
2. `nested_nav`: nested sections and title/path mappings.
3. `auto_nav`: no `nav`, multiple directories, `index.md` and `README.md`.
4. `links`: relative links, anchors, broken links.
5. `large_docs`: many pages and large files to prove bounds.
6. `invalid_config`: invalid YAML recovery.

Required automated coverage:

- project discovery unit tests;
- nav parser tests;
- link resolver tests;
- bounded search tests;
- watcher reload tests where platform support allows;
- TUI snapshot tests for representative pages;
- app-server/API tests for `docs/open` and `docs/search`;
- process cleanup test for optional `mkdocs serve`.

Required live acceptance:

- Run PFTerminal in a tmux session inside a real MkDocs repo.
- Open `/docs`.
- Edit the current page and observe live reload.
- Ask an agent to open a specific docs page.
- Ask an agent to search a docs term.
- Start and stop optional loopback serve mode.
- Confirm no orphaned `mkdocs`, search, or watcher child processes remain.

## Implementation Milestones

### Milestone 1: read-only source viewer

- CLI/slash command opens viewer.
- Discovery, nav loading, Markdown render, manual open, and scrolling work.

### Milestone 2: live reload and search

- Watch config/docs files.
- Add bounded search.
- Add link and anchor navigation.

### Milestone 3: agent API and pane integration

- Add structured `docs/*` API.
- Let agents open/search docs without shell commands.
- Reuse/focus existing docs pane.

### Milestone 4: optional MkDocs serve validation

- Add loopback-only `mkdocs serve` lifecycle.
- Add process cleanup and status UI.

### Milestone 5: plugin/rendered output support

- Detect generated-only pages.
- Provide rendered validation metadata and limitations.

## Open Questions

- Should docs viewer state be global per PFTerminal session or per workspace
  pane?
- Should `/docs` live inside the existing pane system or use a dedicated
  full-screen modal?
- Should the MVP render Markdown through a new Rust renderer or embed an
  existing terminal Markdown renderer behind a stable internal interface?
- How should image links be represented in terminals that support inline image
  protocols versus plain SSH terminals?
- Should rendered validation mode use `mkdocs build` plus static files or
  `mkdocs serve` with live reload?

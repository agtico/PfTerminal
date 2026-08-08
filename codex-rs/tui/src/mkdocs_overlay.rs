use std::io::Result;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use pulldown_cmark::Event;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::key_hint::KeyBindingListExt;
use crate::keymap::ListKeymap;
use crate::keymap::PagerKeymap;
use crate::mkdocs_viewer::MkDocsSite;
use crate::tui;
use crate::tui::TuiEvent;

pub(crate) struct MkDocsOverlay {
    site: MkDocsSite,
    selected_index: usize,
    visible_indices: Vec<usize>,
    list_scroll: usize,
    page_scroll: usize,
    page_source: String,
    page_error: Option<String>,
    page_links: Vec<DocLink>,
    link_picker_active: bool,
    selected_link_index: usize,
    back_stack: Vec<PageLocation>,
    forward_stack: Vec<PageLocation>,
    status_message: Option<String>,
    search_active: bool,
    search_query: String,
    focus: MkDocsFocus,
    render_cache: Option<PageRenderCache>,
    pager_keymap: PagerKeymap,
    list_keymap: ListKeymap,
    is_done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MkDocsFocus {
    Index,
    Page,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DocLink {
    label: String,
    destination: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PageLocation {
    page_index: usize,
    page_scroll: usize,
}

#[derive(Clone)]
struct PageRenderCache {
    page_index: usize,
    source: String,
    width: u16,
    lines: Vec<Line<'static>>,
}

impl MkDocsOverlay {
    pub(crate) fn new(
        site: MkDocsSite,
        pager_keymap: PagerKeymap,
        list_keymap: ListKeymap,
    ) -> Self {
        let selected_index = site.selected_index.min(site.pages.len().saturating_sub(1));
        let mut overlay = Self {
            site,
            selected_index,
            visible_indices: Vec::new(),
            list_scroll: 0,
            page_scroll: 0,
            page_source: String::new(),
            page_error: None,
            page_links: Vec::new(),
            link_picker_active: false,
            selected_link_index: 0,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            status_message: None,
            search_active: false,
            search_query: String::new(),
            focus: MkDocsFocus::Index,
            render_cache: None,
            pager_keymap,
            list_keymap,
            is_done: false,
        };
        overlay.refresh_visible_indices();
        overlay.load_selected_page();
        overlay
    }

    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Draw | TuiEvent::Resize(_) => {
                tui.draw(u16::MAX, |frame| self.render(frame.area(), frame.buffer))?;
            }
            TuiEvent::Key(key_event) => {
                self.handle_key_event(tui, key_event);
            }
            TuiEvent::Paste(text) if self.search_active => {
                self.search_query.push_str(&text.replace(['\r', '\n'], " "));
                self.refresh_visible_indices();
                tui.frame_requester()
                    .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn is_done(&self) -> bool {
        self.is_done
    }

    fn handle_key_event(&mut self, tui: &mut tui::Tui, key_event: KeyEvent) {
        self.status_message = None;
        if self.search_active {
            self.handle_search_key_event(key_event);
            tui.frame_requester()
                .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
            return;
        }

        if self.pager_keymap.close.is_pressed(key_event) {
            self.is_done = true;
        } else if self.link_picker_active {
            self.handle_link_picker_key_event(tui, key_event);
        } else if key_event.code == KeyCode::Char('/') && key_event.modifiers.is_empty() {
            self.search_active = true;
            self.focus = MkDocsFocus::Index;
            self.search_query.clear();
            self.refresh_visible_indices();
        } else if key_event.code == KeyCode::Tab && key_event.modifiers.is_empty() {
            self.toggle_focus();
        } else {
            match self.focus {
                MkDocsFocus::Index => self.handle_index_key_event(key_event),
                MkDocsFocus::Page => self.handle_page_key_event(tui, key_event),
            }
        }

        tui.frame_requester()
            .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
    }

    fn handle_search_key_event(&mut self, key_event: KeyEvent) {
        if self.pager_keymap.close.is_pressed(key_event) {
            self.is_done = true;
            return;
        }

        match key_event.code {
            KeyCode::Esc => {
                if self.search_query.is_empty() {
                    self.search_active = false;
                } else {
                    self.search_query.clear();
                    self.refresh_visible_indices();
                }
            }
            KeyCode::Enter => {
                self.search_active = false;
                self.focus = MkDocsFocus::Page;
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.refresh_visible_indices();
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::Char(ch)
                if key_event.modifiers.is_empty() || key_event.modifiers == KeyModifiers::SHIFT =>
            {
                self.search_query.push(ch);
                self.refresh_visible_indices();
            }
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.visible_indices.is_empty() {
            return;
        }
        let current_position = self
            .visible_indices
            .iter()
            .position(|index| *index == self.selected_index)
            .unwrap_or(0);
        let next_position = if delta.is_negative() {
            current_position.saturating_sub(delta.unsigned_abs())
        } else {
            (current_position + delta as usize).min(self.visible_indices.len() - 1)
        };
        let next_index = self.visible_indices[next_position];
        if next_index != self.selected_index {
            self.selected_index = next_index;
            self.page_scroll = 0;
            self.load_selected_page();
        }
        self.ensure_selected_visible();
    }

    fn handle_index_key_event(&mut self, key_event: KeyEvent) {
        if self.list_keymap.cancel.is_pressed(key_event) {
            self.is_done = true;
        } else if self.list_keymap.move_up.is_pressed(key_event) {
            self.move_selection(-1);
        } else if self.list_keymap.move_down.is_pressed(key_event) {
            self.move_selection(1);
        } else if self.list_keymap.page_up.is_pressed(key_event) {
            self.move_selection(-10);
        } else if self.list_keymap.page_down.is_pressed(key_event) {
            self.move_selection(10);
        } else if self.list_keymap.jump_top.is_pressed(key_event) {
            self.select_visible_position(0);
        } else if self.list_keymap.jump_bottom.is_pressed(key_event) {
            self.select_visible_position(self.visible_indices.len().saturating_sub(1));
        } else if self.list_keymap.accept.is_pressed(key_event)
            || self.list_keymap.move_right.is_pressed(key_event)
        {
            self.focus = MkDocsFocus::Page;
            self.page_scroll = 0;
        }
    }

    fn handle_page_key_event(&mut self, tui: &tui::Tui, key_event: KeyEvent) {
        if self.list_keymap.cancel.is_pressed(key_event)
            || self.list_keymap.move_left.is_pressed(key_event)
        {
            self.focus = MkDocsFocus::Index;
        } else if (key_event.code == KeyCode::Enter || key_event.code == KeyCode::Char('o'))
            && key_event.modifiers.is_empty()
        {
            if self.page_links.is_empty() {
                self.status_message = Some("This page has no links.".to_string());
            } else {
                self.link_picker_active = true;
                self.selected_link_index = 0;
                self.status_message = None;
            }
        } else if key_event.code == KeyCode::Char('b') && key_event.modifiers.is_empty() {
            self.navigate_history(/*back*/ true);
        } else if key_event.code == KeyCode::Char('f') && key_event.modifiers.is_empty() {
            self.navigate_history(/*back*/ false);
        } else if self.list_keymap.move_up.is_pressed(key_event) || is_ctrl_y(key_event) {
            self.scroll_page_by(-1);
        } else if self.list_keymap.move_down.is_pressed(key_event) || is_ctrl_e(key_event) {
            self.scroll_page_by(1);
        } else if self.pager_keymap.page_up.is_pressed(key_event) {
            self.scroll_page_by(-self.page_height(tui));
        } else if self.pager_keymap.page_down.is_pressed(key_event) {
            self.scroll_page_by(self.page_height(tui));
        } else if self.pager_keymap.half_page_up.is_pressed(key_event) {
            self.scroll_page_by(-(self.page_height(tui) / 2).max(1));
        } else if self.pager_keymap.half_page_down.is_pressed(key_event) {
            self.scroll_page_by((self.page_height(tui) / 2).max(1));
        } else if self.pager_keymap.jump_top.is_pressed(key_event) {
            self.page_scroll = 0;
        } else if self.pager_keymap.jump_bottom.is_pressed(key_event) {
            self.page_scroll = usize::MAX;
        }
    }

    fn select_visible_position(&mut self, position: usize) {
        let Some(next_index) = self.visible_indices.get(position).copied() else {
            return;
        };
        if next_index != self.selected_index {
            self.selected_index = next_index;
            self.page_scroll = 0;
            self.load_selected_page();
        }
        self.ensure_selected_visible();
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            MkDocsFocus::Index => MkDocsFocus::Page,
            MkDocsFocus::Page => MkDocsFocus::Index,
        };
    }

    fn refresh_visible_indices(&mut self) {
        let query = self.search_query.trim();
        self.visible_indices = self
            .site
            .pages
            .iter()
            .enumerate()
            .filter_map(|(index, _)| self.site.page_matches_query(index, query).then_some(index))
            .collect();

        if !self.visible_indices.contains(&self.selected_index)
            && let Some(first) = self.visible_indices.first().copied()
        {
            self.selected_index = first;
            self.page_scroll = 0;
            self.load_selected_page();
        }
        self.ensure_selected_visible();
    }

    fn load_selected_page(&mut self) {
        self.render_cache = None;
        self.link_picker_active = false;
        self.selected_link_index = 0;
        match self.site.read_page_source(self.selected_index) {
            Ok(source) => {
                self.page_links = extract_doc_links(&source);
                self.page_source = source;
                self.page_error = None;
            }
            Err(err) => {
                self.page_links.clear();
                self.page_source.clear();
                self.page_error = Some(err.to_string());
            }
        }
    }

    fn handle_link_picker_key_event(&mut self, tui: &tui::Tui, key_event: KeyEvent) {
        if self.list_keymap.cancel.is_pressed(key_event)
            || self.list_keymap.move_left.is_pressed(key_event)
        {
            self.link_picker_active = false;
        } else if self.list_keymap.move_up.is_pressed(key_event) {
            self.selected_link_index = self.selected_link_index.saturating_sub(1);
        } else if self.list_keymap.move_down.is_pressed(key_event) {
            self.selected_link_index =
                (self.selected_link_index + 1).min(self.page_links.len().saturating_sub(1));
        } else if self.list_keymap.page_up.is_pressed(key_event) {
            self.selected_link_index = self.selected_link_index.saturating_sub(10);
        } else if self.list_keymap.page_down.is_pressed(key_event) {
            self.selected_link_index =
                (self.selected_link_index + 10).min(self.page_links.len().saturating_sub(1));
        } else if self.list_keymap.jump_top.is_pressed(key_event) {
            self.selected_link_index = 0;
        } else if self.list_keymap.jump_bottom.is_pressed(key_event) {
            self.selected_link_index = self.page_links.len().saturating_sub(1);
        } else if self.list_keymap.accept.is_pressed(key_event)
            || self.list_keymap.move_right.is_pressed(key_event)
        {
            self.open_selected_link(tui);
        }
    }

    fn open_selected_link(&mut self, tui: &tui::Tui) {
        let Some(link) = self.page_links.get(self.selected_link_index).cloned() else {
            self.status_message = Some("No documentation link is selected.".to_string());
            self.link_picker_active = false;
            return;
        };
        match self
            .site
            .resolve_internal_link(self.selected_index, &link.destination)
        {
            Ok(target) => {
                self.back_stack.push(PageLocation {
                    page_index: self.selected_index,
                    page_scroll: self.page_scroll,
                });
                self.forward_stack.clear();
                self.selected_index = target.page_index;
                self.page_scroll = 0;
                self.load_selected_page();
                let missing_anchor = target.anchor.as_deref().and_then(|anchor| {
                    (!self.scroll_to_anchor(anchor, self.page_content_width(tui)))
                        .then(|| anchor.to_string())
                });
                self.focus = MkDocsFocus::Page;
                self.ensure_selected_visible();
                self.status_message = Some(match missing_anchor {
                    Some(anchor) => format!(
                        "Opened {}, but heading anchor `#{anchor}` was not found.",
                        self.site.pages[self.selected_index].rel_path.display()
                    ),
                    None => format!(
                        "Opened {}.",
                        self.site.pages[self.selected_index].rel_path.display()
                    ),
                });
            }
            Err(err) => {
                self.status_message = Some(err.to_string());
                self.link_picker_active = false;
            }
        }
    }

    fn navigate_history(&mut self, back: bool) {
        let (source, destination) = if back {
            (&mut self.back_stack, &mut self.forward_stack)
        } else {
            (&mut self.forward_stack, &mut self.back_stack)
        };
        let Some(location) = source.pop() else {
            self.status_message = Some(if back {
                "No previous documentation location.".to_string()
            } else {
                "No forward documentation location.".to_string()
            });
            return;
        };
        destination.push(PageLocation {
            page_index: self.selected_index,
            page_scroll: self.page_scroll,
        });
        self.selected_index = location.page_index;
        self.load_selected_page();
        self.page_scroll = location.page_scroll;
        self.focus = MkDocsFocus::Page;
        self.ensure_selected_visible();
        self.status_message = Some(format!(
            "Returned to {}.",
            self.site.pages[self.selected_index].rel_path.display()
        ));
    }

    fn scroll_to_anchor(&mut self, anchor: &str, width: u16) -> bool {
        let Some(source_offset) = heading_source_offset(&self.page_source, anchor) else {
            return false;
        };
        let mut preceding_lines = Vec::new();
        crate::markdown::append_markdown(
            &self.page_source[..source_offset],
            Some(width.max(1) as usize),
            Some(self.site.project_root.as_path()),
            &mut preceding_lines,
        );
        self.page_scroll = preceding_lines.len().saturating_sub(1);
        true
    }

    fn page_content_width(&self, tui: &tui::Tui) -> u16 {
        let body = self.body_area(tui.terminal.viewport_area);
        let (_, page_area) = split_body(body);
        page_area.width
    }

    fn page_height(&self, tui: &tui::Tui) -> isize {
        let body = self.body_area(tui.terminal.viewport_area);
        body.height.saturating_sub(2).max(1) as isize
    }

    fn scroll_page_by(&mut self, amount: isize) {
        if amount.is_negative() {
            self.page_scroll = self.page_scroll.saturating_sub(amount.unsigned_abs());
        } else {
            self.page_scroll = self.page_scroll.saturating_add(amount as usize);
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        self.render_header(area, buf);
        let body = self.body_area(area);
        let footer = Rect::new(area.x, body.bottom(), area.width, 1);
        let (list_area, page_area) = split_body(body);
        if self.link_picker_active {
            self.render_link_index(list_area, buf);
        } else {
            self.render_page_index(list_area, buf);
        }
        self.render_page(page_area, buf);
        self.render_footer(footer, buf);
    }

    fn body_area(&self, area: Rect) -> Rect {
        Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.saturating_sub(2),
        )
    }

    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        let row = Rect::new(area.x, area.y, area.width, 1);
        Span::from("/ ".repeat(area.width as usize / 2))
            .dim()
            .render(row, buf);
        let title = format!(
            "/ {}  {}",
            self.site.overlay_title(),
            self.site.config_path.display()
        );
        Span::from(fit_text(&title, row.width))
            .dim()
            .render(row, buf);
    }

    fn render_page_index(&mut self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.ensure_selected_visible_for_height(area.height as usize);

        let title = if self.search_query.is_empty() {
            format!("Pages ({})", self.site.pages.len())
        } else {
            format!(
                "Pages ({}/{})",
                self.visible_indices.len(),
                self.site.pages.len()
            )
        };
        let title_style = if self.focus == MkDocsFocus::Index && !self.search_active {
            Style::default().bold().reversed()
        } else {
            Style::default().bold()
        };
        Span::styled(fit_text(&title, area.width), title_style)
            .render(Rect::new(area.x, area.y, area.width, 1), buf);

        let rows = area.height.saturating_sub(1) as usize;
        let start_y = area.y.saturating_add(1);
        for row in 0..rows {
            let y = start_y + row as u16;
            let rect = Rect::new(area.x, y, area.width, 1);
            let Some(page_index) = self.visible_indices.get(self.list_scroll + row).copied() else {
                clear_row(rect, buf);
                continue;
            };
            let page = &self.site.pages[page_index];
            let selected = page_index == self.selected_index;
            let prefix = if selected { "> " } else { "  " };
            let text = format!("{prefix}{}", page.rel_path.display());
            let style = if selected && self.focus == MkDocsFocus::Index {
                Style::default().reversed()
            } else if selected {
                Style::default().bold()
            } else {
                Style::default()
            };
            Span::styled(fit_text(&text, area.width), style).render(rect, buf);
        }

        if area.right() > 0 {
            let divider_x = area.right() - 1;
            for y in area.y..area.bottom() {
                let mut cell = Cell::from('|');
                cell.set_style(Style::default().dim());
                buf[(divider_x, y)] = cell;
            }
        }
    }

    fn render_link_index(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        Span::styled(
            fit_text(&format!("Links ({})", self.page_links.len()), area.width),
            Style::default().bold().reversed(),
        )
        .render(Rect::new(area.x, area.y, area.width, 1), buf);

        let rows = area.height.saturating_sub(1) as usize;
        let max_start = self.page_links.len().saturating_sub(rows);
        let start = self
            .selected_link_index
            .saturating_sub(rows.saturating_sub(1) / 2)
            .min(max_start);
        for row in 0..rows {
            let y = area.y.saturating_add(1).saturating_add(row as u16);
            let rect = Rect::new(area.x, y, area.width, 1);
            let Some((index, link)) = self.page_links.iter().enumerate().nth(start + row) else {
                clear_row(rect, buf);
                continue;
            };
            let selected = index == self.selected_link_index;
            let prefix = if selected { "> " } else { "  " };
            let label = if link.label.is_empty() {
                link.destination.as_str()
            } else {
                link.label.as_str()
            };
            let text = format!("{prefix}{label} -> {}", link.destination);
            let style = if selected {
                Style::default().reversed()
            } else {
                Style::default()
            };
            Span::styled(fit_text(&text, area.width), style).render(rect, buf);
        }

        if area.right() > 0 {
            let divider_x = area.right() - 1;
            for y in area.y..area.bottom() {
                let mut cell = Cell::from('|');
                cell.set_style(Style::default().dim());
                buf[(divider_x, y)] = cell;
            }
        }
    }

    fn render_page(&mut self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let Some(page) = self.site.pages.get(self.selected_index) else {
            Paragraph::new("No page selected.").render(area, buf);
            return;
        };

        let title = format!("{}", page.rel_path.display());
        let title_style = if self.focus == MkDocsFocus::Page && !self.search_active {
            Style::default().bold().reversed()
        } else {
            Style::default().bold()
        };
        Span::styled(fit_text(&title, area.width), title_style)
            .render(Rect::new(area.x, area.y, area.width, 1), buf);

        let docs_dir = format!("docs: {}", self.site.docs_dir.display());
        Span::from(fit_text(&docs_dir, area.width)).dim().render(
            Rect::new(area.x, area.y.saturating_add(1), area.width, 1),
            buf,
        );

        let content_area = Rect::new(
            area.x,
            area.y.saturating_add(2),
            area.width,
            area.height.saturating_sub(2),
        );
        if content_area.height == 0 {
            return;
        }

        let lines = self.rendered_page_lines(content_area.width);
        let max_scroll = lines.len().saturating_sub(content_area.height as usize);
        self.page_scroll = self.page_scroll.min(max_scroll);
        let visible = lines
            .iter()
            .skip(self.page_scroll)
            .take(content_area.height as usize)
            .cloned()
            .collect::<Vec<_>>();
        Paragraph::new(Text::from(visible)).render(content_area, buf);

        let drawn = lines
            .len()
            .saturating_sub(self.page_scroll)
            .min(content_area.height as usize) as u16;
        for y in content_area.y.saturating_add(drawn)..content_area.bottom() {
            clear_row(Rect::new(content_area.x, y, content_area.width, 1), buf);
        }
    }

    fn rendered_page_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        if let Some(cache) = &self.render_cache
            && cache.page_index == self.selected_index
            && cache.width == width
            && cache.source == self.page_source
        {
            return cache.lines.clone();
        }

        let mut lines = Vec::new();
        if let Some(error) = &self.page_error {
            lines.push(Line::from(Span::styled(
                error.clone(),
                Style::default().red(),
            )));
        } else {
            crate::markdown::append_markdown(
                &self.page_source,
                Some(width as usize),
                Some(self.site.project_root.as_path()),
                &mut lines,
            );
        }
        if lines.is_empty() {
            lines.push(Line::from("(empty page)").dim());
        }
        self.render_cache = Some(PageRenderCache {
            page_index: self.selected_index,
            source: self.page_source.clone(),
            width,
            lines: lines.clone(),
        });
        lines
    }

    fn render_footer(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 {
            return;
        }
        let status = if let Some(message) = &self.status_message {
            format!(" {message} ")
        } else if self.link_picker_active {
            "links: up/down/j/k select  Enter/Right open  Esc/Left cancel  q close ".to_string()
        } else if self.search_active {
            format!(
                " search paths/content: {}  Enter accept  Esc clear/exit search  q close ",
                self.search_query
            )
        } else if self.focus == MkDocsFocus::Page {
            "page: j/k scroll  Enter/o links  b back  f forward  Esc/Left index  q close "
                .to_string()
        } else if self.search_query.is_empty() {
            "index: up/down/j/k select  Enter/Right read page  / filter  q/Esc close ".to_string()
        } else {
            format!(
                "filter: {}  up/down/j/k select  Enter/Right read page  / refilter  q/Esc close ",
                self.search_query
            )
        };
        Span::from("─".repeat(area.width as usize))
            .dim()
            .render(area, buf);
        Span::from(fit_text(&status, area.width))
            .dim()
            .render(area, buf);
    }

    fn ensure_selected_visible(&mut self) {
        self.ensure_selected_visible_for_height(usize::MAX);
    }

    fn ensure_selected_visible_for_height(&mut self, height: usize) {
        let Some(position) = self
            .visible_indices
            .iter()
            .position(|index| *index == self.selected_index)
        else {
            self.list_scroll = 0;
            return;
        };
        if position < self.list_scroll {
            self.list_scroll = position;
        } else {
            let rows = height.saturating_sub(1);
            if rows > 0 && position >= self.list_scroll + rows {
                self.list_scroll = position + 1 - rows;
            }
        }
        let max_scroll = self
            .visible_indices
            .len()
            .saturating_sub(height.saturating_sub(1));
        self.list_scroll = self.list_scroll.min(max_scroll);
    }
}

fn split_body(area: Rect) -> (Rect, Rect) {
    if area.width < 48 {
        let list_width = (area.width / 2).max(16).min(area.width);
        let page_x = area.x.saturating_add(list_width);
        return (
            Rect::new(area.x, area.y, list_width, area.height),
            Rect::new(
                page_x,
                area.y,
                area.width.saturating_sub(list_width),
                area.height,
            ),
        );
    }

    let list_width = ((area.width as usize * 35) / 100)
        .clamp(24, 44)
        .min(area.width as usize) as u16;
    let page_x = area.x.saturating_add(list_width).saturating_add(1);
    (
        Rect::new(area.x, area.y, list_width, area.height),
        Rect::new(
            page_x,
            area.y,
            area.right().saturating_sub(page_x),
            area.height,
        ),
    )
}

fn fit_text(text: &str, width: u16) -> String {
    text.chars().take(width as usize).collect()
}

fn clear_row(area: Rect, buf: &mut Buffer) {
    for x in area.x..area.right() {
        buf[(x, area.y)] = Cell::from(' ');
    }
}

fn is_ctrl_e(key_event: KeyEvent) -> bool {
    key_event.code == KeyCode::Char('e') && key_event.modifiers == KeyModifiers::CONTROL
}

fn is_ctrl_y(key_event: KeyEvent) -> bool {
    key_event.code == KeyCode::Char('y') && key_event.modifiers == KeyModifiers::CONTROL
}

fn extract_doc_links(source: &str) -> Vec<DocLink> {
    let mut links = Vec::new();
    let mut active: Option<DocLink> = None;
    for event in Parser::new(source) {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                active = Some(DocLink {
                    label: String::new(),
                    destination: dest_url.into_string(),
                });
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(link) = active.as_mut() {
                    link.label.push_str(&text);
                }
            }
            Event::End(TagEnd::Link) => {
                if let Some(mut link) = active.take() {
                    link.label = link.label.trim().to_string();
                    links.push(link);
                }
            }
            _ => {}
        }
    }
    links
}

fn heading_source_offset(source: &str, anchor: &str) -> Option<usize> {
    let expected = anchor.trim().trim_start_matches('#').to_ascii_lowercase();
    let mut active_heading: Option<(usize, String)> = None;
    for (event, range) in Parser::new(source).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                active_heading = Some((range.start, String::new()));
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, heading)) = active_heading.as_mut() {
                    heading.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((offset, heading)) = active_heading.take()
                    && heading_matches_anchor(&heading, &expected)
                {
                    return Some(offset);
                }
            }
            Event::Html(html) | Event::InlineHtml(html)
                if html_anchor_id(&html).is_some_and(|id| id.eq_ignore_ascii_case(&expected)) =>
            {
                return Some(range.start);
            }
            _ => {}
        }
    }
    None
}

fn heading_matches_anchor(heading: &str, expected: &str) -> bool {
    let heading = heading.trim();
    if let Some(start) = heading.rfind("{#")
        && let Some(explicit) = heading[start + 2..].strip_suffix('}')
        && explicit.eq_ignore_ascii_case(expected)
    {
        return true;
    }
    slugify_heading(heading) == expected
}

fn slugify_heading(heading: &str) -> String {
    let heading = heading
        .rfind("{#")
        .map_or(heading, |explicit_start| &heading[..explicit_start]);
    let mut slug = String::new();
    let mut pending_dash = false;
    for ch in heading.chars() {
        if ch.is_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.extend(ch.to_lowercase());
        } else if ch.is_whitespace() || matches!(ch, '-' | '_') {
            pending_dash = true;
        }
    }
    slug
}

fn html_anchor_id(html: &str) -> Option<&str> {
    let lower = html.to_ascii_lowercase();
    let anchor_start = lower.find("<a")?;
    let anchor = &html[anchor_start + 2..];
    let anchor_lower = &lower[anchor_start + 2..];
    if !anchor_lower
        .chars()
        .next()
        .is_some_and(|ch| ch.is_whitespace() || ch == '>')
    {
        return None;
    }
    for attribute in ["id", "name"] {
        let mut offset = 0;
        while let Some(found) = anchor_lower[offset..].find(attribute) {
            let start = offset + found;
            let before_ok = start == 0
                || anchor_lower[..start]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace);
            let after = start + attribute.len();
            let after_ok = anchor_lower[after..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_whitespace() || ch == '=');
            if before_ok && after_ok {
                let rest = anchor[after..].trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let rest = rest.trim_start();
                    let quote = rest.chars().next()?;
                    if matches!(quote, '\'' | '"') {
                        let value = &rest[quote.len_utf8()..];
                        return value.split_once(quote).map(|(id, _)| id);
                    }
                }
            }
            offset = after;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_structured_markdown_links_without_regex_routing() {
        let links = extract_doc_links(
            "Read [the **provider** guide](../config.md#provider-choices) and [web](https://example.com).",
        );
        assert_eq!(
            links,
            vec![
                DocLink {
                    label: "the provider guide".to_string(),
                    destination: "../config.md#provider-choices".to_string(),
                },
                DocLink {
                    label: "web".to_string(),
                    destination: "https://example.com".to_string(),
                },
            ]
        );
    }

    #[test]
    fn resolves_generated_and_explicit_heading_anchors_to_source_offsets() {
        let source = "# Home\n\nIntro.\n\n## Provider Choices\n\nBody.\n\n## Recovery {#safe-recovery}\n\n<a id=\"telegram\"></a>\n\n## Telegram Connector\n";
        assert_eq!(
            heading_source_offset(source, "provider-choices"),
            source.find("## Provider Choices")
        );
        assert_eq!(
            heading_source_offset(source, "safe-recovery"),
            source.find("## Recovery")
        );
        assert_eq!(
            heading_source_offset(source, "telegram"),
            source.find("<a id=\"telegram\"></a>")
        );
        assert_eq!(heading_source_offset(source, "missing"), None);
    }

    #[test]
    fn parses_html_anchor_id_and_name_attributes_without_accepting_substrings() {
        assert_eq!(html_anchor_id("<a id=\"telegram\"></a>"), Some("telegram"));
        assert_eq!(html_anchor_id("<a name='recovery'></a>"), Some("recovery"));
        assert_eq!(html_anchor_id("<aside data-id=\"wrong\">"), None);
        assert_eq!(html_anchor_id("<aside id=\"wrong\">"), None);
    }
}

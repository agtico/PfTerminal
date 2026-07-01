use std::io::Result;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
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
use ratatui::widgets::WidgetRef;

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
            TuiEvent::Draw | TuiEvent::Resize => {
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
        if self.search_active {
            self.handle_search_key_event(key_event);
            tui.frame_requester()
                .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
            return;
        }

        if self.pager_keymap.close.is_pressed(key_event) {
            self.is_done = true;
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
        let query = self.search_query.trim().to_ascii_lowercase();
        self.visible_indices = self
            .site
            .pages
            .iter()
            .enumerate()
            .filter_map(|(index, page)| {
                let path = page.rel_path.to_string_lossy().to_ascii_lowercase();
                (query.is_empty() || path.contains(&query)).then_some(index)
            })
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
        match self.site.read_page_source(self.selected_index) {
            Ok(source) => {
                self.page_source = source;
                self.page_error = None;
            }
            Err(err) => {
                self.page_source.clear();
                self.page_error = Some(err.to_string());
            }
        }
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
        self.render_page_index(list_area, buf);
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
            .render_ref(row, buf);
        let title = format!(
            "/ {}  {}",
            self.site.overlay_title(),
            self.site.config_path.display()
        );
        Span::from(fit_text(&title, row.width))
            .dim()
            .render_ref(row, buf);
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
            .render_ref(Rect::new(area.x, area.y, area.width, 1), buf);

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
            Span::styled(fit_text(&text, area.width), style).render_ref(rect, buf);
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
            Paragraph::new("No page selected.").render_ref(area, buf);
            return;
        };

        let title = format!("{}", page.rel_path.display());
        let title_style = if self.focus == MkDocsFocus::Page && !self.search_active {
            Style::default().bold().reversed()
        } else {
            Style::default().bold()
        };
        Span::styled(fit_text(&title, area.width), title_style)
            .render_ref(Rect::new(area.x, area.y, area.width, 1), buf);

        let docs_dir = format!("docs: {}", self.site.docs_dir.display());
        Span::from(fit_text(&docs_dir, area.width))
            .dim()
            .render_ref(
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
        Paragraph::new(Text::from(visible)).render_ref(content_area, buf);

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
        let status = if self.search_active {
            format!(
                " search: {}  Enter accept  Esc clear/exit search  q close ",
                self.search_query
            )
        } else if self.focus == MkDocsFocus::Page {
            "page: up/down/j/k scroll  Ctrl-d/u half  Ctrl-f/b page  Esc/Left index  q close "
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
            .render_ref(area, buf);
        Span::from(fit_text(&status, area.width))
            .dim()
            .render_ref(area, buf);
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

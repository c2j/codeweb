use crate::graph::key::NodeKey;
use crate::graph::traverse;
use crate::graph::Node;
use crate::project::Project;
use petgraph::graph::NodeIndex;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

const LOCALES: &[&str] = &["zh-CN", "en"];

pub struct App {
    project: Project,
    screen: Screen,
    should_quit: bool,

    // Explorer: search + node list
    search_query: String,
    search_mode: bool,
    sql_search_mode: bool,
    nodes: Vec<NodeIndex>,
    list_state: ListState,

    // Detail view
    detail_node_idx: Option<NodeIndex>,
    detail_lines: Vec<Line<'static>>,
    detail_scroll: u16,

    // Info scroll
    info_scroll: u16,

    // Options
    chain_style: traverse::ChainStyle,
    show_attributes: bool,
    filter_low_degree: bool,
    filter_threshold: usize,
    locale_idx: usize,
    show_chain_files: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Explorer,
    Detail,
    Info,
}

impl App {
    pub fn new(project: Project) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        let mut app = Self {
            project,
            screen: Screen::Explorer,
            should_quit: false,
            search_query: String::new(),
            search_mode: false,
            sql_search_mode: false,
            nodes: Vec::new(),
            list_state,
            detail_node_idx: None,
            detail_lines: Vec::new(),
            detail_scroll: 0,
            info_scroll: 0,
            chain_style: traverse::ChainStyle::default(),
            show_attributes: true,
            filter_low_degree: false,
            filter_threshold: 0,
            locale_idx: 0,
            show_chain_files: false,
        };
        rust_i18n::set_locale(LOCALES[0]);
        app.refresh_node_list();
        app
    }

    pub fn run<B: ratatui::backend::Backend>(
        mut self,
        terminal: &mut ratatui::Terminal<B>,
    ) -> crate::error::Result<()> {
        while !self.should_quit {
            terminal.draw(|f| self.draw(f)).map_err(|e| {
                crate::error::CodeWebError::ExportError {
                    message: e.to_string(),
                }
            })?;

            if crossterm::event::poll(std::time::Duration::from_millis(100)).map_err(|e| {
                crate::error::CodeWebError::ExportError {
                    message: e.to_string(),
                }
            })? {
                if let crossterm::event::Event::Key(key) =
                    crossterm::event::read().map_err(|e| {
                        crate::error::CodeWebError::ExportError {
                            message: e.to_string(),
                        }
                    })?
                {
                    self.handle_key(key);
                }
            }
        }
        Ok(())
    }

    // ── Node list management ──

    fn refresh_node_list(&mut self) {
        let Some(store) = self.project.store() else {
            return;
        };
        let graph = store.graph();
        if self.search_query.is_empty() {
            if self.filter_low_degree {
                self.nodes = traverse::low_degree_nodes(graph, self.filter_threshold)
                    .into_iter()
                    .map(|d| d.idx)
                    .collect();
            } else {
                self.nodes = graph.node_indices().collect();
            }
        } else if self.sql_search_mode {
            self.nodes = store
                .search_by_sql(&self.search_query)
                .into_iter()
                .map(|(idx, _)| idx)
                .collect();
        } else {
            self.nodes = traverse::find_nodes_by_name(graph, &self.search_query)
                .into_iter()
                .map(|(idx, _)| idx)
                .collect();
        }
        if !self.nodes.is_empty() {
            let current = self.list_state.selected().unwrap_or(0);
            self.list_state
                .select(Some(current.min(self.nodes.len() - 1)));
        }
    }

    fn selected_node(&self) -> Option<NodeIndex> {
        let i = self.list_state.selected()?;
        self.nodes.get(i).copied()
    }

    fn open_detail(&mut self, idx: NodeIndex) {
        let Some(store) = self.project.store() else {
            return;
        };
        let graph = store.graph();
        let node = &graph[idx];
        let key = NodeKey::from_node(node);
        let (tag, tag_color) = node_tag(node);

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<8} ", tag),
                Style::default().fg(tag_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                key.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]));

        let in_deg = graph
            .neighbors_directed(idx, petgraph::Direction::Incoming)
            .count();
        let out_deg = graph
            .neighbors_directed(idx, petgraph::Direction::Outgoing)
            .count();
        lines.push(Line::from(Span::styled(
            format!(
                "{}:{} {}:{} {}:{}",
                t!("degree.in"),
                in_deg,
                t!("degree.out"),
                out_deg,
                t!("degree.total"),
                in_deg + out_deg
            ),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));

        if self.show_attributes {
            let attr_lines = format_node_attributes_full(node);
            for attr_line in attr_lines {
                lines.push(Line::from(Span::styled(
                    attr_line,
                    Style::default().fg(Color::White),
                )));
            }
            lines.push(Line::from(""));
        }

        let (chain, _) = traverse::trace_chain(graph, idx, 50, usize::MAX);
        let chain_lines = match self.chain_style {
            traverse::ChainStyle::Tree => self.render_chain_tree_tui(&chain, graph, idx),
            traverse::ChainStyle::Path => {
                let text = traverse::format_chain(&chain, graph, traverse::ChainStyle::Path);
                text.lines().map(|l| Line::from(l.to_string())).collect()
            }
        };
        lines.extend(chain_lines);

        if self.show_chain_files {
            let chain_files = traverse::collect_chain_files(&chain, graph);
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("── {} ({}) ──", t!("section.files"), chain_files.len()),
                Style::default().fg(Color::Yellow),
            )));
            if chain_files.is_empty() {
                lines.push(Line::from(Span::styled(
                    t!("none").to_string(),
                    Style::default()
                        .fg(Color::Black)
                        .add_modifier(Modifier::DIM),
                )));
            } else {
                for (file, nodes) in &chain_files {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{:>3} ", nodes.len()),
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::raw(file.to_string_lossy().to_string()),
                    ]));
                    let display_nodes = nodes.iter().take(8);
                    for node_label in display_nodes {
                        lines.push(Line::from(Span::styled(
                            format!("      {}", node_label),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                    if nodes.len() > 8 {
                        lines.push(Line::from(Span::styled(
                            format!("      ... +{} more", nodes.len() - 8),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
            }
        }

        self.detail_node_idx = Some(idx);
        self.detail_lines = lines;
        self.detail_scroll = 0;
        self.screen = Screen::Detail;
    }

    // ── Key handling ──

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyEventKind;
        if key.kind != KeyEventKind::Press {
            return;
        }

        match self.screen {
            Screen::Explorer => self.handle_explorer_key(key.code),
            Screen::Detail => self.handle_detail_key(key.code),
            Screen::Info => self.handle_info_key(key.code),
        }
    }

    fn handle_explorer_key(&mut self, code: crossterm::event::KeyCode) {
        use crossterm::event::KeyCode;

        if self.search_mode {
            // Search mode: all keys go to search query
            match code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.search_mode = false;
                }
                KeyCode::Char('/') => {
                    self.sql_search_mode = !self.sql_search_mode;
                    self.search_query.clear();
                    self.list_state.select(Some(0));
                    self.refresh_node_list();
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                    self.list_state.select(Some(0));
                    self.refresh_node_list();
                }
                KeyCode::Char(c) => {
                    self.search_query.push(c);
                    self.list_state.select(Some(0));
                    self.refresh_node_list();
                }
                _ => {}
            }
            return;
        }

        // Command mode: single-key shortcuts
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('/') => {
                self.search_mode = true;
            }
            KeyCode::Char('2') | KeyCode::Char('i') => {
                self.screen = Screen::Info;
                self.info_scroll = 0;
            }
            KeyCode::Char('a') => {
                let _ = self.project.analyze();
                self.refresh_node_list();
            }
            KeyCode::Char('s') => {
                self.chain_style = match self.chain_style {
                    traverse::ChainStyle::Tree => traverse::ChainStyle::Path,
                    traverse::ChainStyle::Path => traverse::ChainStyle::Tree,
                };
            }
            KeyCode::Char('v') => {
                self.show_attributes = !self.show_attributes;
            }
            KeyCode::Char('l') => {
                self.filter_low_degree = !self.filter_low_degree;
                self.list_state.select(Some(0));
                self.refresh_node_list();
            }
            KeyCode::Char('\\') => {
                self.locale_idx = (self.locale_idx + 1) % LOCALES.len();
                rust_i18n::set_locale(LOCALES[self.locale_idx]);
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.filter_threshold = self.filter_threshold.saturating_add(1).min(100);
                self.list_state.select(Some(0));
                self.refresh_node_list();
            }
            KeyCode::Char('-') => {
                self.filter_threshold = self.filter_threshold.saturating_sub(1);
                self.list_state.select(Some(0));
                self.refresh_node_list();
            }
            KeyCode::Enter => {
                if let Some(idx) = self.selected_node() {
                    self.open_detail(idx);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = self.list_state.selected().unwrap_or(0);
                let max = self.nodes.len().saturating_sub(1);
                self.list_state.select(Some((i + 1).min(max)));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = self.list_state.selected().unwrap_or(0);
                self.list_state.select(Some(i.saturating_sub(1)));
            }
            _ => {}
        }
    }

    fn handle_detail_key(&mut self, code: crossterm::event::KeyCode) {
        use crossterm::event::KeyCode;
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Backspace => {
                self.screen = Screen::Explorer;
                self.detail_lines.clear();
                self.detail_scroll = 0;
                self.detail_node_idx = None;
            }
            KeyCode::Char('s') => {
                self.chain_style = match self.chain_style {
                    traverse::ChainStyle::Tree => traverse::ChainStyle::Path,
                    traverse::ChainStyle::Path => traverse::ChainStyle::Tree,
                };
                if let Some(idx) = self.detail_node_idx {
                    self.open_detail(idx);
                }
            }
            KeyCode::Char('f') => {
                self.show_chain_files = !self.show_chain_files;
                if let Some(idx) = self.detail_node_idx {
                    self.open_detail(idx);
                }
            }
            KeyCode::Char('\\') => {
                self.locale_idx = (self.locale_idx + 1) % LOCALES.len();
                rust_i18n::set_locale(LOCALES[self.locale_idx]);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.detail_scroll = self.detail_scroll.saturating_add(1);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn handle_info_key(&mut self, code: crossterm::event::KeyCode) {
        use crossterm::event::KeyCode;
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc | KeyCode::Char('1') => {
                self.screen = Screen::Explorer;
                self.info_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.info_scroll = self.info_scroll.saturating_add(1);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.info_scroll = self.info_scroll.saturating_sub(1);
            }
            KeyCode::Char('\\') => {
                self.locale_idx = (self.locale_idx + 1) % LOCALES.len();
                rust_i18n::set_locale(LOCALES[self.locale_idx]);
            }
            _ => {}
        }
    }

    // ── Drawing ──

    fn draw(&self, f: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .split(f.area());

        self.draw_title_bar(f, chunks[0]);

        match self.screen {
            Screen::Explorer => self.draw_explorer(f, chunks[1]),
            Screen::Detail => self.draw_detail(f, chunks[1]),
            Screen::Info => self.draw_info(f, chunks[1]),
        }

        self.draw_status_bar(f, chunks[2]);
    }

    fn draw_title_bar(&self, f: &mut Frame, area: Rect) {
        let title = format!(
            " codeweb ─ {} ─ {} ",
            self.project.name(),
            match self.screen {
                Screen::Explorer => t!("screen.explorer").to_string(),
                Screen::Detail => t!("screen.detail").to_string(),
                Screen::Info => t!("screen.info").to_string(),
            }
        );
        let bar = Paragraph::new(title).style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
        f.render_widget(bar, area);
    }

    fn draw_status_bar(&self, f: &mut Frame, area: Rect) {
        let style_label = match self.chain_style {
            traverse::ChainStyle::Tree => t!("style.tree").to_string(),
            traverse::ChainStyle::Path => t!("style.path").to_string(),
        };
        let attr_indicator = if self.show_attributes { "Attr" } else { "attr" };

        let hints: String = if self.search_mode && self.screen == Screen::Explorer {
            t!("statusbar.explorer_search",
                search => t!("hint.search_exit"),
                back => t!("hint.back"),
            )
            .to_string()
        } else {
            match self.screen {
                Screen::Explorer => {
                    if self.filter_low_degree {
                        t!("statusbar.explorer_filter_on",
                            nav => t!("hint.nav"),
                            full => t!("hint.full"),
                            style => t!("hint.style"),
                            style_val => &style_label,
                            filter_on => t!("hint.filter_on"),
                            search => t!("hint.search"),
                            info => t!("hint.info"),
                            lang => t!("hint.lang"),
                            quit => t!("hint.quit"),
                        )
                        .to_string()
                    } else {
                        t!("statusbar.explorer_filter_off",
                            nav => t!("hint.nav"),
                            full => t!("hint.full"),
                            style => t!("hint.style"),
                            style_val => &style_label,
                            filter_off => t!("hint.filter_off"),
                            search => t!("hint.search"),
                            info => t!("hint.info"),
                            lang => t!("hint.lang"),
                            quit => t!("hint.quit"),
                        )
                        .to_string()
                    }
                }
                Screen::Detail => t!("statusbar.detail",
                    scroll => t!("hint.scroll"),
                    back => t!("hint.back"),
                    style => t!("hint.style"),
                    style_val => &style_label,
                    files => t!("hint.files"),
                    lang => t!("hint.lang"),
                    quit => t!("hint.quit"),
                )
                .to_string(),
                Screen::Info => t!("statusbar.info",
                    scroll => t!("hint.scroll"),
                    back => t!("hint.back"),
                    lang => t!("hint.lang"),
                    quit => t!("hint.quit"),
                )
                .to_string(),
            }
        };
        let bar = Paragraph::new(format!(" {}  [{}]", hints, attr_indicator))
            .style(Style::default().bg(Color::DarkGray).fg(Color::White));
        f.render_widget(bar, area);
    }

    fn draw_explorer(&self, f: &mut Frame, area: Rect) {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);

        // Search bar
        let mode_tag = if self.sql_search_mode {
            "[SQL] "
        } else {
            "[Name] "
        };
        let cursor = if self.search_mode { "▏" } else { "_" };
        let prompt = if self.search_mode {
            format!("> {}{}{}", mode_tag, self.search_query, cursor)
        } else if self.search_query.is_empty() {
            format!("> {}{}", mode_tag, t!("hint.search_hint"))
        } else {
            format!("> {}{}{}", mode_tag, self.search_query, cursor)
        };
        let border_style = if self.search_mode {
            if self.sql_search_mode {
                Style::default().fg(Color::Magenta)
            } else {
                Style::default().fg(Color::Cyan)
            }
        } else {
            Style::default()
        };
        let search = Paragraph::new(prompt).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", t!("panel.search")))
                .border_style(border_style),
        );
        f.render_widget(search, outer[0]);

        // Split panel: nodes | preview
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(outer[1]);

        // Node list
        let Some(store) = self.project.store() else {
            return;
        };
        let graph = store.graph();
        let degree_map: std::collections::HashMap<NodeIndex, (usize, usize)> =
            if self.filter_low_degree {
                traverse::low_degree_nodes(graph, self.filter_threshold)
                    .into_iter()
                    .map(|d| (d.idx, (d.in_degree, d.out_degree)))
                    .collect()
            } else {
                std::collections::HashMap::new()
            };
        let items: Vec<ListItem> = self
            .nodes
            .iter()
            .map(|idx| {
                let node = &graph[*idx];
                let (tag, color) = node_tag(node);
                let key = NodeKey::from_node(node);
                let mut spans = vec![
                    Span::styled(format!("{:<8} ", tag), Style::default().fg(color)),
                    Span::raw(key.to_string()),
                ];
                if let Some((in_deg, out_deg)) = degree_map.get(idx) {
                    let in_label = t!("degree.in").to_string();
                    let out_label = t!("degree.out").to_string();
                    spans.push(Span::styled(
                        format!(" [{}:{} {}:{}]", in_label, in_deg, out_label, out_deg),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        let node_count = self.nodes.len();
        let list_title = if self.search_query.is_empty() {
            format!(" {} ({}) ", t!("panel.nodes"), node_count)
        } else {
            format!(
                " {} ",
                t!("panel.nodes_matched", matched => node_count, total => graph.node_count())
            )
        };
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(list_title))
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
        f.render_stateful_widget(list, panels[0], &mut self.list_state.clone());

        // Preview panel: show call chain for selected node
        if let Some(idx) = self.selected_node() {
            let (chain, _) = traverse::trace_chain(graph, idx, 50, usize::MAX);
            let preview_lines = match self.chain_style {
                traverse::ChainStyle::Tree => self.render_chain_tree_tui(&chain, graph, idx),
                traverse::ChainStyle::Path => {
                    let text = traverse::format_chain(&chain, graph, traverse::ChainStyle::Path);
                    text.lines().map(|l| Line::from(l.to_string())).collect()
                }
            };
            let para = Paragraph::new(preview_lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} ", t!("panel.preview"))),
                )
                .wrap(Wrap { trim: false });
            f.render_widget(para, panels[1]);
        } else {
            let para = Paragraph::new(t!("select_node").to_string()).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", t!("panel.preview"))),
            );
            f.render_widget(para, panels[1]);
        }
    }

    fn draw_detail(&self, f: &mut Frame, area: Rect) {
        let para = Paragraph::new(self.detail_lines.clone())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", t!("panel.detail"))),
            )
            .wrap(Wrap { trim: false })
            .scroll((self.detail_scroll, 0));
        f.render_widget(para, area);
    }

    fn draw_info(&self, f: &mut Frame, area: Rect) {
        let Some(store) = self.project.store() else {
            return;
        };
        let stats = store.stats();

        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);

        // Stats
        let stats_lines = vec![
            Line::from(vec![
                Span::styled(
                    format!("{}: ", t!("stat.procedures")),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{}", stats.procedures)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", t!("stat.functions")),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{}", stats.functions)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", t!("stat.tables")),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{}", stats.tables)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", t!("stat.views")),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{}", stats.views)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", t!("stat.mappers")),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{}", stats.mappers)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", t!("stat.java_methods")),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{}", stats.java_methods)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", t!("stat.edges")),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{}", stats.edges)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", t!("stat.files")),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{}", stats.files)),
            ]),
        ];
        let stats_para = Paragraph::new(stats_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", t!("panel.stats"))),
            )
            .scroll((self.info_scroll, 0));
        f.render_widget(stats_para, panels[0]);

        // Files
        let file_nodes = store.file_nodes();
        let manifest = store.manifest();
        let mut entries: Vec<_> = manifest.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let root = self.project.root();
        let file_items: Vec<ListItem> = entries
            .iter()
            .map(|(path, record)| {
                let rel = path.strip_prefix(root).unwrap_or(path);
                let type_tag = match record.file_type {
                    crate::parser::fingerprint::FileType::Sql => t!("filetype.sql").to_string(),
                    crate::parser::fingerprint::FileType::Java => t!("filetype.java").to_string(),
                    crate::parser::fingerprint::FileType::Xml => t!("filetype.xml").to_string(),
                };
                let node_count = file_nodes
                    .get(path as &std::path::Path)
                    .map(|v| v.len())
                    .unwrap_or(0);
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:<4} ", type_tag),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("{:>3} ", node_count),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(rel.to_string_lossy().to_string()),
                ]))
            })
            .collect();
        let file_list =
            List::new(file_items).block(Block::default().borders(Borders::ALL).title(format!(
                " {} ({}) ",
                t!("panel.files"),
                manifest.len()
            )));
        f.render_widget(file_list, panels[1]);
    }

    // ── Chain tree rendering (kept from original) ──

    fn render_chain_tree_tui(
        &self,
        chain: &traverse::CallChain,
        graph: &crate::graph::CodeGraph,
        node_idx: NodeIndex,
    ) -> Vec<Line<'static>> {
        let mut lines: Vec<Line> = Vec::new();

        lines.push(Line::from(Span::styled(
            t!("callers").to_string(),
            Style::default().fg(Color::Cyan),
        )));
        if chain.callers.is_empty() {
            lines.push(Line::from(Span::styled(
                t!("none").to_string(),
                Style::default()
                    .fg(Color::Black)
                    .add_modifier(Modifier::DIM),
            )));
        } else {
            for (i, caller) in chain.callers.iter().enumerate() {
                let is_last = i == chain.callers.len() - 1;
                tree_node_to_lines(caller, graph, "  ", is_last, &mut lines);
            }
        }

        lines.push(Line::from(""));
        let target_key = NodeKey::from_node(&graph[node_idx]);
        let (target_tag, target_color) = node_tag(&graph[node_idx]);
        lines.push(Line::from(vec![
            Span::styled(
                "  ▶ ",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<8} ", target_tag),
                Style::default()
                    .fg(target_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                target_key.to_string(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        let attr_lines = format_node_attributes(&graph[node_idx]);
        for attr_line in attr_lines {
            lines.push(Line::from(Span::styled(
                format!("  {}", attr_line),
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            t!("callees").to_string(),
            Style::default().fg(Color::Green),
        )));
        if chain.callees.is_empty() {
            lines.push(Line::from(Span::styled(
                t!("none").to_string(),
                Style::default()
                    .fg(Color::Black)
                    .add_modifier(Modifier::DIM),
            )));
        } else {
            for (i, callee) in chain.callees.iter().enumerate() {
                let is_last = i == chain.callees.len() - 1;
                tree_node_to_lines(callee, graph, "  ", is_last, &mut lines);
            }
        }

        lines
    }
}

// ── Helper functions (kept unchanged) ──

fn node_tag(node: &Node) -> (std::borrow::Cow<'static, str>, Color) {
    match node {
        Node::Procedure { .. } => (std::borrow::Cow::Borrowed("proc"), Color::Green),
        Node::Function { .. } => (std::borrow::Cow::Borrowed("func"), Color::LightGreen),
        Node::Unresolved { .. } => (std::borrow::Cow::Borrowed("unres"), Color::Red),
        Node::MappedStatement { .. } => (std::borrow::Cow::Borrowed("mapper"), Color::Blue),
        Node::JavaSql { .. } => (std::borrow::Cow::Borrowed("sql"), Color::Magenta),
        Node::JavaMethod { .. } => (std::borrow::Cow::Borrowed("method"), Color::Cyan),
        Node::JavaClass { .. } => (std::borrow::Cow::Borrowed("class"), Color::Rgb(180, 100, 0)),
        Node::Table { .. } => (std::borrow::Cow::Borrowed("table"), Color::Rgb(180, 100, 0)),
        Node::View { .. } => (std::borrow::Cow::Borrowed("view"), Color::Blue),
        Node::Package { .. } => (std::borrow::Cow::Borrowed("pkg"), Color::Yellow),
        Node::Trigger { .. } => (std::borrow::Cow::Borrowed("trigger"), Color::Red),
        Node::Type { .. } => (std::borrow::Cow::Borrowed("type"), Color::Yellow),
        Node::Sequence { .. } => (std::borrow::Cow::Borrowed("seq"), Color::LightGreen),
        Node::Index { .. } => (std::borrow::Cow::Borrowed("index"), Color::Gray),
        Node::MaterializedView { .. } => (std::borrow::Cow::Borrowed("mview"), Color::Cyan),
        Node::Synonym { .. } => (std::borrow::Cow::Borrowed("synonym"), Color::Magenta),
        Node::Event { .. } => (std::borrow::Cow::Borrowed("event"), Color::LightRed),
        Node::Custom { type_name, .. } => (
            std::borrow::Cow::Owned((**type_name).clone()),
            Color::DarkGray,
        ),
    }
}

fn tree_node_to_lines(
    node: &crate::graph::traverse::TreeNode,
    graph: &crate::graph::CodeGraph,
    prefix: &str,
    is_last: bool,
    lines: &mut Vec<Line>,
) {
    let connector = if is_last { "└── " } else { "├── " };
    let (tag, color) = node_tag(&graph[node.idx]);
    let key = NodeKey::from_node(&graph[node.idx]);
    lines.push(Line::from(vec![
        Span::raw(prefix.to_string()),
        Span::styled(
            connector.to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(format!("{:<8} ", tag), Style::default().fg(color)),
        Span::styled(key.to_string(), Style::default().fg(Color::Black)),
    ]));

    let child_prefix = if is_last {
        format!("{}    ", prefix)
    } else {
        format!("{}│   ", prefix)
    };
    for (i, child) in node.children.iter().enumerate() {
        let child_last = i == node.children.len() - 1;
        tree_node_to_lines(child, graph, &child_prefix, child_last, lines);
    }
}

fn format_node_attributes(node: &Node) -> Vec<String> {
    format_node_attributes_impl(node, true)
}

fn format_node_attributes_full(node: &Node) -> Vec<String> {
    format_node_attributes_impl(node, false)
}

fn format_node_attributes_impl(node: &Node, compact: bool) -> Vec<String> {
    use crate::graph::{DistributeInfo, PartitionInfo};
    let mut attrs = Vec::new();
    match node {
        Node::Table {
            location,
            columns,
            partition_by,
            distribute_by,
            tablespace,
            temporary,
            unlogged,
            ddl_source,
            ..
        } => {
            if let Some(loc) = location {
                attrs.push(format!("file: {}:{}", loc.file.to_string_lossy(), loc.line));
            } else {
                attrs.push("file: (implicit)".to_string());
            }
            if *temporary {
                attrs.push("temporary".to_string());
            }
            if *unlogged {
                attrs.push("unlogged".to_string());
            }
            if let Some(ts) = tablespace {
                attrs.push(format!("tablespace: {}", ts));
            }
            if !columns.is_empty() {
                attrs.push(format!("columns ({}):", columns.len()));
                let display_cols = if compact {
                    columns.iter().take(5)
                } else {
                    columns.iter().take(50)
                };
                for col in display_cols {
                    let pk = if col.is_primary_key { " [PK]" } else { "" };
                    let null = if col.nullable { "NULL" } else { "NOT NULL" };
                    let def = col
                        .default_value
                        .as_deref()
                        .map(|d| format!(" DEFAULT {}", d))
                        .unwrap_or_default();
                    attrs.push(format!(
                        "  {} {} {}{}{}",
                        col.name, col.data_type, null, pk, def
                    ));
                }
                if columns.len() > 5 && compact {
                    attrs.push(format!("  ... +{} more", columns.len() - 5));
                }
            }
            if let Some(part) = partition_by {
                match part.as_ref() {
                    PartitionInfo::Range {
                        columns,
                        partitions,
                    } => {
                        attrs.push(format!(
                            "partition: RANGE({}) [{} partitions]",
                            columns.join(", "),
                            partitions.len()
                        ));
                        if !compact && !partitions.is_empty() {
                            for p in partitions {
                                attrs.push(format!("  {}", p));
                            }
                        }
                    }
                    PartitionInfo::List {
                        columns,
                        partitions,
                    } => {
                        attrs.push(format!(
                            "partition: LIST({}) [{} partitions]",
                            columns.join(", "),
                            partitions.len()
                        ));
                        if !compact && !partitions.is_empty() {
                            for p in partitions {
                                attrs.push(format!("  {}", p));
                            }
                        }
                    }
                    PartitionInfo::Hash {
                        columns,
                        partitions_count,
                    } => {
                        attrs.push(format!(
                            "partition: HASH({}) [{}]",
                            columns.join(", "),
                            partitions_count
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "auto".to_string())
                        ));
                    }
                }
            }
            if let Some(dist) = distribute_by {
                match dist.as_ref() {
                    DistributeInfo::Hash { columns } => {
                        attrs.push(format!("distribute: HASH({})", columns.join(", ")));
                    }
                    DistributeInfo::Replication => {
                        attrs.push("distribute: REPLICATION".to_string());
                    }
                    DistributeInfo::RoundRobin { columns } => {
                        attrs.push(format!("distribute: ROUNDROBIN({})", columns.join(", ")));
                    }
                    DistributeInfo::Modulo { columns } => {
                        attrs.push(format!("distribute: MODULO({})", columns.join(", ")));
                    }
                }
            }
            if let Some(ddl) = ddl_source {
                attrs.push(format!("ddl: {}", ddl.as_ref()));
            }
        }
        Node::JavaSql {
            class_name,
            method_name,
            extraction_method,
            java_file,
            line,
            sql,
            ..
        } => {
            attrs.push(format!("file: {}:{}", java_file.to_string_lossy(), line));
            if let (Some(c), Some(m)) = (class_name, method_name) {
                attrs.push(format!("method: {}.{}", c, m));
            } else if let Some(c) = class_name {
                attrs.push(format!("class: {}", c));
            } else if let Some(m) = method_name {
                attrs.push(format!("method: {}", m));
            }
            attrs.push(format!("extraction: {}", extraction_method));
            if let Some(sql_text) = sql {
                let line_limit = if compact { 3 } else { 20 };
                for (i, line) in sql_text.lines().enumerate() {
                    if i >= line_limit {
                        attrs.push(format!(
                            "  ... +{} more lines",
                            sql_text.lines().count() - i
                        ));
                        break;
                    }
                    attrs.push(format!("  {}", line));
                }
            }
        }
        Node::MappedStatement {
            kind,
            xml_file,
            line,
            sql,
            ..
        } => {
            attrs.push(format!("file: {}:{}", xml_file.to_string_lossy(), line));
            attrs.push(format!("kind: {}", kind));
            if let Some(sql_text) = sql {
                let line_limit = if compact { 3 } else { 20 };
                for (i, line) in sql_text.lines().enumerate() {
                    if i >= line_limit {
                        attrs.push(format!(
                            "  ... +{} more lines",
                            sql_text.lines().count() - i
                        ));
                        break;
                    }
                    attrs.push(format!("  {}", line));
                }
            }
        }
        _ => {}
    }
    attrs
}

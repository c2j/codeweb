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

pub struct App {
    project: Project,
    screen: Screen,
    list_state: ListState,
    should_quit: bool,
    search_query: String,
    search_results: Vec<(NodeIndex, String)>,
    search_selected: usize,
    chain_display: Vec<Line<'static>>,
    in_search: bool,
    filter_low_degree: bool,
    filter_threshold: usize,
    detail_node_idx: Option<NodeIndex>,
    chain_style: traverse::ChainStyle,
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Dashboard,
    Files,
    Graph,
    Trace,
    Detail,
}

impl App {
    pub fn new(project: Project) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            project,
            screen: Screen::Dashboard,
            list_state,
            should_quit: false,
            search_query: String::new(),
            search_results: Vec::new(),
            search_selected: 0,
            chain_display: Vec::new(),
            in_search: true,
            filter_low_degree: false,
            filter_threshold: 0,
            detail_node_idx: None,
            chain_style: traverse::ChainStyle::default(),
        }
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

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyEventKind;

        if key.kind != KeyEventKind::Press {
            return;
        }

        use crossterm::event::KeyCode;
        if key.code == KeyCode::Char('s') && !self.in_search {
            self.chain_style = match self.chain_style {
                traverse::ChainStyle::Tree => traverse::ChainStyle::Path,
                traverse::ChainStyle::Path => traverse::ChainStyle::Tree,
            };
            self.refresh_chain_display();
            return;
        }

        match self.screen {
            Screen::Trace => self.handle_trace_key(key),
            Screen::Graph => self.handle_graph_key(key),
            Screen::Detail => self.handle_detail_key(key),
            _ => self.handle_global_key(key),
        }
    }

    fn handle_trace_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEventKind;
        if key.kind != KeyEventKind::Press {
            return;
        }

        if self.in_search {
            match key.code {
                KeyCode::Backspace => {
                    self.search_query.pop();
                    self.update_search_results();
                    self.search_selected = 0;
                }
                KeyCode::Enter => {
                    if !self.search_results.is_empty() {
                        let (idx, _) = self.search_results[self.search_selected];
                        self.run_trace(idx);
                        self.in_search = false;
                    }
                }
                KeyCode::Down => {
                    if !self.search_results.is_empty() {
                        self.search_selected =
                            (self.search_selected + 1).min(self.search_results.len() - 1);
                    }
                }
                KeyCode::Up => {
                    self.search_selected = self.search_selected.saturating_sub(1);
                }
                KeyCode::Esc => {
                    self.in_search = true;
                    self.chain_display.clear();
                }
                KeyCode::Char('1') | KeyCode::Char('d') => {
                    self.screen = Screen::Dashboard;
                    self.list_state.select(Some(0));
                }
                KeyCode::Char('2') | KeyCode::Char('f') => {
                    self.screen = Screen::Files;
                    self.list_state.select(Some(0));
                }
                KeyCode::Char('3') | KeyCode::Char('g') => {
                    self.screen = Screen::Graph;
                    self.list_state.select(Some(0));
                }
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char(c) => {
                    self.search_query.push(c);
                    self.update_search_results();
                    self.search_selected = 0;
                }
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Esc => {
                    self.in_search = true;
                    self.chain_display.clear();
                }
                KeyCode::Char('1') | KeyCode::Char('d') => {
                    self.screen = Screen::Dashboard;
                    self.list_state.select(Some(0));
                }
                KeyCode::Char('2') | KeyCode::Char('f') => {
                    self.screen = Screen::Files;
                    self.list_state.select(Some(0));
                }
                KeyCode::Char('3') | KeyCode::Char('g') => {
                    self.screen = Screen::Graph;
                    self.list_state.select(Some(0));
                }
                KeyCode::Char('q') => self.should_quit = true,
                _ => {}
            }
        }
    }

    fn handle_graph_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyEventKind};
        if key.kind != KeyEventKind::Press {
            return;
        }

        match key.code {
            KeyCode::Char('l') => {
                self.filter_low_degree = !self.filter_low_degree;
                self.list_state.select(Some(0));
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.filter_threshold = self.filter_threshold.saturating_add(1).min(100);
                self.list_state.select(Some(0));
            }
            KeyCode::Char('-') => {
                self.filter_threshold = self.filter_threshold.saturating_sub(1);
                self.list_state.select(Some(0));
            }
            KeyCode::Enter => {
                self.detail_node_idx = self.resolve_graph_selected_node();
                if self.detail_node_idx.is_some() {
                    self.screen = Screen::Detail;
                }
            }
            _ => self.handle_global_key(key),
        }
    }

    fn handle_detail_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyEventKind};
        if key.kind != KeyEventKind::Press {
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.screen = Screen::Graph;
                self.detail_node_idx = None;
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('1') | KeyCode::Char('d') => {
                self.screen = Screen::Dashboard;
                self.list_state.select(Some(0));
                self.detail_node_idx = None;
            }
            KeyCode::Char('2') | KeyCode::Char('f') => {
                self.screen = Screen::Files;
                self.list_state.select(Some(0));
                self.detail_node_idx = None;
            }
            KeyCode::Char('4') | KeyCode::Char('t') => {
                self.screen = Screen::Trace;
                self.detail_node_idx = None;
            }
            _ => {}
        }
    }

    fn handle_global_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyEventKind};
        if key.kind != KeyEventKind::Press {
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('1') | KeyCode::Char('d') => {
                self.screen = Screen::Dashboard;
                self.list_state.select(Some(0));
            }
            KeyCode::Char('2') | KeyCode::Char('f') => {
                self.screen = Screen::Files;
                self.list_state.select(Some(0));
            }
            KeyCode::Char('3') | KeyCode::Char('g') => {
                self.screen = Screen::Graph;
                self.list_state.select(Some(0));
            }
            KeyCode::Char('4') | KeyCode::Char('t') => {
                self.screen = Screen::Trace;
            }
            KeyCode::Char('a') => {
                let _ = self.project.analyze();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = self.list_state.selected().unwrap_or(0);
                let max = self.list_len().saturating_sub(1);
                self.list_state.select(Some((i + 1).min(max)));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = self.list_state.selected().unwrap_or(0);
                self.list_state.select(Some(i.saturating_sub(1)));
            }
            _ => {}
        }
    }

    fn update_search_results(&mut self) {
        if let Some(store) = self.project.store() {
            self.search_results = traverse::find_nodes_by_name(store.graph(), &self.search_query);
        }
    }

    fn resolve_graph_selected_node(&self) -> Option<NodeIndex> {
        let store = self.project.store()?;
        let graph = store.graph();
        if self.filter_low_degree {
            let filtered = traverse::low_degree_nodes(graph, self.filter_threshold);
            self.list_state
                .selected()
                .and_then(|s| filtered.into_iter().nth(s).map(|d| d.idx))
        } else {
            let indices: Vec<_> = graph.node_indices().collect();
            self.list_state
                .selected()
                .and_then(|s| indices.into_iter().nth(s))
        }
    }

    fn run_trace(&mut self, idx: NodeIndex) {
        if let Some(store) = self.project.store() {
            let chain = traverse::trace_chain(store.graph(), idx);
            let tree = traverse::format_chain(&chain, store.graph(), self.chain_style);
            self.chain_display = tree.lines().map(|l| Line::from(l.to_string())).collect();
        }
    }

    fn refresh_chain_display(&mut self) {
        if self.screen == Screen::Trace && !self.in_search
            && !self.search_results.is_empty()
            && self.search_selected < self.search_results.len()
        {
            let (idx, _) = self.search_results[self.search_selected];
            self.run_trace(idx);
        }
    }

    fn render_chain_tree_tui(
        &self,
        chain: &traverse::CallChain,
        graph: &crate::graph::CodeGraph,
        node_idx: NodeIndex,
    ) -> Vec<Line<'static>> {
        let mut lines: Vec<Line> = Vec::new();

        lines.push(Line::from(Span::styled(
            "── CALLERS ──",
            Style::default().fg(Color::Cyan),
        )));
        if chain.callers.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (none)",
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
            "── CALLEES ──",
            Style::default().fg(Color::Green),
        )));
        if chain.callees.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (none)",
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

    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Dashboard => 1,
            Screen::Files => self
                .project
                .store()
                .map(|s| s.manifest().len())
                .unwrap_or(0),
            Screen::Graph => {
                if self.filter_low_degree {
                    if let Some(store) = self.project.store() {
                        return traverse::low_degree_nodes(store.graph(), self.filter_threshold)
                            .len();
                    }
                }
                self.project
                    .store()
                    .map(|s| s.graph().node_count())
                    .unwrap_or(0)
            }
            Screen::Trace => {
                if self.in_search {
                    self.search_results.len().max(1)
                } else {
                    self.chain_display.len().max(1)
                }
            }
            Screen::Detail => 1,
        }
    }

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
            Screen::Dashboard => self.draw_dashboard(f, chunks[1]),
            Screen::Files => self.draw_files(f, chunks[1]),
            Screen::Graph => self.draw_graph(f, chunks[1]),
            Screen::Trace => self.draw_trace(f, chunks[1]),
            Screen::Detail => self.draw_detail(f, chunks[1]),
        }

        self.draw_status_bar(f, chunks[2]);
    }

    fn draw_title_bar(&self, f: &mut Frame, area: Rect) {
        let title = format!(
            " codeweb ─ {} ─ {} ",
            self.project.name(),
            match self.screen {
                Screen::Dashboard => "Dashboard",
                Screen::Files => "Files",
                Screen::Graph => "Graph",
                Screen::Trace => "Trace",
                Screen::Detail => "Detail",
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
            traverse::ChainStyle::Tree => "Tree",
            traverse::ChainStyle::Path => "Path",
        };
        let hints: String = match self.screen {
            Screen::Dashboard => "[A]nalyze  [2]Files  [3]Graph  [4]Trace  [Q]uit".to_string(),
            Screen::Files => "[↑↓] Navigate  [1]Dashboard  [3]Graph  [4]Trace  [Q]uit".to_string(),
            Screen::Graph if !self.filter_low_degree => {
                "[↑↓] Navigate  [Enter]Detail  [L]Filter:OFF  [S]tyle  [1]Dashboard  [2]Files  [4]Trace  [Q]uit".to_string()
            }
            Screen::Graph => {
                "[↑↓] Navigate  [Enter]Detail  [L]Filter:ON [+/-]Threshold  [S]tyle  [1]Dashboard  [2]Files  [4]Trace  [Q]uit".to_string()
            }
            Screen::Trace if self.in_search => {
                "[Enter]Select  [Esc]Back  [1]Dashboard  [2]Files  [3]Graph  [Q]uit".to_string()
            }
            Screen::Trace => format!("[Esc]Back  [S]tyle:{}  [1]Dashboard  [2]Files  [3]Graph  [Q]uit", style_label),
            Screen::Detail => format!("[Esc]Back  [S]tyle:{}  [1]Dashboard  [2]Files  [4]Trace  [Q]uit", style_label),
        };
        let bar = Paragraph::new(format!(" {}", hints))
            .style(Style::default().bg(Color::DarkGray).fg(Color::White));
        f.render_widget(bar, area);
    }

    fn draw_dashboard(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        if let Some(store) = self.project.store() {
            let stats = store.stats();
            let lines = vec![
                Line::from(vec![
                    Span::styled("Project: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        self.project.name(),
                        Style::default()
                            .fg(Color::Black)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Procedures: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}", stats.procedures)),
                    Span::raw("  "),
                    Span::styled("Mappers: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}", stats.mappers)),
                ]),
                Line::from(vec![
                    Span::styled("Java Methods: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}", stats.java_methods)),
                    Span::raw("  "),
                    Span::styled("Java Classes: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}", stats.java_classes)),
                ]),
                Line::from(vec![
                    Span::styled("Tables: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}", stats.tables)),
                    Span::raw("  "),
                    Span::styled("Views: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}", stats.views)),
                ]),
                Line::from(vec![
                    Span::styled("Edges: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}", stats.edges)),
                    Span::raw("  "),
                    Span::styled("Unresolved: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}", stats.unresolved)),
                ]),
                Line::from(vec![
                    Span::styled("Files: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}", stats.files)),
                ]),
            ];
            let stats_panel = Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Graph Stats "),
                )
                .wrap(Wrap { trim: true });
            f.render_widget(stats_panel, chunks[0]);

            let file_nodes = store.file_nodes();
            let items: Vec<ListItem> = file_nodes
                .iter()
                .map(|(file, keys)| {
                    let rel = file.strip_prefix(self.project.root()).unwrap_or(file);
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{:>3} ", keys.len()),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(rel.to_string_lossy().to_string()),
                    ]))
                })
                .collect();

            let file_list = List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" Files ({}) ", file_nodes.len())),
            );
            f.render_widget(file_list, chunks[1]);
        } else {
            let msg = Paragraph::new("No store found. Press [A] to analyze.")
                .block(Block::default().borders(Borders::ALL).title(" Dashboard "));
            f.render_widget(msg, area);
        }
    }

    fn draw_files(&self, f: &mut Frame, area: Rect) {
        if let Some(store) = self.project.store() {
            let manifest = store.manifest();
            let mut entries: Vec<_> = manifest.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));

            let items: Vec<ListItem> = entries
                .iter()
                .map(|(path, record)| {
                    let rel = path.strip_prefix(self.project.root()).unwrap_or(path);
                    let type_tag = match record.file_type {
                        crate::parser::fingerprint::FileType::Sql => "SQL",
                        crate::parser::fingerprint::FileType::Java => "Java",
                        crate::parser::fingerprint::FileType::Xml => "XML",
                    };
                    let node_count = store
                        .file_nodes()
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

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" Files ({}) ", manifest.len())),
                )
                .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
            f.render_stateful_widget(list, area, &mut self.list_state.clone());
        }
    }

    fn draw_graph(&self, f: &mut Frame, area: Rect) {
        if let Some(store) = self.project.store() {
            let graph = store.graph();

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(area);

            let selected_node_idx = if self.filter_low_degree {
                let filtered = traverse::low_degree_nodes(graph, self.filter_threshold);
                let items: Vec<ListItem> = filtered
                    .iter()
                    .map(|info| {
                        let node = &graph[info.idx];
                        let (tag, color) = node_tag(node);
                        let key = NodeKey::from_node(node);
                        ListItem::new(Line::from(vec![
                            Span::styled(format!("{:<8} ", tag), Style::default().fg(color)),
                            Span::raw(key.to_string()),
                            Span::raw(format!(" [in:{} out:{}]", info.in_degree, info.out_degree)),
                        ]))
                    })
                    .collect();

                let list = List::new(items)
                    .block(Block::default().borders(Borders::ALL).title(format!(
                        " Graph Nodes (degree ≤ {}, {} shown) ",
                        self.filter_threshold,
                        filtered.len()
                    )))
                    .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
                f.render_stateful_widget(list, chunks[0], &mut self.list_state.clone());

                self.list_state
                    .selected()
                    .and_then(|s| filtered.into_iter().nth(s).map(|d| d.idx))
            } else {
                let indices: Vec<_> = graph.node_indices().collect();
                let items: Vec<ListItem> = indices
                    .iter()
                    .map(|idx| {
                        let node = &graph[*idx];
                        let (tag, color) = node_tag(node);
                        let key = NodeKey::from_node(node);
                        ListItem::new(Line::from(vec![
                            Span::styled(format!("{:<8} ", tag), Style::default().fg(color)),
                            Span::raw(key.to_string()),
                        ]))
                    })
                    .collect();

                let list = List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!(" Graph Nodes ({}) ", graph.node_count())),
                    )
                    .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
                f.render_stateful_widget(list, chunks[0], &mut self.list_state.clone());

                self.list_state
                    .selected()
                    .and_then(|s| indices.into_iter().nth(s))
            };

            if let Some(node_idx) = selected_node_idx {
                let chain = traverse::trace_chain(graph, node_idx);
                let lines = match self.chain_style {
                    traverse::ChainStyle::Tree => {
                        self.render_chain_tree_tui(&chain, graph, node_idx)
                    }
                    traverse::ChainStyle::Path => {
                        let text =
                            traverse::format_chain(&chain, graph, traverse::ChainStyle::Path);
                        text.lines()
                            .map(|l| Line::from(l.to_string()))
                            .collect()
                    }
                };

                let para = Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Node Detail "),
                );
                f.render_widget(para, chunks[1]);
            } else {
                let para = Paragraph::new("Select a node to view details").block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Node Detail "),
                );
                f.render_widget(para, chunks[1]);
            }
        }
    }

    fn draw_trace(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(area);

        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(chunks[0]);

        let input_text = format!("> {}_", self.search_query);
        let input = Paragraph::new(input_text)
            .block(Block::default().borders(Borders::ALL).title(" Search "));
        f.render_widget(input, inner[0]);

        if !self.search_results.is_empty() {
            let items: Vec<ListItem> = self
                .search_results
                .iter()
                .enumerate()
                .map(|(i, (_, name))| {
                    let style = if i == self.search_selected {
                        Style::default().bg(Color::DarkGray).fg(Color::White)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(Span::styled(name.clone(), style)))
                })
                .collect();

            let list = List::new(items).block(Block::default().borders(Borders::NONE));
            let mut state = ListState::default();
            state.select(Some(self.search_selected));
            f.render_stateful_widget(list, inner[1], &mut state);
        }

        if self.chain_display.is_empty() {
            let msg = if self.search_query.is_empty() {
                "Type to search for nodes"
            } else {
                "Press Enter on a result to trace"
            };
            let para = Paragraph::new(msg)
                .block(Block::default().borders(Borders::ALL).title(" Call Chain "));
            f.render_widget(para, chunks[1]);
        } else {
            let para = Paragraph::new(self.chain_display.clone())
                .block(Block::default().borders(Borders::ALL).title(" Call Chain "));
            f.render_widget(para, chunks[1]);
        }
    }

    fn draw_detail(&self, f: &mut Frame, area: Rect) {
        if let (Some(store), Some(node_idx)) = (self.project.store(), self.detail_node_idx) {
            let graph = store.graph();
            let node = &graph[node_idx];
            let key = NodeKey::from_node(node);
            let (tag, tag_color) = node_tag(node);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(8)])
                .split(area);

            let mut lines: Vec<Line> = Vec::new();

            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<8} ", tag),
                    Style::default()
                        .fg(tag_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    key.to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]));

            let in_deg = graph
                .neighbors_directed(node_idx, petgraph::Direction::Incoming)
                .count();
            let out_deg = graph
                .neighbors_directed(node_idx, petgraph::Direction::Outgoing)
                .count();
            lines.push(Line::from(Span::styled(
                format!("in:{} out:{} total:{}", in_deg, out_deg, in_deg + out_deg),
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(""));

            let attr_lines = format_node_attributes_full(node);
            for attr_line in attr_lines {
                lines.push(Line::from(Span::styled(attr_line, Style::default().fg(Color::White))));
            }

            lines.push(Line::from(""));

            let chain = traverse::trace_chain(graph, node_idx);
            let chain_lines = match self.chain_style {
                traverse::ChainStyle::Tree => {
                    self.render_chain_tree_tui(&chain, graph, node_idx)
                }
                traverse::ChainStyle::Path => {
                    let text =
                        traverse::format_chain(&chain, graph, traverse::ChainStyle::Path);
                    text.lines().map(|l| Line::from(l.to_string())).collect()
                }
            };
            lines.extend(chain_lines);

            let para = Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(" Node Detail "))
                .wrap(Wrap { trim: false });
            f.render_widget(para, chunks[0]);
        } else {
            let para = Paragraph::new("No node selected").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Node Detail "),
            );
            f.render_widget(para, area);
        }
    }
}

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
        Node::Custom { type_name, .. } => {
            (std::borrow::Cow::Owned(type_name.clone()), Color::DarkGray)
        }
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
    if let Node::Table {
        location,
        columns,
        partition_by,
        distribute_by,
        tablespace,
        temporary,
        unlogged,
        ddl_source,
        ..
    } = node
    {
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
                attrs.push(format!("  {} {} {}{}{}", col.name, col.data_type, null, pk, def));
            }
            if columns.len() > 5 && compact {
                attrs.push(format!("  ... +{} more", columns.len() - 5));
            }
        }
        if let Some(part) = partition_by {
            match part {
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
            match dist {
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
            attrs.push(format!("ddl: {}", ddl));
        }
    }
    attrs
}

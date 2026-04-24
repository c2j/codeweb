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
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Dashboard,
    Files,
    Graph,
    Trace,
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

        match self.screen {
            Screen::Trace => self.handle_trace_key(key),
            Screen::Graph => self.handle_graph_key(key),
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
            _ => self.handle_global_key(key),
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

    fn run_trace(&mut self, idx: NodeIndex) {
        if let Some(store) = self.project.store() {
            let chain = traverse::trace_chain(store.graph(), idx);
            let tree = traverse::format_chain_tree(&chain, store.graph());
            self.chain_display = tree.lines().map(|l| Line::from(l.to_string())).collect();
        }
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
        let hints = match self.screen {
            Screen::Dashboard => "[A]nalyze  [2]Files  [3]Graph  [4]Trace  [Q]uit",
            Screen::Files => "[↑↓] Navigate  [1]Dashboard  [3]Graph  [4]Trace  [Q]uit",
            Screen::Graph if !self.filter_low_degree => {
                "[↑↓] Navigate  [L]Filter:OFF  [1]Dashboard  [2]Files  [4]Trace  [Q]uit"
            }
            Screen::Graph => {
                "[↑↓] Navigate  [L]Filter:ON [+/-]Threshold  [1]Dashboard  [2]Files  [4]Trace  [Q]uit"
            }
            Screen::Trace if self.in_search => {
                "[Enter]Select  [Esc]Back  [1]Dashboard  [2]Files  [3]Graph  [Q]uit"
            }
            Screen::Trace => "[Esc]Back  [1]Dashboard  [2]Files  [3]Graph  [Q]uit",
        };
        let bar = Paragraph::new(format!(" {}", hints))
            .style(Style::default().bg(Color::DarkGray).fg(Color::Yellow));
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
                    Span::styled("Project: ", Style::default().fg(Color::Gray)),
                    Span::styled(self.project.name(), Style::default().fg(Color::White)),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Procedures: ", Style::default().fg(Color::Gray)),
                    Span::raw(format!("{}", stats.procedures)),
                    Span::raw("  "),
                    Span::styled("Mappers: ", Style::default().fg(Color::Gray)),
                    Span::raw(format!("{}", stats.mappers)),
                ]),
                Line::from(vec![
                    Span::styled("Java Methods: ", Style::default().fg(Color::Gray)),
                    Span::raw(format!("{}", stats.java_methods)),
                    Span::raw("  "),
                    Span::styled("Java Classes: ", Style::default().fg(Color::Gray)),
                    Span::raw(format!("{}", stats.java_classes)),
                ]),
                Line::from(vec![
                    Span::styled("Tables: ", Style::default().fg(Color::Gray)),
                    Span::raw(format!("{}", stats.tables)),
                    Span::raw("  "),
                    Span::styled("Views: ", Style::default().fg(Color::Gray)),
                    Span::raw(format!("{}", stats.views)),
                ]),
                Line::from(vec![
                    Span::styled("Edges: ", Style::default().fg(Color::Gray)),
                    Span::raw(format!("{}", stats.edges)),
                    Span::raw("  "),
                    Span::styled("Unresolved: ", Style::default().fg(Color::Gray)),
                    Span::raw(format!("{}", stats.unresolved)),
                ]),
                Line::from(vec![
                    Span::styled("Files: ", Style::default().fg(Color::Gray)),
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
                .highlight_style(Style::default().bg(Color::DarkGray));
            f.render_stateful_widget(list, area, &mut self.list_state.clone());
        }
    }

    fn draw_graph(&self, f: &mut Frame, area: Rect) {
        if let Some(store) = self.project.store() {
            let graph = store.graph();

            if self.filter_low_degree {
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
                    .highlight_style(Style::default().bg(Color::DarkGray));
                f.render_stateful_widget(list, area, &mut self.list_state.clone());
            } else {
                let items: Vec<ListItem> = graph
                    .node_indices()
                    .map(|idx| {
                        let node = &graph[idx];
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
                    .highlight_style(Style::default().bg(Color::DarkGray));
                f.render_stateful_widget(list, area, &mut self.list_state.clone());
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
                        Style::default().bg(Color::DarkGray)
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
}

fn node_tag(node: &Node) -> (&'static str, Color) {
    match node {
        Node::Procedure { .. } => ("proc", Color::Green),
        Node::Unresolved { .. } => ("unres", Color::Red),
        Node::MappedStatement { .. } => ("mapper", Color::Blue),
        Node::JavaSql { .. } => ("sql", Color::Magenta),
        Node::JavaMethod { .. } => ("method", Color::Cyan),
        Node::JavaClass { .. } => ("class", Color::Yellow),
        Node::Table { .. } => ("table", Color::Yellow),
        Node::View { .. } => ("view", Color::Blue),
    }
}

use crate::graph::key::NodeKey;
use crate::graph::Node;
use crate::project::Project;
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
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Dashboard,
    Files,
    Graph,
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

    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Dashboard => 1,
            Screen::Files => self
                .project
                .store()
                .map(|s| s.manifest().len())
                .unwrap_or(0),
            Screen::Graph => self
                .project
                .store()
                .map(|s| s.graph().node_count())
                .unwrap_or(0),
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
            Screen::Dashboard => "[A]nalyze  [2]Files  [3]Graph  [Q]uit",
            Screen::Files => "[↑↓] Navigate  [1]Dashboard  [3]Graph  [Q]uit",
            Screen::Graph => "[↑↓] Navigate  [1]Dashboard  [2]Files  [Q]uit",
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

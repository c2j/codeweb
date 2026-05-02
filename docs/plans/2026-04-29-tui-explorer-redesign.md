# TUI Explorer Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Merge the 5-screen TUI into a streamlined 3-screen structure with scroll support and unified search.

**Architecture:** Replace Dashboard/Files/Graph/Trace/Detail with Explorer (main), Detail (overlay), Info (secondary). Explorer has always-visible search, scrollable node list, and live detail preview. All content panels support vertical scrolling.

**Tech Stack:** Rust, ratatui 0.29, crossterm 0.28

---

## Current State

5 screens: `Dashboard`, `Files`, `Graph`, `Trace`, `Detail`

Problems:
- Graph has no search
- Trace search result content not scrollable
- Detail content not scrollable
- Dashboard and Files are rarely needed as full screens
- Too many key bindings for screen switching

## Target State

3 screens: `Explorer`, `Detail`, `Info`

### Explorer (main screen)

```
┌─ codeweb ─ project ─ Explorer ────────────────────────────┐
│┌ Search ──────────────────────────────────────────────────┐│
││ > sp_order_                                              ││
│└──────────────────────────────────────────────────────────┘│
│┌ Nodes (5) ───────────┐┌ Preview ────────────────────────┐│
││► proc  sp_order_inse..││ ▶ proc  sp_order_insert         ││
││  proc  sp_order_valid..││ in:3 out:8                      ││
││  table inventory      ││                                  ││
││  table warehouse      ││ ── CALLERS ──                    ││
││  proc  sp_price_calc  ││ ├── mapper OrderMapper.insert.. ││
││  ...                  ││ ── CALLEES ──                    ││
││                       ││ ├── proc  sp_order_validate     ││
│└───────────────────────┘└──────────────────────────────────┘│
│ [↑↓]Nav  [Enter]Full  [S]tyle:Tree  [L]Filter  [2]Info  [Q]uit │
└────────────────────────────────────────────────────────────┘
```

Layout: search bar (3 rows) → split panel (nodes 35% | preview 65%)

### Detail (full-screen overlay)

```
┌─ codeweb ─ project ─ Detail ──────────────────────────────┐
│  proc    sp_order_insert                                   │
│  in:3 out:8 total:11                                       │
│  ...                                                       │
│  ── CALLERS ──                                             │
│  ...full call chain...                                     │
│  ── CALLEES ──                                             │
│  ...full call chain...                                     │
│                                                            │
│ [↑↓]Scroll  [Esc]Back  [S]tyle:Tree  [Q]uit              │
└────────────────────────────────────────────────────────────┘
```

Full-screen scrollable content. `detail_scroll: u16` tracks vertical offset.

### Info (secondary)

```
┌─ codeweb ─ project ─ Info ────────────────────────────────┐
│┌ Stats ──────────────────────────────┐┌ Files ───────────┐│
││ Procedures: 45                      ││ SQL  12  path..  ││
││ Tables: 23                          ││ Java  8  path..  ││
││ ...                                 ││ XML   5  path..  ││
│└─────────────────────────────────────┘└──────────────────┘│
│ [Esc]Back  [Q]uit                                         │
└────────────────────────────────────────────────────────────┘
```

Merged dashboard stats + file list. Both scrollable.

---

## Key Bindings (simplified)

### Explorer
| Key | Action |
|-----|--------|
| type | Append to search query, filter nodes |
| Backspace | Delete last char from search |
| ↑/k | Previous node in list |
| ↓/j | Next node in list |
| Enter | Open Detail for selected node |
| S | Toggle chain style (tree/path) |
| L | Toggle low-degree filter |
| +/- | Adjust filter threshold |
| 2/I | Switch to Info screen |
| A | Analyze project |
| Q/Esc | Quit |

### Detail
| Key | Action |
|-----|--------|
| ↑/k | Scroll up |
| ↓/j | Scroll down |
| S | Toggle chain style |
| Esc/Enter | Back to Explorer |
| Q | Quit |

### Info
| Key | Action |
|-----|--------|
| ↑/k | Scroll up |
| ↓/j | Scroll down |
| Esc/1 | Back to Explorer |
| Q | Quit |

---

## Task 1: Rewrite App Struct and Enums

**Files:**
- Modify: `src/tui/app.rs`

**Step 1: Replace Screen enum and add Focus enum**

Replace:
```rust
#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Dashboard,
    Files,
    Graph,
    Trace,
    Detail,
}
```

With:
```rust
#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Explorer,
    Detail,
    Info,
}
```

**Step 2: Rewrite App struct**

Replace the current `App` struct with:
```rust
pub struct App {
    project: Project,
    screen: Screen,
    should_quit: bool,

    // Explorer: search + node list
    search_query: String,
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
    filter_low_degree: bool,
    filter_threshold: usize,
}
```

**Step 3: Rewrite App::new()**

```rust
pub fn new(project: Project) -> Self {
    let mut list_state = ListState::default();
    list_state.select(Some(0));
    let mut app = Self {
        project,
        screen: Screen::Explorer,
        should_quit: false,
        search_query: String::new(),
        nodes: Vec::new(),
        list_state,
        detail_node_idx: None,
        detail_lines: Vec::new(),
        detail_scroll: 0,
        info_scroll: 0,
        chain_style: traverse::ChainStyle::default(),
        filter_low_degree: false,
        filter_threshold: 0,
    };
    app.refresh_node_list();
    app
}
```

**Step 4: Add refresh_node_list helper**

```rust
fn refresh_node_list(&mut self) {
    let Some(store) = self.project.store() else { return };
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
    } else {
        self.nodes = traverse::find_nodes_by_name(graph, &self.search_query)
            .into_iter()
            .map(|(idx, _)| idx)
            .collect();
    }
    // Clamp selection
    if !self.nodes.is_empty() {
        let current = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(current.min(self.nodes.len() - 1)));
    }
}
```

**Step 5: Run cargo check to verify struct compiles**

Run: `cargo check`
Expected: compile errors in the draw/handle_key methods (expected, we fix those next)

---

## Task 2: Rewrite Key Handling

**Files:**
- Modify: `src/tui/app.rs`

**Step 1: Rewrite handle_key as a clean dispatch**

```rust
fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
    use crossterm::event::{KeyCode, KeyEventKind};
    if key.kind != KeyEventKind::Press {
        return;
    }

    match self.screen {
        Screen::Explorer => self.handle_explorer_key(key.code),
        Screen::Detail => self.handle_detail_key(key.code),
        Screen::Info => self.handle_info_key(key.code),
    }
}
```

**Step 2: Implement handle_explorer_key**

```rust
fn handle_explorer_key(&mut self, code: crossterm::event::KeyCode) {
    use crossterm::event::KeyCode;
    match code {
        KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
        KeyCode::Char('2') | KeyCode::Char('i') => {
            self.screen = Screen::Info;
            self.info_scroll = 0;
        }
        KeyCode::Char('a') => { let _ = self.project.analyze(); self.refresh_node_list(); }
        KeyCode::Char('s') => {
            self.chain_style = match self.chain_style {
                traverse::ChainStyle::Tree => traverse::ChainStyle::Path,
                traverse::ChainStyle::Path => traverse::ChainStyle::Tree,
            };
            self.update_detail_preview();
        }
        KeyCode::Char('l') => {
            self.filter_low_degree = !self.filter_low_degree;
            self.list_state.select(Some(0));
            self.refresh_node_list();
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
        KeyCode::Backspace => {
            self.search_query.pop();
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
            self.update_detail_preview();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let i = self.list_state.selected().unwrap_or(0);
            self.list_state.select(Some(i.saturating_sub(1)));
            self.update_detail_preview();
        }
        KeyCode::Char(c) if !matches!(c, 'q' | 's' | 'l' | 'a' | 'i' | '2' | 'j' | 'k') => {
            self.search_query.push(c);
            self.list_state.select(Some(0));
            self.refresh_node_list();
        }
        _ => {}
    }
}
```

**Step 3: Implement handle_detail_key with scroll**

```rust
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
        KeyCode::Down | KeyCode::Char('j') => {
            self.detail_scroll = self.detail_scroll.saturating_add(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            self.detail_scroll = self.detail_scroll.saturating_sub(1);
        }
        _ => {}
    }
}
```

**Step 4: Implement handle_info_key**

```rust
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
        _ => {}
    }
}
```

**Step 5: Add selected_node and open_detail/update_detail_preview helpers**

```rust
fn selected_node(&self) -> Option<NodeIndex> {
    let i = self.list_state.selected()?;
    self.nodes.get(i).copied()
}

fn open_detail(&mut self, idx: NodeIndex) {
    let Some(store) = self.project.store() else { return };
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

    let in_deg = graph.neighbors_directed(idx, petgraph::Direction::Incoming).count();
    let out_deg = graph.neighbors_directed(idx, petgraph::Direction::Outgoing).count();
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

    let chain = traverse::trace_chain(graph, idx);
    let chain_lines = match self.chain_style {
        traverse::ChainStyle::Tree => self.render_chain_tree_tui(&chain, graph, idx),
        traverse::ChainStyle::Path => {
            let text = traverse::format_chain(&chain, graph, traverse::ChainStyle::Path);
            text.lines().map(|l| Line::from(l.to_string())).collect()
        }
    };
    lines.extend(chain_lines);

    self.detail_node_idx = Some(idx);
    self.detail_lines = lines;
    self.detail_scroll = 0;
    self.screen = Screen::Detail;
}

fn update_detail_preview(&mut self) {
    // Detail preview is generated on-the-fly in draw_explorer
    // This is a no-op placeholder; the actual rendering reads nodes directly
}
```

**Step 6: Run cargo check**

Run: `cargo check`
Expected: compile errors only in draw methods (next task)

---

## Task 3: Rewrite Drawing Methods

**Files:**
- Modify: `src/tui/app.rs`

**Step 1: Rewrite draw dispatch**

```rust
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
```

**Step 2: Update title bar**

```rust
fn draw_title_bar(&self, f: &mut Frame, area: Rect) {
    let title = format!(
        " codeweb ─ {} ─ {} ",
        self.project.name(),
        match self.screen {
            Screen::Explorer => "Explorer",
            Screen::Detail => "Detail",
            Screen::Info => "Info",
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
```

**Step 3: Update status bar**

```rust
fn draw_status_bar(&self, f: &mut Frame, area: Rect) {
    let style_label = match self.chain_style {
        traverse::ChainStyle::Tree => "Tree",
        traverse::ChainStyle::Path => "Path",
    };
    let hints: String = match self.screen {
        Screen::Explorer => {
            if self.filter_low_degree {
                format!("[↑↓]Nav  [Enter]Full  [S]tyle:{}  [L]Filter:ON [+/-]  [2]Info  [Q]uit", style_label)
            } else {
                format!("[↑↓]Nav  [Enter]Full  [S]tyle:{}  [L]Filter  [2]Info  [Q]uit", style_label)
            }
        }
        Screen::Detail => format!("[↑↓]Scroll  [Esc]Back  [S]tyle:{}  [Q]uit", style_label),
        Screen::Info => "[↑↓]Scroll  [Esc]Back  [Q]uit".to_string(),
    };
    let bar = Paragraph::new(format!(" {}", hints))
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_widget(bar, area);
}
```

**Step 4: Implement draw_explorer**

Layout: search bar (3 lines) → split panel (35% nodes | 65% preview)

```rust
fn draw_explorer(&self, f: &mut Frame, area: Rect) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // Search bar
    let input_text = format!("> {}_", self.search_query);
    let search = Paragraph::new(input_text)
        .block(Block::default().borders(Borders::ALL).title(" Search "));
    f.render_widget(search, outer[0]);

    // Split panel: nodes | preview
    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(outer[1]);

    // Node list
    let Some(store) = self.project.store() else { return };
    let graph = store.graph();
    let items: Vec<ListItem> = self.nodes.iter().map(|idx| {
        let node = &graph[*idx];
        let (tag, color) = node_tag(node);
        let key = NodeKey::from_node(node);
        let mut spans = vec![
            Span::styled(format!("{:<8} ", tag), Style::default().fg(color)),
            Span::raw(key.to_string()),
        ];
        if self.filter_low_degree {
            let in_deg = graph.neighbors_directed(*idx, petgraph::Direction::Incoming).count();
            let out_deg = graph.neighbors_directed(*idx, petgraph::Direction::Outgoing).count();
            spans.push(Span::styled(format!(" [in:{} out:{}]", in_deg, out_deg), Style::default().fg(Color::DarkGray)));
        }
        ListItem::new(Line::from(spans))
    }).collect();

    let node_count = self.nodes.len();
    let list_title = if self.search_query.is_empty() {
        format!(" Nodes ({}) ", node_count)
    } else {
        format!(" Nodes ({}/{} matched) ", node_count, graph.node_count())
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(list_title))
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_stateful_widget(list, panels[0], &mut self.list_state.clone());

    // Preview panel: show call chain for selected node
    if let Some(idx) = self.selected_node() {
        let chain = traverse::trace_chain(graph, idx);
        let preview_lines = match self.chain_style {
            traverse::ChainStyle::Tree => self.render_chain_tree_tui(&chain, graph, idx),
            traverse::ChainStyle::Path => {
                let text = traverse::format_chain(&chain, graph, traverse::ChainStyle::Path);
                text.lines().map(|l| Line::from(l.to_string())).collect()
            }
        };
        let para = Paragraph::new(preview_lines)
            .block(Block::default().borders(Borders::ALL).title(" Preview "))
            .wrap(Wrap { trim: false });
        f.render_widget(para, panels[1]);
    } else {
        let para = Paragraph::new("Select a node to preview")
            .block(Block::default().borders(Borders::ALL).title(" Preview "));
        f.render_widget(para, panels[1]);
    }
}
```

**Step 5: Rewrite draw_detail with scroll**

```rust
fn draw_detail(&self, f: &mut Frame, area: Rect) {
    let para = Paragraph::new(self.detail_lines.clone())
        .block(Block::default().borders(Borders::ALL).title(" Node Detail "))
        .wrap(Wrap { trim: false })
        .scroll((self.detail_scroll, 0));
    f.render_widget(para, area);
}
```

**Step 6: Implement draw_info**

```rust
fn draw_info(&self, f: &mut Frame, area: Rect) {
    let Some(store) = self.project.store() else { return };
    let stats = store.stats();
    let graph = store.graph();

    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    // Stats
    let stats_lines = vec![
        Line::from(vec![
            Span::styled("Procedures: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", stats.procedures)),
        ]),
        Line::from(vec![
            Span::styled("Functions: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", stats.functions)),
        ]),
        Line::from(vec![
            Span::styled("Tables: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", stats.tables)),
        ]),
        Line::from(vec![
            Span::styled("Views: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", stats.views)),
        ]),
        Line::from(vec![
            Span::styled("Mappers: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", stats.mappers)),
        ]),
        Line::from(vec![
            Span::styled("Java Methods: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", stats.java_methods)),
        ]),
        Line::from(vec![
            Span::styled("Edges: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", stats.edges)),
        ]),
        Line::from(vec![
            Span::styled("Files: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", stats.files)),
        ]),
    ];
    let stats_para = Paragraph::new(stats_lines)
        .block(Block::default().borders(Borders::ALL).title(" Stats "))
        .scroll((self.info_scroll, 0));
    f.render_widget(stats_para, panels[0]);

    // Files
    let file_nodes = store.file_nodes();
    let manifest = store.manifest();
    let mut entries: Vec<_> = manifest.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let root = self.project.root();
    let file_items: Vec<ListItem> = entries.iter().map(|(path, record)| {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let type_tag = match record.file_type {
            crate::parser::fingerprint::FileType::Sql => "SQL",
            crate::parser::fingerprint::FileType::Java => "Java",
            crate::parser::fingerprint::FileType::Xml => "XML",
        };
        let node_count = file_nodes.get(path as &std::path::Path).map(|v| v.len()).unwrap_or(0);
        ListItem::new(Line::from(vec![
            Span::styled(format!("{:<4} ", type_tag), Style::default().fg(Color::Cyan)),
            Span::styled(format!("{:>3} ", node_count), Style::default().fg(Color::Yellow)),
            Span::raw(rel.to_string_lossy().to_string()),
        ]))
    }).collect();
    let file_list = List::new(file_items)
        .block(Block::default().borders(Borders::ALL).title(format!(" Files ({}) ", manifest.len())));
    f.render_widget(file_list, panels[1]);
}
```

**Step 7: Remove old draw methods**

Delete: `draw_dashboard`, `draw_files`, `draw_graph`, `draw_trace` (the old ones).
Delete: `handle_trace_key`, `handle_graph_key`, `handle_detail_key`, `handle_global_key` (replaced by new handlers).
Delete: `resolve_graph_selected_node`, `run_trace`, `refresh_chain_display`, `list_len`.
Keep: `node_tag`, `tree_node_to_lines`, `format_node_attributes`, `format_node_attributes_full`, `format_node_attributes_impl`, `render_chain_tree_tui`.

**Step 8: Run cargo clippy -- -D warnings**

Run: `cargo clippy -- -D warnings`
Expected: PASS

---

## Task 4: Run Tests and Verify

**Step 1: Run all tests**

Run: `cargo test`
Expected: all pass

**Step 2: Run format check**

Run: `cargo fmt -- --check`
Expected: no output (clean)

**Step 3: Manual TUI smoke test**

Run: `cargo run --features tui -- tui`
Verify:
- Explorer shows search bar + node list + preview
- Typing filters nodes
- ↑↓ navigates, preview updates
- Enter opens scrollable Detail
- ↑↓ scrolls Detail content
- Esc returns to Explorer
- S toggles tree/path style
- 2/I switches to Info
- Q quits

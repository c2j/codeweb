# Serve Trace Performance + Resizable Panels

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Optimize serve mode for nodes with 40k+ edges — add trace depth/node limits, paginated neighbor endpoints, frontend threshold skip, virtual scroll detail panel, and resizable split panels.

**Architecture:** Backend adds `depth`/`max_nodes` params to `/trace` and new paginated `/nodes/:id/callers` + `/nodes/:id/callees` endpoints. Frontend skips Cytoscape rendering when node count exceeds threshold, uses virtual scroll for detail lists, and adds drag-to-resize panel dividers.

**Tech Stack:** Rust (axum, petgraph), vanilla JS (Cytoscape.js, dagre), CSS flexbox

**Worktree:** `.worktrees/serve-trace-perf` on branch `feat/serve-trace-perf`

---

## Key Files

| File | Purpose |
|------|---------|
| `src/graph/traverse.rs` | `build_tree_dfs()`, `trace_chain()`, `MatchRank` |
| `src/server/handlers.rs` | All API endpoints including `/trace`, `/nodes/:id` |
| `src/server/mod.rs` | Router definition |
| `assets/app.js` | Browser UI — selectNode, renderTraceGraph, showDetail |
| `assets/style.css` | Layout styles — panel widths, flexbox |
| `assets/index.html` | HTML structure — panel dividers to be added |
| `tests/serve_api.rs` | Serve integration tests |

---

## Task 1: Add `depth` and `max_nodes` limits to trace endpoint

**Files:**
- Modify: `src/graph/traverse.rs` — `build_tree_dfs()` signature and body
- Modify: `src/graph/traverse.rs` — `trace_chain()` signature
- Modify: `src/server/handlers.rs` — `TraceQuery` struct and `trace()` handler

### Step 1: Modify `build_tree_dfs` to accept `max_depth` and `max_nodes`

In `src/graph/traverse.rs`, change `build_tree_dfs` signature to accept configurable limits. Add a counter passed by `&mut usize` to track total nodes visited. When `max_nodes` exceeded, stop adding children.

Current signature (line 117):
```rust
fn build_tree_dfs(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    direction: Direction,
    ancestors: &mut HashSet<NodeIndex>,
    depth: usize,
) -> Vec<TreeNode>
```

New signature:
```rust
fn build_tree_dfs(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    direction: Direction,
    ancestors: &mut HashSet<NodeIndex>,
    depth: usize,
    max_depth: usize,
    max_nodes: usize,
    visited: &mut usize,
) -> Vec<TreeNode>
```

- Remove the hardcoded `let max_depth = 50;` — use the parameter instead.
- At the start of the function, if `*visited >= max_nodes`, return empty Vec.
- After pushing each TreeNode, increment `*visited`. Check against `max_nodes` before recursing into children.

### Step 2: Modify `trace_chain` to accept limits

Change `trace_chain` signature:
```rust
pub fn trace_chain(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    max_depth: usize,
    max_nodes: usize,
) -> CallChain
```

Initialize `let mut visited = 0;` and pass to both `build_tree_dfs` calls. Note: `visited` counts nodes across both directions cumulatively so the total cap works.

### Step 3: Update `TraceQuery` and `trace()` handler

In `src/server/handlers.rs`:

```rust
#[derive(serde::Deserialize)]
struct TraceQuery {
    from: String,
    depth: Option<usize>,
    max_nodes: Option<usize>,
}
```

In `trace()` handler, use defaults:
```rust
let depth = query.depth.unwrap_or(2).min(5);
let max_nodes = query.max_nodes.unwrap_or(500);
let chain = traverse::trace_chain(graph, *start_idx, depth, max_nodes);
```

### Step 4: Add `truncated` and counts to trace response

```rust
let result = serde_json::json!({
    "target": { ... },
    "callers": tree_nodes_to_json(&chain.callers, graph),
    "callees": tree_nodes_to_json(&chain.callees, graph),
    "caller_count": chain.callers.len(),
    "callee_count": chain.callees.len(),
    "truncated": chain.callers.len() + chain.callees.len() >= max_nodes,
});
```

Actually — `truncated` should be determined by whether `visited >= max_nodes`. Return `visited_count` from `trace_chain`. Easiest: make `trace_chain` return `(CallChain, usize)` where the `usize` is total visited.

### Step 5: Run tests

```bash
cargo test
cargo clippy --features serve -- -D warnings
```

### Step 6: Commit

```
perf(serve): add depth and max_nodes limits to /trace endpoint
```

---

## Task 2: Add paginated neighbor endpoints

**Files:**
- Modify: `src/server/handlers.rs` — add two new handlers + routes
- Modify: `src/server/mod.rs` — add routes (if router is in mod.rs; check handlers.rs first)

### Step 1: Add `/nodes/:id/callers` endpoint

In `handlers.rs`, add:

```rust
#[derive(serde::Deserialize)]
struct NeighborsQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn node_callers(
    State(state): State<AppState>,
    Path(id): Path<usize>,
    Query(query): Query<NeighborsQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let graph = state.graph();
    let idx = NodeIndex::new(id);
    if idx.index() >= graph.node_count() {
        return Err(StatusCode::NOT_FOUND);
    }
    let neighbors: Vec<_> = graph
        .neighbors_directed(idx, petgraph::Direction::Incoming)
        .map(|n| {
            let key = NodeKey::from_node(&graph[n]);
            serde_json::json!({
                "id": n.index(),
                "key": key.to_string(),
                "type": node_type_tag(&graph[n]),
            })
        })
        .collect();

    let total = neighbors.len();
    let limit = query.limit.unwrap_or(50);
    let offset = query.offset.unwrap_or(0);
    let result: Vec<_> = neighbors.into_iter().skip(offset).take(limit).collect();

    Ok(Json(serde_json::json!({
        "total": total,
        "limit": limit,
        "offset": offset,
        "nodes": result,
    })))
}
```

### Step 2: Add `/nodes/:id/callees` endpoint

Same pattern but with `Direction::Outgoing`.

### Step 3: Register routes

In the `router()` function, add:
```rust
.route("/api/v1/nodes/:id/callers", get(node_callers))
.route("/api/v1/nodes/:id/callees", get(node_callees))
```

### Step 4: Run tests + commit

```
feat(serve): add paginated /nodes/:id/callers and /nodes/:id/callees endpoints
```

---

## Task 3: Frontend — trace graph threshold skip

**Files:**
- Modify: `assets/app.js` — `renderTraceGraph()`

### Step 1: Add threshold constant

At top of `app.js`:
```javascript
const TRACE_GRAPH_THRESHOLD = 300;
```

### Step 2: Modify `renderTraceGraph`

Before adding elements to Cytoscape, count total nodes. If exceeds threshold, clear the graph and show a message instead:

```javascript
function renderTraceGraph(trace) {
  if (!cy) return;
  cy.elements().remove();

  const callerCount = trace.caller_count || 0;
  const calleeCount = trace.callee_count || 0;
  const total = callerCount + calleeCount;

  if (total > TRACE_GRAPH_THRESHOLD) {
    document.getElementById('cy').innerHTML =
      '<div style="display:flex;align-items:center;justify-content:center;height:100%;color:#a0a0a0;text-align:center;padding:40px">' +
      '<div>' +
      '<div style="font-size:48px;margin-bottom:12px">&#x1F4CA;</div>' +
      '<div style="font-size:16px;margin-bottom:8px">' + total.toLocaleString() + ' upstream/downstream nodes</div>' +
      '<div style="font-size:13px">Graph view hidden for large node counts.<br>Use the detail panel to browse relationships.</div>' +
      '</div></div>';
    return;
  }

  // ... existing rendering code (els, addTree, cy.add, layout)
}
```

### Step 3: Commit

```
fix(serve): skip graph rendering for traces exceeding 300 nodes
```

---

## Task 4: Frontend — detail panel virtual scroll for callers/callees

**Files:**
- Modify: `assets/app.js` — `showDetail()`, add new functions
- Modify: `assets/style.css` — add styles for virtual scroll lists

### Step 1: Change `showDetail` to use paginated loading

Replace the current `showDetail` that dumps all callers/callees from the `/nodes/:id` response. Instead:

1. Show node key, type, degree from `/nodes/:id` (keep existing call for metadata).
2. For callers/callees sections, call `/nodes/:id/callers?limit=50&offset=0` and `/nodes/:id/callees?limit=50&offset=0`.
3. Render each section as a scrollable div with a "load more" trigger at the bottom.
4. When scrolled to bottom (or button clicked), fetch next page and append.

New functions needed:
- `loadNeighbors(id, direction, offset, container)` — fetches a page and renders
- Each section is a `<div class="neighbor-list">` with scroll listener

```javascript
async function showDetail(d) {
  document.getElementById('detail-title').textContent = d.type + ' ' + d.key;
  let h = '<div class="section-title first">Degree</div>';
  h += '<div>in:' + d.in_degree + ' out:' + d.out_degree + ' total:' + (d.in_degree + d.out_degree) + '</div>';

  h += '<div class="section-title">Callers (' + d.in_degree + ')</div>';
  h += '<div id="callers-list" class="neighbor-list" data-id="' + d.id + '" data-dir="callers" data-offset="0"></div>';

  h += '<div class="section-title">Callees (' + d.out_degree + ')</div>';
  h += '<div id="callees-list" class="neighbor-list" data-id="' + d.id + '" data-dir="callees" data-offset="0"></div>';

  document.getElementById('detail-content').innerHTML = h;
  document.getElementById('detail-panel').classList.remove('hidden');

  loadNeighbors(d.id, 'callers', 0);
  loadNeighbors(d.id, 'callees', 0);

  requestAnimationFrame(() => { if (cy) { cy.resize(); cy.fit(undefined, 40); } });
}

async function loadNeighbors(id, dir, offset) {
  const container = document.getElementById(dir + '-list');
  if (!container) return;
  const limit = 50;

  if (offset === 0) {
    container.innerHTML = '<div class="loading">Loading...</div>';
  }

  const data = await api('/nodes/' + id + '/' + dir + '?limit=' + limit + '&offset=' + offset);

  if (offset === 0) container.innerHTML = '';

  for (const n of data.nodes) {
    const div = document.createElement('div');
    div.className = 'detail-node';
    div.innerHTML = '<span class="node-tag" style="color:' + (TAG_COLORS[n.type] || '#999') + '">' + n.type + '</span> ' + esc(n.key);
    div.onclick = () => selectNode(n.id);
    container.appendChild(div);
  }

  container.dataset.offset = String(offset + data.nodes.length);

  if (offset + data.nodes.length < data.total) {
    let loader = container.querySelector('.load-more');
    if (!loader) {
      loader = document.createElement('div');
      loader.className = 'load-more';
      loader.textContent = 'Load more (' + (data.total - offset - data.nodes.length) + ' remaining)';
      loader.onclick = () => {
        loader.remove();
        loadNeighbors(id, dir, parseInt(container.dataset.offset));
      };
      container.appendChild(loader);
    }
  }

  requestAnimationFrame(() => { if (cy) { cy.resize(); cy.fit(undefined, 40); } });
}
```

### Step 2: Add CSS styles

```css
.neighbor-list { max-height: 300px; overflow-y: auto; }
.load-more { padding: 8px; text-align: center; color: #e94560; cursor: pointer; font-size: 12px; }
.load-more:hover { text-decoration: underline; }
.loading { padding: 8px; color: #666; font-size: 12px; }
```

### Step 3: Commit

```
feat(serve): paginated virtual scroll for detail panel callers/callees
```

---

## Task 5: Frontend — resizable split panels

**Files:**
- Modify: `assets/index.html` — add divider elements
- Modify: `assets/style.css` — divider styles, remove fixed widths
- Modify: `assets/app.js` — drag handler logic

### Step 1: Add divider elements to HTML

In `index.html`, between panels add divider divs:

```html
<div id="content">
  <div id="node-panel">
    <div id="node-list"></div>
  </div>
  <div class="panel-divider" data-left="node-panel" data-right="graph-panel"></div>
  <div id="graph-panel">
    <div id="cy"></div>
  </div>
  <div class="panel-divider" data-left="graph-panel" data-right="detail-panel"></div>
  <div id="detail-panel" class="hidden">
    ...
  </div>
</div>
```

### Step 2: Add CSS for dividers

```css
.panel-divider {
  width: 4px;
  background: #0f3460;
  cursor: col-resize;
  flex-shrink: 0;
  transition: background 0.15s;
}
.panel-divider:hover, .panel-divider.active {
  background: #e94560;
}
```

Update panel styles:
- `#node-panel`: remove `width:320px`, keep `min-width:150px`, add `flex-shrink:0`, set initial width via JS or CSS `width:320px` (overridable)
- `#detail-panel`: remove `min-width:400px`, keep `min-width:200px`, keep `width:400px` as initial
- `#graph-panel`: keep `flex:1;min-width:200px`

### Step 3: Add drag handler in JS

```javascript
document.querySelectorAll('.panel-divider').forEach(divider => {
  let startX = 0;
  let startLeftWidth = 0;
  let leftPanel = null;
  let rightPanel = null;

  divider.addEventListener('mousedown', e => {
    e.preventDefault();
    leftPanel = document.getElementById(divider.dataset.left);
    rightPanel = document.getElementById(divider.dataset.right);
    if (!leftPanel || !rightPanel) return;
    startX = e.clientX;
    startLeftWidth = leftPanel.getBoundingClientRect().width;
    divider.classList.add('active');
    document.addEventListener('mousemove', onDrag);
    document.addEventListener('mouseup', onStop);
  });

  function onDrag(e) {
    const dx = e.clientX - startX;
    const newWidth = Math.max(150, startLeftWidth + dx);
    leftPanel.style.width = newWidth + 'px';
    leftPanel.style.minWidth = newWidth + 'px';
    if (cy) cy.resize();
  }

  function onStop() {
    divider.classList.remove('active');
    document.removeEventListener('mousemove', onDrag);
    document.removeEventListener('mouseup', onStop);
  }
});
```

### Step 4: Commit

```
feat(serve): add draggable panel dividers for resizable layout
```

---

## Final Verification

```bash
cargo test
cargo clippy --features serve -- -D warnings
cargo fmt -- --check
```

Commit any remaining fixes, push, create PR.

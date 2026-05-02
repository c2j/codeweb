let cy = null;
let allNodes = [];
let allNodesTotal = 0;
let selectedNodeId = null;

const ITEM_HEIGHT = 36;
const BUFFER = 5;
const TRACE_GRAPH_THRESHOLD = 300;

const TAG_COLORS = {
  proc: '#4caf50', 'proc*': '#388e3c', func: '#8bc34a', 'func*': '#689f38',
  table: '#ff9800', mapper: '#2196f3', method: '#00bcd4', class: '#ff5722',
  view: '#2196f3', pkg: '#ffeb3b', unres: '#f44336', trigger: '#f44336',
  type: '#ffeb3b', seq: '#8bc34a', index: '#9e9e9e', mview: '#00bcd4',
  synonym: '#9c27b0', event: '#ff5722', sql: '#9c27b0',
};

async function api(path) {
  const r = await fetch('/api/v1' + path);
  return r.json();
}

async function init() {
  const stats = await api('/stats');
  document.getElementById('stats-bar').innerHTML = [
    stats.procedures + ' procs',
    stats.functions + ' funcs',
    stats.tables + ' tables',
    stats.mappers + ' mappers',
    stats.java_methods + ' methods',
    stats.edges + ' edges',
    stats.files + ' files',
  ].map(s => '<span>' + s + '</span>').join('');

  await loadNodes();
  initGraph();

  const si = document.getElementById('search-input');
  let t;
  si.addEventListener('input', () => { clearTimeout(t); t = setTimeout(() => loadNodes(si.value), 500); });

  document.addEventListener('keydown', e => {
    if (e.key === '/' && document.activeElement !== si) { e.preventDefault(); si.focus(); }
    if (e.key === 'Escape') { hideDetail(); si.blur(); }
  });

  document.getElementById('detail-close').addEventListener('click', hideDetail);
}

async function loadNodes(search) {
  const data = await api('/nodes?limit=100&offset=0' + (search ? '&search=' + encodeURIComponent(search) : ''));
  allNodes = data.nodes;
  allNodesTotal = data.total;
  renderVirtualList();
  updateNodeCount();
}

function updateNodeCount() {
  const el = document.getElementById('node-count');
  if (allNodesTotal > allNodes.length) {
    el.textContent = allNodesTotal + ' nodes (showing ' + allNodes.length + ')';
  } else {
    el.textContent = allNodesTotal + ' nodes';
  }
}

function renderVirtualList() {
  const list = document.getElementById('node-list');
  const scrollTop = list.scrollTop;
  const viewHeight = list.clientHeight;
  const count = allNodes.length;

  const startIdx = Math.max(0, Math.floor(scrollTop / ITEM_HEIGHT) - BUFFER);
  const endIdx = Math.min(count, Math.ceil((scrollTop + viewHeight) / ITEM_HEIGHT) + BUFFER);

  const totalHeight = count * ITEM_HEIGHT;

  let html = '<div style="height:' + totalHeight + 'px;position:relative">';
  for (let i = startIdx; i < endIdx; i++) {
    const n = allNodes[i];
    if (!n) continue;
    html += '<div class="node-item' + (n.id === selectedNodeId ? ' active' : '') +
      '" style="position:absolute;top:' + (i * ITEM_HEIGHT) + 'px;left:0;right:0;height:' + ITEM_HEIGHT + 'px"' +
      ' onclick="selectNode(' + n.id + ')" data-id="' + n.id + '">' +
      '<span class="node-tag" style="color:' + (TAG_COLORS[n.type] || '#999') + '">' + n.type + '</span>' +
      '<span class="node-key" title="' + esc(n.key) + '">' + esc(n.key) + '</span>' +
      '<span class="node-degree">' + n.in_degree + '/' + n.out_degree + '</span></div>';
  }
  html += '</div>';
  list.innerHTML = html;
}

function esc(s) { return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }

async function selectNode(id) {
  selectedNodeId = id;
  renderVirtualList();
  const node = allNodes.find(n => n.id === id);
  if (!node) return;

  const trace = await api('/trace?from=' + encodeURIComponent(node.key));
  renderTraceGraph(trace);

  const detail = await api('/nodes/' + id);
  showDetail(detail);
}

function initGraph() {
  cy = cytoscape({
    container: document.getElementById('cy'),
    style: [
      { selector: 'node', style: {
        'label': 'data(label)', 'text-valign': 'center', 'text-halign': 'center',
        'font-size': '10px', 'color': '#e0e0e0', 'background-color': 'data(color)',
        'width': 24, 'height': 24, 'text-wrap': 'ellipsis', 'text-max-width': '80px',
      }},
      { selector: 'node.target', style: { 'border-width': 3, 'border-color': '#e94560', 'width': 32, 'height': 32 }},
      { selector: 'edge', style: {
        'width': 1.5, 'line-color': '#555', 'target-arrow-color': '#555',
        'target-arrow-shape': 'triangle', 'curve-style': 'bezier', 'arrow-scale': 0.8,
      }},
      { selector: '.caller-edge', style: { 'line-color': '#2196f3', 'target-arrow-color': '#2196f3' }},
      { selector: '.callee-edge', style: { 'line-color': '#4caf50', 'target-arrow-color': '#4caf50' }},
    ],
    layout: { name: 'preset' },
  });
  cy.on('tap', 'node', e => selectNode(parseInt(e.target.id())));
}

function renderTraceGraph(trace) {
  if (!cy) return;
  cy.elements().remove();
  const totalCount = (trace.caller_count || 0) + (trace.callee_count || 0);
  if (totalCount > TRACE_GRAPH_THRESHOLD) {
    document.getElementById('cy').innerHTML =
      '<div style="display:flex;flex-direction:column;align-items:center;justify-content:center;height:100%;color:#a0a0a0;text-align:center;padding:20px;">' +
      '<div style="font-size:48px;font-weight:700;color:#e94560;margin-bottom:12px;">' + totalCount + '</div>' +
      '<div style="font-size:14px;">upstream/downstream nodes — graph view hidden for large counts. Use the detail panel to browse.</div>' +
      '</div>';
    return;
  }
  document.getElementById('cy').innerHTML = '';
  const els = [];
  const seen = new Set();

  els.push({ data: { id: String(trace.target.id), label: trace.target.key, color: TAG_COLORS[trace.target.type] || '#999' }, classes: 'target' });
  seen.add(trace.target.id);

  function addTree(nodes, parentId, dir) {
    for (const n of nodes) {
      if (!seen.has(n.id)) {
        els.push({ data: { id: String(n.id), label: n.key, color: TAG_COLORS[n.type] || '#999' }});
        seen.add(n.id);
      }
      if (dir === 'caller') {
        els.push({ data: { source: String(n.id), target: String(parentId) }, classes: 'caller-edge' });
      } else {
        els.push({ data: { source: String(parentId), target: String(n.id) }, classes: 'callee-edge' });
      }
      addTree(n.children || [], n.id, dir);
    }
  }

  addTree(trace.callers || [], trace.target.id, 'caller');
  addTree(trace.callees || [], trace.target.id, 'callee');

  cy.add(els);
  cy.layout({ name: 'dagre', rankDir: 'TB', spacingFactor: 1.2, nodeSep: 30, rankSep: 60 }).run();
  cy.fit(undefined, 40);
}

function showDetail(d) {
  document.getElementById('detail-title').textContent = d.type + ' ' + d.key;
  let h = '<div class="section-title first">Degree</div>';
  h += '<div>in:' + d.in_degree + ' out:' + d.out_degree + ' total:' + (d.in_degree + d.out_degree) + '</div>';
  h += '<div class="section-title">Callers (' + d.in_degree + ')</div>';
  h += '<div id="callers-list" class="neighbor-list"><div class="loading">Loading...</div></div>';
  h += '<div class="section-title">Callees (' + d.out_degree + ')</div>';
  h += '<div id="callees-list" class="neighbor-list"><div class="loading">Loading...</div></div>';
  document.getElementById('detail-content').innerHTML = h;
  document.getElementById('detail-panel').classList.remove('hidden');
  loadNeighbors(d.id, 'callers', 0);
  loadNeighbors(d.id, 'callees', 0);
  requestAnimationFrame(() => { if (cy) { cy.resize(); cy.fit(undefined, 40); } });
}

async function loadNeighbors(id, dir, offset) {
  const container = document.getElementById(dir + '-list');
  if (!container) return;
  if (offset === 0) container.innerHTML = '<div class="loading">Loading...</div>';

  const data = await api('/nodes/' + id + '/' + dir + '?limit=50&offset=' + offset);
  const nodes = data.nodes || [];
  const total = data.total || 0;

  if (offset === 0) container.innerHTML = '';

  for (const n of nodes) {
    const el = document.createElement('div');
    el.className = 'detail-node';
    el.innerHTML = '<span class="node-tag" style="color:' + (TAG_COLORS[n.type] || '#999') + '">' + n.type + '</span> ' + esc(n.key);
    el.onclick = () => selectNode(n.id);
    container.appendChild(el);
  }

  const remaining = total - (offset + nodes.length);
  const existingLoadMore = container.querySelector('.load-more');
  if (existingLoadMore) existingLoadMore.remove();

  if (remaining > 0) {
    const btn = document.createElement('div');
    btn.className = 'load-more';
    btn.textContent = 'Load more (' + remaining + ' remaining)';
    btn.onclick = () => loadNeighbors(id, dir, offset + nodes.length);
    container.appendChild(btn);
  }
}

function hideDetail() {
  document.getElementById('detail-panel').classList.add('hidden');
  selectedNodeId = null;
  renderVirtualList();
  requestAnimationFrame(() => { if (cy) { cy.resize(); cy.fit(undefined, 40); } });
}

document.getElementById('node-list').addEventListener('scroll', renderVirtualList);

document.querySelectorAll('.panel-divider').forEach(divider => {
  let startX, startLeftWidth, leftPanel, rightPanel;

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
    const newWidth = Math.max(parseInt(getComputedStyle(leftPanel).minWidth) || 150, startLeftWidth + dx);
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

document.addEventListener('DOMContentLoaded', init);

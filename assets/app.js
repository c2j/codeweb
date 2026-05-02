// codeweb browser UI
let cy = null;
let allNodes = [];
let selectedNodeId = null;

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
  si.addEventListener('input', () => { clearTimeout(t); t = setTimeout(() => loadNodes(si.value), 200); });

  document.addEventListener('keydown', e => {
    if (e.key === '/' && document.activeElement !== si) { e.preventDefault(); si.focus(); }
    if (e.key === 'Escape') { hideDetail(); si.blur(); }
  });

  document.getElementById('detail-close').addEventListener('click', hideDetail);
}

async function loadNodes(search) {
  const data = await api('/nodes' + (search ? '?search=' + encodeURIComponent(search) : ''));
  allNodes = data;
  renderNodeList();
  document.getElementById('node-count').textContent = allNodes.length + ' nodes';
}

function renderNodeList() {
  document.getElementById('node-list').innerHTML = allNodes.map(n =>
    '<div class="node-item' + (n.id === selectedNodeId ? ' active' : '') +
    '" onclick="selectNode(' + n.id + ')" data-id="' + n.id + '">' +
    '<span class="node-tag" style="color:' + (TAG_COLORS[n.type] || '#999') + '">' + n.type + '</span>' +
    '<span class="node-key" title="' + esc(n.key) + '">' + esc(n.key) + '</span>' +
    '<span class="node-degree">' + n.in_degree + '/' + n.out_degree + '</span></div>'
  ).join('');
}

function esc(s) { return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }

async function selectNode(id) {
  selectedNodeId = id;
  renderNodeList();
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
  const els = [];
  const seen = new Set();

  // target node
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
  if (d.callers && d.callers.length) {
    h += '<div class="section-title">Callers (' + d.callers.length + ')</div>';
    d.callers.forEach(c => {
      h += '<div class="detail-node" onclick="selectNode(' + c.id + ')"><span class="node-tag" style="color:' + (TAG_COLORS[c.type] || '#999') + '">' + c.type + '</span> ' + esc(c.key) + '</div>';
    });
  }
  if (d.callees && d.callees.length) {
    h += '<div class="section-title">Callees (' + d.callees.length + ')</div>';
    d.callees.forEach(c => {
      h += '<div class="detail-node" onclick="selectNode(' + c.id + ')"><span class="node-tag" style="color:' + (TAG_COLORS[c.type] || '#999') + '">' + c.type + '</span> ' + esc(c.key) + '</div>';
    });
  }
  document.getElementById('detail-content').innerHTML = h;
  document.getElementById('detail-panel').classList.remove('hidden');
  requestAnimationFrame(() => { if (cy) { cy.resize(); cy.fit(undefined, 40); } });
}

function hideDetail() {
  document.getElementById('detail-panel').classList.add('hidden');
  selectedNodeId = null;
  renderNodeList();
  requestAnimationFrame(() => { if (cy) { cy.resize(); cy.fit(undefined, 40); } });
}

document.addEventListener('DOMContentLoaded', init);

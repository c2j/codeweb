let allNodes = [];
let allNodesTotal = 0;
let selectedNodeId = null;
let currentTraceKey = null;
let searchMode = 'name'; // 'name' or 'sql'
let showProperties = localStorage.getItem('codeweb-props-expanded') !== 'false';
let currentOffset = 0;
let isLoading = false;
let navHistory = [];
let isNavigatingBack = false;

const ITEM_HEIGHT = 36;
const BUFFER = 5;

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
  const projectName = stats.project_name || 'codeweb';
  document.title = 'Codeweb-' + projectName;
  document.getElementById('title').textContent = 'codeweb';
  document.getElementById('project-name').textContent = projectName;
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

  const si = document.getElementById('search-input');
  let t;
  si.addEventListener('input', () => { clearTimeout(t); t = setTimeout(() => loadNodes(si.value), 500); });

  document.addEventListener('keydown', e => {
    if (e.key === '/' && document.activeElement !== si) { e.preventDefault(); si.focus(); }
    if (e.key === 'Escape') { hideDetail(); si.blur(); }
  });

  document.getElementById('detail-close').addEventListener('click', hideDetail);

  document.querySelectorAll('.search-mode-tab').forEach(tab => {
    tab.addEventListener('click', () => {
      document.querySelectorAll('.search-mode-tab').forEach(t => t.classList.remove('active'));
      tab.classList.add('active');
      searchMode = tab.dataset.mode;
      si.placeholder = searchMode === 'sql' ? 'Search SQL content...' : 'Search by name... (press /)';
      si.value = '';
      loadNodes();
    });
  });

  document.getElementById('detail-panel').addEventListener('click', function(e) {
    if (e.target.closest('.copy-btn')) return;
    const treeNode = e.target.closest('.tree-node');
    if (treeNode && treeNode.dataset.key) {
      navigateTo(treeNode.dataset.key);
    }
  });

  document.getElementById('detail-back').addEventListener('click', goBack);
}

async function loadNodes(search) {
  const q = search || document.getElementById('search-input').value;
  currentOffset = 0;
  isLoading = true;
  renderVirtualList();
  let data;
  if (searchMode === 'sql' && q) {
    data = await api('/nodes/search-sql?q=' + encodeURIComponent(q));
    allNodes = data.nodes;
    allNodesTotal = data.total;
  } else if (q) {
    data = await api('/nodes?limit=100&offset=0&search=' + encodeURIComponent(q));
    allNodes = data.nodes;
    allNodesTotal = data.total;
    currentOffset = data.nodes.length;
  } else {
    data = await api('/nodes?limit=100&offset=0');
    allNodes = data.nodes;
    allNodesTotal = data.total;
    currentOffset = data.nodes.length;
  }
  isLoading = false;
  renderVirtualList();
  updateNodeCount();
}

async function loadMore() {
  if (isLoading || allNodes.length >= allNodesTotal) return;
  if (searchMode === 'sql') return;
  isLoading = true;
  const q = document.getElementById('search-input').value;
  let url = '/nodes?limit=100&offset=' + currentOffset;
  if (q) url += '&search=' + encodeURIComponent(q);
  try {
    const data = await api(url);
    allNodes = allNodes.concat(data.nodes);
    allNodesTotal = data.total;
    currentOffset += data.nodes.length;
  } catch (_) {}
  isLoading = false;
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

function renderSqlSnippet(bodySql) {
  if (!bodySql || bodySql.length === 0) return '';
  const matched = bodySql.find(s => s.matched) || bodySql[0];
  const line = (matched.sql || '').split('\n')[0].trim();
  if (!line) return '';
  const display = line.substring(0, 120) + (line.length > 120 ? '...' : '');
  const matchClass = matched.matched ? ' sql-matched' : ' sql-unmatched';
  return '<div class="sql-snippet' + matchClass + '"><span class="sql-kind">' + esc(matched.kind) + '</span> <span class="sql-text">' + esc(display) + '</span></div>';
}

function renderVirtualList() {
  const list = document.getElementById('node-list');
  if (isLoading) {
    list.innerHTML = '<div class="loading-spinner">Searching...</div>';
    return;
  }
  const scrollTop = list.scrollTop;
  const viewHeight = list.clientHeight;
  const count = allNodes.length;

  // Precompute heights and cumulative Y positions for all items
  const heights = [];
  const cumY = [0]; // cumY[i] = Y position of item i
  for (let i = 0; i < count; i++) {
    const n = allNodes[i];
    if (!n) { heights[i] = 0; continue; }
    const snippet = searchMode === 'sql' ? renderSqlSnippet(n.body_sql) : '';
    heights[i] = snippet ? ITEM_HEIGHT + 20 : ITEM_HEIGHT;
    cumY[i + 1] = cumY[i] + heights[i];
  }

  // Binary search to find the actual index at scrollTop
  let lo = 0, hi = count;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (cumY[mid] < scrollTop) lo = mid + 1;
    else hi = mid;
  }
  const actualStart = Math.max(0, lo - 1 - BUFFER);
  const startIdx = Math.max(0, actualStart);
  const endIdx = Math.min(count, lo + Math.ceil(viewHeight / ITEM_HEIGHT) + BUFFER);

  const totalHeight = cumY[count];

  let html = '<div style="height:' + totalHeight + 'px;position:relative">';
  for (let i = startIdx; i < endIdx; i++) {
    const n = allNodes[i];
    if (!n) continue;
    const snippet = searchMode === 'sql' ? renderSqlSnippet(n.body_sql) : '';
    const itemExtra = snippet ? ' node-item-with-snippet' : '';
    html += '<div class="node-item' + (n.id === selectedNodeId ? ' active' : '') + itemExtra +
      '" style="position:absolute;top:' + cumY[i] + 'px;left:0;right:0;height:' + heights[i] + 'px"' +
      ' onclick="selectNode(' + n.id + ')" data-id="' + n.id + '">' +
      '<div class="node-item-row">' +
      '<span class="node-tag" style="color:' + (TAG_COLORS[n.type] || '#999') + '">' + n.type + '</span>' +
      '<span class="node-key" title="' + esc(n.key) + '">' + esc(n.key) + '</span>' +
      '<span class="copy-btn" onclick="event.stopPropagation(); copyToClipboard(this)" data-copy-key="' + esc(n.key) + '" title="Copy name">📋</span>' +
      (n.score != null ? '<span class="node-score">' + Math.round(n.score * 100) + '%</span>' : '') +
      '<span class="node-degree">' + n.in_degree + '/' + n.out_degree + '</span>' +
      '</div>' + snippet + '</div>';
  }
  html += '</div>';
  list.innerHTML = html;
}

function esc(s) { return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }

async function copyToClipboard(btn) {
  const text = btn.dataset.copyKey;
  try {
    await navigator.clipboard.writeText(text);
    document.querySelectorAll('.copy-btn.copy-success').forEach(b => {
      b.classList.remove('copy-success');
      b.textContent = '\ud83d\udccb';
    });
    btn.textContent = '\u2713';
    btn.classList.add('copy-success');
    setTimeout(() => {
      btn.textContent = '\ud83d\udccb';
      btn.classList.remove('copy-success');
    }, 1500);
  } catch (_) {}
}

function updateBackButton() {
  const btn = document.getElementById('detail-back');
  if (!btn) return;
  btn.style.display = navHistory.length > 0 ? '' : 'none';
}

async function goBack() {
  if (navHistory.length === 0) return;
  const prevKey = navHistory.pop();
  isNavigatingBack = true;
  await navigateTo(prevKey);
}

async function selectNode(id) {
  selectedNodeId = id;
  const node = allNodes.find(n => n.id === id);
  if (!node) { renderVirtualList(); return; }
  await navigateTo(node.key);
}

async function navigateTo(key) {
  if (!isNavigatingBack && currentTraceKey && currentTraceKey !== key) {
    navHistory.push(currentTraceKey);
  }
  isNavigatingBack = false;

  const node = allNodes.find(n => n.key === key);
  selectedNodeId = node ? node.id : null;
  renderVirtualList();

  currentTraceKey = key;
  const [trace, detail] = await Promise.all([
    api('/trace?from=' + encodeURIComponent(key) + '&depth=50&max_nodes=500'),
    selectedNodeId !== null ? api('/nodes/' + selectedNodeId) : Promise.resolve(null),
  ]);
  if (currentTraceKey !== key) return;
  showDetail(trace, detail, searchMode);
  updateBackButton();
}

function toggleProperties() {
  showProperties = !showProperties;
  localStorage.setItem('codeweb-props-expanded', showProperties);
  const el = document.getElementById('prop-content');
  if (el) el.style.display = showProperties ? '' : 'none';
  document.getElementById('prop-toggle').textContent = showProperties ? '[-]' : '[+]';
}

function renderPropertiesHtml(detail, showBodySql) {
  if (!detail || !detail.properties || detail.properties.length === 0) return '';
  const expanded = showProperties;
  let h = '<div class="section-title" style="cursor:pointer" onclick="toggleProperties()">Properties <span id="prop-toggle">' + (expanded ? '[-]' : '[+]') + '</span></div>';
  h += '<div id="prop-content" style="display:' + (expanded ? 'block' : 'none') + '"><div class="prop-list">';
  for (const p of detail.properties) {
    if (p.label === 'body_sql') {
      if (showBodySql) {
        h += '<div class="prop-entry"><span class="prop-label">Procedure SQL (' + p.value.length + ' statements):</span></div>';
        for (const s of p.value) {
          h += '<div class="prop-entry"><span class="prop-label dim">[' + esc(s.kind) + ']</span></div>';
          h += '<pre class="prop-sql">' + esc(s.sql) + '</pre>';
        }
      }
    } else if (Array.isArray(p.value)) {
      h += '<div class="prop-entry"><span class="prop-label">' + esc(p.label) + ':</span> (' + p.value.length + ')</div>';
      for (const col of p.value.slice(0, 10)) {
        h += '<div class="prop-col"><span class="prop-col-name">' + esc(col.name) + '</span> <span class="prop-col-type">' + esc(col.type) + '</span>' + (col.pk ? ' <span class="prop-pk">PK</span>' : '') + '</div>';
      }
      if (p.value.length > 10) {
        h += '<div class="prop-col dim">... +' + (p.value.length - 10) + ' more</div>';
      }
    } else if (p.label === 'sql') {
      h += '<div class="prop-entry"><span class="prop-label">' + esc(p.label) + ':</span></div>';
      h += '<pre class="prop-sql">' + esc(p.value) + '</pre>';
    } else {
      h += '<div class="prop-entry"><span class="prop-label">' + esc(p.label) + ':</span> <span class="prop-val">' + esc(String(p.value)) + '</span></div>';
    }
  }
  h += '</div></div>';
  return h;
}

function showDetail(trace, detail, mode) {
  const target = trace.target;
  const inDeg = trace.caller_count;
  const outDeg = trace.callee_count;
  const showBodySql = mode === 'sql';

  showProperties = localStorage.getItem('codeweb-props-expanded') !== 'false';
  document.getElementById('detail-title').textContent = target.type + ' ' + target.key;

  let h = '<div class="section-title first">Degree</div>';
  h += '<div>in:' + inDeg + ' out:' + outDeg + ' total:' + (inDeg + outDeg) + '</div>';

  h += renderPropertiesHtml(detail, showBodySql);

  h += '<div class="section-title">Callers (' + inDeg + ')</div>';
  h += '<div class="tree-list">';
  h += renderTreeHtml(trace.callers);
  h += '</div>';

  h += '<div class="section-title">Callees (' + outDeg + ')</div>';
  h += '<div class="tree-list">';
  h += renderTreeHtml(trace.callees);
  h += '</div>';

  if (trace.truncated) {
    h += '<div class="truncated">Results truncated \u2014 too many nodes</div>';
  }

  document.getElementById('detail-content').innerHTML = h;
  document.getElementById('detail-panel').classList.remove('hidden');
}

function renderTreeHtml(nodes, prefixes) {
  if (!nodes || nodes.length === 0) return '<div class="tree-empty">(none)</div>';
  prefixes = prefixes || [];
  let html = '';
  for (let i = 0; i < nodes.length; i++) {
    const n = nodes[i];
    const isLast = i === nodes.length - 1;
    const connector = isLast ? '\u2514\u2500\u2500 ' : '\u251c\u2500\u2500 ';
    const prefix = prefixes.join('');
    const label = n.edge_label ? ' <span class="edge-label">' + esc(n.edge_label) + '</span>' : '';

    html += '<div class="tree-node" data-key="' + esc(n.key) + '">';
    html += '<span class="tree-prefix">' + esc(prefix + connector) + '</span>';
    html += '<span class="node-tag" style="color:' + (TAG_COLORS[n.type] || '#999') + '">' + n.type + '</span>';
    html += '<span class="tree-key">' + esc(n.key) + '</span>';
    html += '<span class="copy-btn" onclick="event.stopPropagation(); copyToClipboard(this)" data-copy-key="' + esc(n.key) + '" title="Copy name">📋</span>';
    html += label;
    html += '</div>';

    if (n.children && n.children.length > 0) {
      const childPrefix = isLast ? '    ' : '\u2502   ';
      html += renderTreeHtml(n.children, prefixes.concat([childPrefix]));
    }
  }
  return html;
}

function hideDetail() {
  document.getElementById('detail-panel').classList.add('hidden');
  selectedNodeId = null;
  currentTraceKey = null;
  navHistory = [];
  renderVirtualList();
}

document.getElementById('node-list').addEventListener('scroll', function() {
  renderVirtualList();
  const el = this;
  if (el.scrollTop + el.clientHeight >= el.scrollHeight - ITEM_HEIGHT * BUFFER) {
    loadMore();
  }
});

document.querySelectorAll('.panel-divider').forEach(divider => {
  let startX, startLeftWidth, leftPanel;

  divider.addEventListener('mousedown', e => {
    e.preventDefault();
    leftPanel = document.getElementById(divider.dataset.left);
    if (!leftPanel) return;
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
  }

  function onStop() {
    divider.classList.remove('active');
    document.removeEventListener('mousemove', onDrag);
    document.removeEventListener('mouseup', onStop);
  }
});

document.addEventListener('DOMContentLoaded', init);

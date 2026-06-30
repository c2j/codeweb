# Codeweb Serve 模式 — HTTP API 调用指南

## 概述

Codeweb 的 serve 模式启动一个本地 HTTP 服务器，提供 RESTful API 用于查询和分析代码图谱。同时内嵌浏览器 UI（Cytoscape.js 图可视化）。

## 启动服务

```bash
# 构建 serve 模式
cargo build --features serve

# 启动服务（默认地址 127.0.0.1:3000）
cargo run --features serve -- serve

# 指定项目目录和监听地址
cargo run --features serve -- serve --project /path/to/project --addr 0.0.0.0:8080

# 启动后自动打开浏览器
cargo run --features serve -- serve --open
```

服务启动后，浏览器 UI 可通过 `http://<addr>/` 访问，所有 API 端点以 `/api/v1/` 为前缀。

---

## API 端点总览

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/stats` | 项目统计信息 |
| GET | `/api/v1/files` | 已分析文件列表 |
| GET | `/api/v1/nodes` | 节点列表（支持筛选/搜索/分页） |
| GET | `/api/v1/nodes/:id` | 节点详情（含属性、callers、callees） |
| GET | `/api/v1/nodes/:id/callers` | 节点的上游调用方 |
| GET | `/api/v1/nodes/:id/callees` | 节点的下游被调用方 |
| GET | `/api/v1/nodes/search-sql` | 按 SQL 文本内容搜索节点 |
| GET | `/api/v1/trace` | 双向调用链追踪 |
| POST | `/api/v1/query` | 执行声明式查询（QuerySpec） |
| GET | `/api/v1/export` | 导出图谱（DOT/JSON/Mermaid） |
| GET | `/api/v1/graph` | 完整图谱数据（JSON） |

所有响应均为 JSON 格式（`export` 端点的 DOT/Mermaid 除外）。服务启用了 CORS，可从前端应用直接调用。

---

## 1. GET `/api/v1/stats` — 项目统计

返回图中各类节点和边的数量统计。

### 请求

```bash
curl http://127.0.0.1:3000/api/v1/stats
```

### 响应

```json
{
  "procedures": 42,
  "functions": 15,
  "unresolved": 3,
  "mappers": 28,
  "java_sql": 10,
  "java_methods": 85,
  "java_classes": 12,
  "tables": 35,
  "views": 8,
  "packages": 5,
  "triggers": 2,
  "types": 1,
  "sequences": 3,
  "indexes": 7,
  "materialized_views": 0,
  "synonyms": 0,
  "events": 0,
  "custom_nodes": 0,
  "custom_edges": 0,
  "edges": 156,
  "files": 48
}
```

---

## 2. GET `/api/v1/files` — 已分析文件列表

返回项目中所有已分析文件的信息。

### 请求

```bash
curl http://127.0.0.1:3000/api/v1/files
```

### 响应

```json
[
  {
    "path": "src/main/sql/init.sql",
    "type": "SQL",
    "nodes": 5
  },
  {
    "path": "src/main/java/com/example/UserMapper.java",
    "type": "Java",
    "nodes": 3
  },
  {
    "path": "src/main/resources/mapper/UserMapper.xml",
    "type": "XML",
    "nodes": 4
  }
]
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `path` | string | 相对于项目根目录的文件路径 |
| `type` | string | 文件类型：`SQL`、`Java`、`XML` |
| `nodes` | number | 该文件中定义的图节点数量 |

---

## 3. GET `/api/v1/nodes` — 节点列表

查询图中的节点，支持按名称搜索、类型过滤、连接度过滤和分页。

### 请求参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `search` | string | 否 | — | 按名称搜索（子串匹配，不区分大小写），结果按匹配度排序 |
| `node_type` | string | 否 | — | 按类型过滤（见下方类型标签表） |
| `orphan` | boolean | 否 | `false` | 设为 `true` 只显示孤立节点（无任何连接） |
| `low_degree` | number | 否 | — | 只显示总连接度 ≤ N 的节点 |
| `inferred` | boolean | 否 | `false` | 设为 `true` 只显示推测型节点（表/视图无 DDL 定义） |
| `system` | boolean | 否 | `false` | 设为 `true` 只显示系统对象（pg_catalog、sys、dbe_* 等 schema） |
| `limit` | number | 否 | `100` | 返回结果数量上限 |
| `offset` | number | 否 | `0` | 分页偏移量 |

### 节点类型标签

> 带 `*` 后缀的标签表示推测型节点（在 DML 中引用但无对应 DDL 定义）。

| node_type 值 | 说明 |
|--------------|------|
| `proc` | 存储过程 |
| `proc*` | 存储过程（partial，body 未解析） |
| `func` | 函数 |
| `func*` | 函数（partial） |
| `table` | 表（有 DDL 定义） |
| `table*` | 表（推测型，无 DDL） |
| `view` | 视图（有 DDL 定义） |
| `view*` | 视图（推测型，无 DDL） |
| `mapper` | MyBatis/iBatis MappedStatement |
| `sql` | Java 中内嵌的 SQL |
| `method` | Java 方法 |
| `class` | Java 类 |
| `pkg` | 包 |
| `trigger` | 触发器 |
| `type` | 自定义类型 |
| `seq` | 序列 |
| `index` | 索引 |
| `mview` | 物化视图 |
| `synonym` | 同义词 |
| `event` | 事件 |
| `builtin` | 内建函数 |
| `unres` | 未解析引用 |

### 请求示例

```bash
# 搜索名称包含 "order" 的节点
curl "http://127.0.0.1:3000/api/v1/nodes?search=order"

# 只看存储过程
curl "http://127.0.0.1:3000/api/v1/nodes?node_type=proc"

# 查看孤立节点
curl "http://127.0.0.1:3000/api/v1/nodes?orphan=true"

# 分页查询
curl "http://127.0.0.1:3000/api/v1/nodes?limit=20&offset=40"
```

### 响应

```json
{
  "total": 156,
  "limit": 100,
  "offset": 0,
  "nodes": [
    {
      "id": 0,
      "key": "proc:public.calculate_order",
      "type": "proc",
      "in_degree": 3,
      "out_degree": 5
    },
    {
      "id": 1,
      "key": "table:public.orders",
      "type": "table",
      "in_degree": 12,
      "out_degree": 0
    }
  ]
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `total` | number | 符合条件的总节点数（分页前） |
| `limit` | number | 当前请求的 limit |
| `offset` | number | 当前请求的 offset |
| `nodes[].id` | number | 节点 ID（用于其他 API 的 `:id` 参数） |
| `nodes[].key` | string | 节点的唯一标识键，格式：`<type>:<schema>.<name>` |
| `nodes[].type` | string | 节点类型标签 |
| `nodes[].in_degree` | number | 入度（被多少节点引用） |
| `nodes[].out_degree` | number | 出度（引用了多少节点） |

---

## 4. GET `/api/v1/nodes/:id` — 节点详情

获取某个节点的完整信息，包括属性、上游调用方和下游被调用方。

### 请求

```bash
curl http://127.0.0.1:3000/api/v1/nodes/0
```

### 响应

```json
{
  "id": 0,
  "key": "proc:public.calculate_order",
  "type": "proc",
  "in_degree": 3,
  "out_degree": 5,
  "callers": [
    {
      "id": 10,
      "key": "proc:public.process_payment",
      "type": "proc"
    }
  ],
  "callees": [
    {
      "id": 1,
      "key": "table:public.orders",
      "type": "table"
    }
  ],
  "properties": [
    { "label": "schema", "value": "public" },
    { "label": "package", "value": null },
    { "label": "name", "value": "calculate_order" },
    { "label": "file", "value": "src/main/sql/orders.sql" },
    { "label": "line", "value": 42 }
  ]
}
```

`properties` 字段因节点类型不同而异：

**存储过程/函数（proc/func）**：`schema`、`package`、`name`、`file`、`line`、`partial`（仅当为 true 时出现）

**表（table）**：`schema`、`name`、`file`、`line`、`columns`（数组，含 `name`/`type`/`nullable`/`pk`）、`temporary`、`unlogged`、`tablespace`、`ddl`

**MappedStatement（mapper）**：`namespace`、`statement_id`、`kind`、`file`、`line`、`sql`

**Java 内嵌 SQL（sql）**：`file`、`line`、`class`、`method`、`extraction`、`sql`

**Java 方法（method）**：`fqn`、`class_fqn`、`name`、`signature`、`file`、`line`

节点不存在时返回 `404`。

---

## 5. GET `/api/v1/nodes/:id/callers` — 上游调用方

获取调用（指向）指定节点的所有上游节点，支持分页。

### 请求参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `limit` | number | 否 | `50` | 返回结果数量上限 |
| `offset` | number | 否 | `0` | 分页偏移量 |

### 请求示例

```bash
curl http://127.0.0.1:3000/api/v1/nodes/1/callers
curl "http://127.0.0.1:3000/api/v1/nodes/1/callers?limit=10&offset=0"
```

### 响应

```json
{
  "total": 5,
  "limit": 50,
  "offset": 0,
  "nodes": [
    {
      "id": 0,
      "key": "proc:public.calculate_order",
      "type": "proc"
    }
  ]
}
```

节点不存在时返回 `404`。

---

## 6. GET `/api/v1/nodes/:id/callees` — 下游被调用方

获取指定节点调用（指向）的所有下游节点，支持分页。

### 请求参数

与 callers 相同：`limit`（默认 50）、`offset`（默认 0）。

### 请求示例

```bash
curl http://127.0.0.1:3000/api/v1/nodes/0/callees
```

### 响应

格式与 callers 相同。

---

## 7. GET `/api/v1/nodes/search-sql` — 按 SQL 文本搜索

搜索 MappedStatement 和 JavaSql 节点中包含指定 SQL 片段的节点。搜索为子串匹配，不区分大小写。支持 `?` 通配符匹配任意非空字符串。

### 请求参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `q` | string | 是 | SQL 片段（子串匹配，不区分大小写） |

### 请求示例

```bash
# 精确搜索
curl "http://127.0.0.1:3000/api/v1/nodes/search-sql?q=SELECT%20*%20FROM%20orders"

# 使用 ? 通配符匹配参数占位符
curl "http://127.0.0.1:3000/api/v1/nodes/search-sql?q=user_id=?%20AND%20status=?"
```

### 响应

```json
{
  "total": 2,
  "nodes": [
    {
      "id": 15,
      "key": "mapper:com.example.UserMapper.selectByStatus",
      "type": "mapper",
      "in_degree": 1,
      "out_degree": 0
    },
    {
      "id": 22,
      "key": "javasql:UserDao.findById",
      "type": "sql",
      "in_degree": 0,
      "out_degree": 0
    }
  ]
}
```

---

## 8. GET `/api/v1/trace` — 双向调用链追踪

从指定节点出发，同时向上（callers）和向下（callees）追踪调用链，返回树形结构。

### 请求参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `from` | string | 是 | — | 节点名称搜索（子串匹配，取第一个匹配结果） |
| `depth` | number | 否 | `2` | 追踪深度：0=仅目标，1=直接调用方/被调用方，N=N层（最大 10） |
| `max_nodes` | number | 否 | `500` | 最大访问节点数（防止图过大） |

### 请求示例

```bash
# 追踪 calculate_order 的 3 层调用链
curl "http://127.0.0.1:3000/api/v1/trace?from=calculate_order&depth=3"

# 指定最大节点数
curl "http://127.0.0.1:3000/api/v1/trace?from=process_payment&depth=5&max_nodes=200"
```

### 响应

```json
{
  "target": {
    "id": 0,
    "key": "proc:public.calculate_order",
    "type": "proc"
  },
  "callers": [
    {
      "id": 10,
      "key": "proc:public.process_payment",
      "type": "proc",
      "edge_label": "intra_call",
      "children": [
        {
          "id": 20,
          "key": "method:com.example.PaymentService.process",
          "type": "method",
          "edge_label": "calls_java",
          "children": []
        }
      ]
    }
  ],
  "callees": [
    {
      "id": 1,
      "key": "table:public.orders",
      "type": "table",
      "edge_label": "table_access",
      "children": []
    }
  ],
  "caller_count": 3,
  "callee_count": 5,
  "truncated": false
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `target` | object | 起始节点 |
| `callers` | array | 上游调用链（树形结构） |
| `callees` | array | 下游被调用链（树形结构） |
| `caller_count` | number | 上游调用方总数 |
| `callee_count` | number | 下游被调用方总数 |
| `truncated` | boolean | 是否因 `max_nodes` 限制而截断 |

每个树节点包含：
- `id`：节点 ID
- `key`：节点标识键
- `type`：节点类型
- `edge_label`：连到父节点的边类型（如 `intra_call`、`cross_call`、`table_access`、`invokes_mapper`、`calls_java` 等）
- `children`：子节点数组

无匹配节点时返回 `404`。

---

## 9. POST `/api/v1/query` — 声明式查询

执行一个 JSON 格式的 QuerySpec，支持多步骤遍历、过滤和多种收集模式。这是最灵活的查询接口。

### 请求体（QuerySpec）

```json
{
  "start": {
    "type": "proc",
    "name": "calculate",
    "schema": "public"
  },
  "steps": [
    {
      "action": "outgoing",
      "edge_categories": ["call"],
      "max_depth": 3
    }
  ],
  "collect": "nodes"
}
```

#### StartSpec — 起始节点选择

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `type` | string | 否 | 节点类型标签（如 `proc`、`table`、`method`） |
| `name` | string | 否 | 节点名称（子串匹配） |
| `schema` | string | 否 | Schema 过滤 |

三个字段取交集——同时指定则必须全部匹配。至少提供一个。

#### StepSpec — 遍历步骤

`steps` 是一个步骤数组，按顺序依次执行：

| action | 说明 | 额外参数 |
|--------|------|---------|
| `outgoing` | 沿出边方向遍历 | `edge_categories`（可选）、`max_depth`（可选） |
| `incoming` | 沿入边方向遍历 | `edge_categories`（可选）、`max_depth`（可选） |
| `filter` | 过滤当前节点集 | `type_tag`（可选）、`schema`（可选） |
| `until` | 遍历直到遇到指定类型 | `type_tag`（必填） |

**edge_categories 可选值**：`call`、`composition`、`dataflow`、`reference`、`inheritance`

#### CollectMode — 收集模式

| 值 | 说明 |
|----|------|
| `nodes` | 收集所有到达的节点（去重后返回节点列表） |
| `paths` | 收集所有遍历路径 |
| `subgraph` | 子图模式 |

### 请求示例

```bash
# 查找所有名称包含 "order" 的存储过程
curl -X POST http://127.0.0.1:3000/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "start": { "type": "proc", "name": "order" },
    "collect": "nodes"
  }'

# 从某个存储过程出发，沿 call 边追踪 3 层
curl -X POST http://127.0.0.1:3000/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "start": { "type": "proc", "name": "calculate_order" },
    "steps": [
      { "action": "outgoing", "edge_categories": ["call"], "max_depth": 3 }
    ],
    "collect": "nodes"
  }'

# 追踪路径模式：从 Java 方法到存储过程
curl -X POST http://127.0.0.1:3000/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "start": { "type": "method", "name": "PaymentService" },
    "steps": [
      { "action": "outgoing", "max_depth": 5 }
    ],
    "collect": "paths"
  }'

# 反向影响分析：谁依赖了这张表
curl -X POST http://127.0.0.1:3000/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "start": { "type": "table", "name": "orders" },
    "steps": [
      { "action": "incoming", "max_depth": 3 }
    ],
    "collect": "nodes"
  }'

# 遍历直到遇到 Java 方法
curl -X POST http://127.0.0.1:3000/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "start": { "type": "proc", "name": "calculate" },
    "steps": [
      { "action": "incoming", "edge_categories": ["call"] },
      { "action": "until", "type_tag": "method" }
    ],
    "collect": "nodes"
  }'
```

### 响应

**collect: "nodes"**

```json
{
  "nodes": [
    {
      "index": 0,
      "key": "proc:public.calculate_order",
      "type_tag": "proc"
    }
  ],
  "paths": []
}
```

**collect: "paths"**

```json
{
  "nodes": [],
  "paths": [
    [
      { "index": 0, "key": "proc:public.calculate_order", "type_tag": "proc" },
      { "index": 5, "key": "table:public.orders", "type_tag": "table" }
    ]
  ]
}
```

查询语法错误或无匹配时返回 `400`，响应体为错误描述字符串。

---

## 10. GET `/api/v1/export` — 导出图谱

将完整的代码图谱导出为指定格式。

### 请求参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `format` | string | 是 | 导出格式：`dot`、`json`、`mermaid` |

### 请求示例

```bash
# 导出为 Graphviz DOT 格式
curl "http://127.0.0.1:3000/api/v1/export?format=dot" > graph.dot

# 导出为 JSON
curl "http://127.0.0.1:3000/api/v1/export?format=json" > graph.json

# 导出为 Mermaid 格式
curl "http://127.0.0.1:3000/api/v1/export?format=mermaid" > graph.mmd
```

### 响应

| format | Content-Type | 说明 |
|--------|-------------|------|
| `dot` | `text/vnd.graphviz` | Graphviz DOT 格式，可用 `dot` 命令渲染为图片 |
| `json` | `application/json` | 完整图谱 JSON |
| `mermaid` | `text/plain` | Mermaid 流程图语法，可直接在 Markdown 中渲染 |

不支持的格式返回 `400`。

---

## 11. GET `/api/v1/graph` — 完整图谱数据

返回完整的图谱 JSON 数据，Content-Type 为 `application/json`。

### 请求

```bash
curl http://127.0.0.1:3000/api/v1/graph
```

### 响应

与 `export?format=json` 等价，返回完整图谱的 JSON 序列化。

---

## 典型使用场景

### 场景 1：查找某个存储过程的所有调用方

```bash
# 1. 搜索节点获取 ID
curl "http://127.0.0.1:3000/api/v1/nodes?search=calculate_order"

# 2. 查看节点详情（含直接 callers/callees）
curl "http://127.0.0.1:3000/api/v1/nodes/0"

# 3. 追踪完整调用链
curl "http://127.0.0.1:3000/api/v1/trace?from=calculate_order&depth=5"
```

### 场景 2：查找使用某张表的所有存储过程

```bash
# 方法 1：通过节点详情查看 callers
curl "http://127.0.0.1:3000/api/v1/nodes?search=orders&node_type=table"
curl "http://127.0.0.1:3000/api/v1/nodes/1"

# 方法 2：通过 query API 反向遍历
curl -X POST http://127.0.0.1:3000/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "start": { "type": "table", "name": "orders" },
    "steps": [
      { "action": "incoming", "edge_categories": ["dataflow"], "max_depth": 1 }
    ],
    "collect": "nodes"
  }'
```

### 场景 3：从 Java 方法到数据库表的完整链路

```bash
curl -X POST http://127.0.0.1:3000/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "start": { "type": "method", "name": "PaymentService" },
    "steps": [
      { "action": "outgoing", "max_depth": 5 }
    ],
    "collect": "paths"
  }'
```

### 场景 4：通过 SQL 片段找到对应的 Java 调用方

```bash
# 1. 搜索 SQL 片段
curl "http://127.0.0.1:3000/api/v1/nodes/search-sql?q=SELECT+*+FROM+orders+WHERE+status"

# 2. 查看搜索结果的 callers（即调用该 SQL 的 Java 方法）
curl "http://127.0.0.1:3000/api/v1/nodes/15/callers"
```

### 场景 5：影响分析——评估删除某存储过程的影响范围

```bash
# 反向追踪所有依赖方
curl "http://127.0.0.1:3000/api/v1/trace?from=sp_calc_risk&depth=5&max_nodes=1000"
```

---

## 错误处理

| HTTP 状态码 | 说明 |
|-------------|------|
| `200` | 成功 |
| `400` | 请求参数错误（如 QuerySpec JSON 格式错误、不支持的导出格式） |
| `404` | 节点不存在（`node_detail`、`node_callers`、`node_callees`、`trace`） |
| `500` | 服务器内部错误 |

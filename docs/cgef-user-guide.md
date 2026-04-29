# CGEF 用户指南 — Codeweb 图谱导入与合并

## 概述

CGEF（Codeweb Graph Exchange Format）是 codeweb 定义的 JSON 图谱交换格式。企业内部工具按此格式输出私有代码的语义关系数据，codeweb 通过 `import` 子命令将其解析为独立的 GraphStore，再通过 `merge` 子命令与自有图谱合并，实现跨公有/私有边界的链路追溯。

**典型场景**：企业 A 拥有私有 ERP 系统的存储过程调用关系，无法提供源码给 codeweb 扫描。企业 A 的内部工具生成 CGEF 文件后，codeweb 导入并合并，即可在 trace 时看到 `Java方法 → ERP存储过程` 的完整链路。

---

## 1. CGEF 文档结构

一个 CGEF 文档是一个 JSON 文件，包含以下顶层字段：

```json
{
  "format_version": 1,
  "metadata": {
    "source": "erp-system",
    "generated_at": "2026-04-28T10:00:00Z",
    "description": "ERP 系统存储过程调用关系"
  },
  "node_schemas": { ... },
  "edge_schemas": { ... },
  "nodes": [ ... ],
  "edges": [ ... ]
}
```

| 字段 | 必填 | 说明 |
|------|------|------|
| `format_version` | ✅ | 格式版本，当前必须为 `1` |
| `metadata` | ✅ | 元数据，包含 `source`（来源标识）和 `generated_at`（ISO 8601 时间戳） |
| `node_schemas` | ❌ | 自定义节点类型声明（仅自定义类型需要） |
| `edge_schemas` | ❌ | 自定义边类型声明（仅自定义类型需要） |
| `nodes` | ✅ | 节数组（可以为空） |
| `edges` | ✅ | 边数组（可以为空） |

---

## 2. 标准节点类型

CGEF 支持 codeweb 的全部内置节点类型：

| type 值 | 说明 | key 必填字段 | key 可选字段 |
|---------|------|-------------|-------------|
| `procedure` | 存储过程 | `name` | `schema`, `package`, `kind` |
| `function` | 函数 | `name` | `schema`, `package`, `kind` |
| `table` | 表 | `name` | `schema` |
| `view` | 视图 | `name` | `schema` |
| `mapped_statement` | MyBatis 映射语句 | `namespace`, `statement_id`, `kind` | `xml_file`, `line` |
| `java_method` | Java 方法 | `fqn`, `class_fqn`, `name`, `signature` | `file`, `line` |
| `java_class` | Java 类 | `fqn`, `name` | `package`, `file`, `line` |
| `java_sql` | Java 中的 SQL | `extraction_method` | `class_name`, `method_name`, `java_file`, `line` |
| `package` | 包 | `name` | `schema` |
| `trigger` | 触发器 | `name` | `table` |
| `type` | 自定义类型 | `name` | `schema`, `type_kind` |
| `sequence` | 序列 | `name` | `schema` |
| `index` | 索引 | `table_name` | `name`, `table_schema`, `unique` |
| `materialized_view` | 物化视图 | `name` | `schema` |
| `synonym` | 同义词 | `name`, `target_name` | `schema`, `target_schema` |
| `event` | 事件 | `name` | — |
| `unresolved` | 未解析引用 | `raw_expr`, `context` | — |

### 节点示例

**存储过程**：
```json
{
  "id": "pkg_order.create_order",
  "type": "procedure",
  "key": {
    "schema": "public",
    "package": "pkg_order",
    "name": "create_order",
    "kind": "procedure"
  },
  "location": {
    "file": "sql/pkg_order.sql",
    "line": 15
  },
  "properties": {
    "param_count": 5
  }
}
```

**Java 方法**：
```json
{
  "id": "OrderService.placeOrder",
  "type": "java_method",
  "key": {
    "fqn": "com.example.OrderService.placeOrder(String, int)",
    "class_fqn": "com.example.OrderService",
    "name": "placeOrder",
    "signature": "(String, int)"
  },
  "location": {
    "file": "src/main/java/com/example/OrderService.java",
    "line": 42
  }
}
```

**表**：
```json
{
  "id": "t_orders",
  "type": "table",
  "key": {
    "schema": "public",
    "name": "t_orders"
  }
}
```

---

## 3. 标准边类型

| type 值 | 说明 | 特殊 properties |
|---------|------|-----------------|
| `direct` | 直接静态调用 | — |
| `dynamic` | 动态调用（EXECUTE） | `raw_expr` |
| `table_access` | 表访问 | `modes`（`read`/`write`/`lock_read`/`truncate`）, `write_kinds` |
| `calls_procedure` | Mapper/JavaSql 调用存储过程 | — |
| `invokes_mapper` | JavaSql 通过 namespace.method 调用 Mapper | — |
| `calls_java` | Java 代码调用 | — |
| `contains_method` | 类包含方法 | — |
| `extends` | 类继承 | — |
| `implements` | 接口实现 | — |
| `contains_routine` | 包含例程 | — |
| `triggers_routine` | 触发器调用例程 | — |
| `references_type` | 引用类型 | — |
| `uses_sequence` | 使用序列 | — |
| `indexes_table` | 索引表 | — |
| `aliases_object` | 同义词别名对象 | — |

### 边示例

**直接调用**：
```json
{
  "source": "pkg_order.create_order",
  "target": "pkg_inventory.update_stock",
  "type": "direct",
  "location": {
    "file": "sql/pkg_order.sql",
    "line": 22
  }
}
```

**表访问（写入）**：
```json
{
  "source": "pkg_order.create_order",
  "target": "t_orders",
  "type": "table_access",
  "location": {
    "file": "sql/pkg_order.sql",
    "line": 18
  },
  "properties": {
    "modes": ["write"],
    "write_kinds": ["insert"]
  }
}
```

**动态调用**：
```json
{
  "source": "pkg_report.run_report",
  "target": "unresolved_1",
  "type": "dynamic",
  "properties": {
    "raw_expr": "v_sql"
  }
}
```

---

## 4. 自定义节点与边类型

当标准类型无法满足需求时（如企业内部的 ESB 服务、消息队列、定时任务等），可以定义自定义类型。

### 4.1 声明自定义节点类型

在 `node_schemas` 中声明：

```json
{
  "node_schemas": {
    "esb_service": {
      "display_name": "ESB 服务",
      "key_fields": ["system", "service_name", "version"],
      "properties": {
        "protocol": { "type": "string", "description": "调用协议" },
        "endpoint": { "type": "string", "description": "服务端点 URL" },
        "timeout_ms": { "type": "integer", "description": "超时时间" }
      }
    }
  }
}
```

- `key_fields`：定义该类型节点的逻辑主键字段，codeweb 按 `(type_name, key_fields)` 去重
- `properties`：声明该类型节点可携带的属性及其类型

### 4.2 声明自定义边类型

在 `edge_schemas` 中声明：

```json
{
  "edge_schemas": {
    "calls_esb": {
      "display_name": "调用 ESB 服务",
      "source_types": ["procedure", "java_method"],
      "target_types": ["esb_service"],
      "properties": {
        "mapped_operation": { "type": "string", "description": "映射的操作名" }
      }
    }
  }
}
```

- `source_types` / `target_types`：约束该边类型允许的源/目标节点类型

### 4.3 使用自定义类型

声明后，在 `nodes` 和 `edges` 中使用：

```json
{
  "nodes": [
    {
      "id": "esb_place_order",
      "type": "esb_service",
      "key": {
        "system": "erp",
        "service_name": "placeOrder",
        "version": "v2"
      },
      "properties": {
        "protocol": "SOAP",
        "endpoint": "https://erp.internal/services/order",
        "timeout_ms": 5000
      }
    }
  ],
  "edges": [
    {
      "source": "pkg_order.create_order",
      "target": "esb_place_order",
      "type": "calls_esb",
      "properties": {
        "mapped_operation": "CREATE_ORDER"
      }
    }
  ]
}
```

### 4.4 自定义节点的去重规则

自定义节点按 `(type_name, key_fields)` 去重。key_fields 的值按 JSON key 字母排序后序列化，形成唯一标识。例如：

- `esb_service` + `{"service_name": "placeOrder", "system": "erp", "version": "v2"}` → 去重键

这意味着同一 CGEF 文件中两个 `esb_service` 节点如果 key 字段完全相同，会被视为同一节点。

---

## 5. CLI 命令

### 5.1 `codeweb import` — 导入 CGEF 文件

将 CGEF JSON 文件解析为独立的 GraphStore 文件。

```bash
codeweb import --file <cgef.json> --output <store.bincode> [--prefix <path-prefix>] [--name <store-name>]
```

**参数**：

| 参数 | 必填 | 说明 |
|------|------|------|
| `--file` | ✅ | CGEF JSON 文件路径 |
| `--output` | ✅ | 输出路径，根据扩展名选择格式（`.bincode` 或 `.json`） |
| `--prefix` | ❌ | 路径前缀，映射 CGEF 中的相对路径到本地路径 |
| `--name` | ❌ | GraphStore 名称（用于 `merge` 时标识来源） |

**示例**：

```bash
# 基本导入
codeweb import --file enterprise-graph.json --output erp-store.bincode

# 带路径前缀（将 CGEF 中的 sql/pkg.sql 映射为 /enterprise/module-a/sql/pkg.sql）
codeweb import --file enterprise-graph.json --output erp-store.bincode --prefix /enterprise/module-a

# 导出为 JSON 格式
codeweb import --file enterprise-graph.json --output erp-store.json
```

**输出示例**：
```
Imported 156 nodes, 243 edges from enterprise-graph.json
  Standard nodes: 142, Custom nodes: 14 (3 types)
  Standard edges: 230, Custom edges: 13 (2 types)
Output: erp-store.bincode
```

### 5.2 `codeweb merge` — 合并 GraphStore

将多个 GraphStore 合并为一个。

```bash
codeweb merge --output <merged.bincode> <store1.bincode> [store2.bincode ...]
```

**参数**：

| 参数 | 必填 | 说明 |
|------|------|------|
| `[STORES]...` | ✅ | 输入的 GraphStore 文件列表（位置参数） |
| `--output` / `-o` | ✅ | 合并后的输出路径（bincode 格式） |
| `--name` | ❌ | 合并后的项目名称，默认 `merged` |

**示例**：

```bash
# 合并自有分析结果和 ERP 导入结果
codeweb merge \
  --output merged-graph.bincode \
  my-project.bincode erp-store.bincode

# 指定合并后的项目名称
codeweb merge -o merged.bincode --name full-graph a.bincode b.bincode c.bincode
```

**合并规则**：
- 节点按 `NodeKey` 去重（相同 key 的节点合并为一条）
- 边按 `(source, target, type)` 三元组去重
- `TableAccess` 边的 `modes` 集合取并集
- 自定义边按 `(source, target, type_name)` 去重，`properties` 后者覆盖前者

---

## 6. 端到端示例

### 场景：私有 ERP 存储过程与公 有 Java 项目的链路追溯

**第一步**：企业内部工具生成 CGEF 文件 `erp-graph.json`

```json
{
  "format_version": 1,
  "metadata": {
    "source": "erp-system",
    "generated_at": "2026-04-28T10:00:00Z",
    "description": "ERP 存储过程调用关系"
  },
  "node_schemas": {
    "esb_service": {
      "display_name": "ESB 服务",
      "key_fields": ["system", "service_name", "version"],
      "properties": {
        "protocol": { "type": "string" },
        "endpoint": { "type": "string" }
      }
    }
  },
  "edge_schemas": {
    "calls_esb": {
      "display_name": "调用 ESB 服务",
      "source_types": ["procedure"],
      "target_types": ["esb_service"]
    }
  },
  "nodes": [
    {
      "id": "pkg_erp.create_order",
      "type": "procedure",
      "key": { "schema": "erp", "package": "pkg_erp", "name": "create_order", "kind": "procedure" },
      "location": { "file": "sql/pkg_erp.sql", "line": 10 }
    },
    {
      "id": "pkg_erp.check_inventory",
      "type": "procedure",
      "key": { "schema": "erp", "package": "pkg_erp", "name": "check_inventory", "kind": "function" },
      "location": { "file": "sql/pkg_erp.sql", "line": 45 }
    },
    {
      "id": "t_erp_orders",
      "type": "table",
      "key": { "schema": "erp", "name": "t_erp_orders" }
    },
    {
      "id": "esb_order_sync",
      "type": "esb_service",
      "key": { "system": "wms", "service_name": "syncOrder", "version": "v1" },
      "properties": { "protocol": "HTTP", "endpoint": "https://wms.internal/api/sync" }
    }
  ],
  "edges": [
    {
      "source": "pkg_erp.create_order",
      "target": "pkg_erp.check_inventory",
      "type": "direct",
      "location": { "file": "sql/pkg_erp.sql", "line": 15 }
    },
    {
      "source": "pkg_erp.create_order",
      "target": "t_erp_orders",
      "type": "table_access",
      "location": { "file": "sql/pkg_erp.sql", "line": 12 },
      "properties": { "modes": ["write"], "write_kinds": ["insert"] }
    },
    {
      "source": "pkg_erp.create_order",
      "target": "esb_order_sync",
      "type": "calls_esb"
    }
  ]
}
```

**第二步**：导入

```bash
codeweb import \
  --file erp-graph.json \
  --output erp-store.bincode \
  --prefix /enterprise/erp \
  --name erp-system
```

**第三步**：分析自有代码

```bash
codeweb analyze --project my-java-project
# 生成 my-java-project.bincode
```

**第四步**：合并

```bash
codeweb merge \
  --output full-graph.bincode \
  my-java-project.bincode erp-store.bincode
```

**第五步**：追溯链路

```bash
# 追溯 Java 方法 → ERP 存储过程的完整链路
codeweb trace --from "com.example.OrderService.placeOrder" --direction forward --store full-graph.bincode
```

输出可能为：
```
OrderService.placeOrder (java_method)
  → direct → pkg_erp.create_order (procedure)
    → direct → pkg_erp.check_inventory (function)
    → table_access → t_erp_orders (table) [write: insert]
    → calls_esb → esb_order_sync (esb_service)
```

---

## 7. 验证与校验

### 7.1 导入时校验

`codeweb import` 会自动执行以下校验：

| 校验项 | 级别 | 说明 |
|--------|------|------|
| `format_version` 检查 | Error | 版本不是 `1` 则拒绝 |
| 节点 ID 唯一性 | Error | 同一文档中节点 ID 重复则拒绝 |
| 边引用完整性 | Error | 边的 source/target 必须引用已声明的节点 ID |
| 自定义类型声明 | Error | 自定义节点/边类型必须在 `node_schemas`/`edge_schemas` 中声明 |
| 空文档 | Warning | 节点和边都为空时发出警告但不阻止导入 |

### 7.2 JSON Schema 验证

在生成 CGEF 文件时，可以使用 `docs/cgef-schema.json` 进行预校验：

```bash
# 使用 jsonschema 校验工具（Python 示例）
pip install jsonschema
python -c "
import json, jsonschema
with open('docs/cgef-schema.json') as f: schema = json.load(f)
with open('my-graph.json') as f: doc = json.load(f)
jsonschema.validate(doc, schema)
print('Valid!')
"
```

---

## 8. 路径映射

CGEF 中的 `location.file` 使用相对路径。导入时通过 `--prefix` 参数将其映射到本地绝对路径。

**映射规则**：
- 有 `--prefix`：`prefix + relative_path`
- 无 `--prefix`：保持相对路径不变
- 自动去除 `./` 前缀和多余的分隔符

| CGEF 中的路径 | --prefix | 映射后 |
|--------------|----------|--------|
| `sql/pkg_order.sql` | `/enterprise/module-a` | `/enterprise/module-a/sql/pkg_order.sql` |
| `./sql/pkg.sql` | `/root` | `/root/sql/pkg.sql` |
| `sql/pkg.sql` | （无） | `sql/pkg.sql` |

---

## 9. 序列化格式

GraphStore 支持两种序列化格式：

| 格式 | 扩展名 | 特点 |
|------|--------|------|
| Bincode | `.bincode` | 二进制，紧凑，快速（推荐） |
| JSON | `.json` | 人类可读，便于调试 |

`--output` 参数的扩展名决定输出格式。

---

## 10. 注意事项

1. **版本兼容**：`format_version` 严格检查。codeweb 仅支持版本 `1`，更高版本的 CGEF 文件会被拒绝。
2. **ID 全局唯一**：节点 `id` 在同一 CGEF 文档内必须唯一。建议使用 `{package}.{name}` 或 `{fqn}` 等命名规则。
3. **自定义类型声明**：使用自定义节点/边类型时，必须在 `node_schemas`/`edge_schemas` 中声明，否则导入会报错。
4. **标准类型不可重新声明**：`procedure`、`table` 等标准类型不能出现在 `node_schemas` 中。
5. **合并幂等性**：对相同 GraphStore 多次 merge 同一文件，结果一致（按 NodeKey/边三元组去重）。
6. **大规模图谱**：CGEF 解析器支持 10 万级节点规模。更大规模建议分批导入后合并。
7. **文件安全**：CGEF 文件不包含加密/认证机制，文件安全性由企业自行保障。

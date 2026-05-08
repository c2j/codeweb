# OP 血缘数据转 CGEF 指南

将企业 Excel 血缘关系数据转换为 CGEF JSON 格式，与 codeweb 分析的存储过程调用图合并，实现从菜单到存储过程到表的端到端链路追溯。

---

## 1. 输入数据结构

Excel 包含 4 个 sheet，每个 sheet 描述一类关系：

| Sheet 名称 | 核心链路 | 关键列 |
|---|---|---|
| 菜单OP分析结果 | 菜单 → OP → JSP + 存储过程 | FNC_GET_MENU_PATH, RESOURCE_VALUE, OP_NAME, JSP_PATH, PKGPROC_NAME |
| com.icbc.aas OP分析结果 | Java OpStep 类 → 存储过程 | OP_NAME, PKGPROC_NAME, MYBATIS_ID, MYBATIS_FILE_PATH |
| com.icbc.ctp OP分析结果 | CTP .op 配置 → 存储过程 | OP_FILE_NAME, TYPE_VALUE, SRC_VALUE, PKGPROC_NAME |
| op菜单配置清单表 | 菜单 → OP + Servlet URL | FNC_GET_MENU_PATH, RESOURCE_VALUE, OP_NAME, COMM |

4 个 sheet 之间有数据重叠（如菜单OP分析结果与op菜单配置清单都包含菜单→OP关系），转换时**全部处理**，合并时按去重规则自动幂等。

---

## 2. 桥接原理

```
Excel 数据                        codeweb 分析数据
──────────                        ───────────────
menu ──→ op_handler ──→ procedure ──→ procedure ──→ table
                                   ↑
                              NodeKey 去重合并
```

桥接点是 `procedure` 节点。Excel 中的 `PKGPROC_NAME`（如 `PKG_SQS_CASH_MANAGE.PROC_UPDATE_TRAN_STATUS`）对应 codeweb 的标准 `procedure` 类型。

合并时 codeweb 对 procedure 节点采用三阶段匹配：
- **精确匹配**：`{schema, package, name}` 完全一致（大小写自动归一化）
- **正向 relaxed**：有 schema 的一方降级为无 schema 匹配
- **反向 relaxed**：无 schema 的一方匹配已有的 schema-qualified 节点
- 找不到匹配的 procedure 时，按 `unresolved` 类型处理（见第 6 节）

---

## 3. CGEF 整体结构

```json
{
  "format_version": 1,
  "metadata": {
    "source": "aas-lineage-excel",
    "generated_at": "2026-05-07T10:00:00Z",
    "description": "AAS系统血缘关系数据"
  },
  "node_schemas": { ... },
  "edge_schemas": { ... },
  "nodes": [ ... ],
  "edges": [ ... ]
}
```

### 3.1 node_schemas — 自定义节点类型声明

```json
{
  "node_schemas": {
    "menu": {
      "display_name": "菜单",
      "key_fields": ["resource_value"],
      "properties": {
        "menu_path":           { "type": "string",  "description": "菜单面包屑路径（如 资金管理→上清所业务管理→…）" },
        "resource_value_name": { "type": "string",  "description": "菜单显示名称" },
        "parent_value":        { "type": "string",  "description": "父菜单ID（用于构建菜单层级）" },
        "resource_id":         { "type": "string",  "description": "资源标识（如 R1）" }
      }
    },
    "op_handler": {
      "display_name": "OP处理器",
      "key_fields": ["op_name"],
      "properties": {
        "servlet_url": { "type": "string", "description": "Servlet调用地址（COMM列）" }
      }
    },
    "jsp": {
      "display_name": "JSP页面",
      "key_fields": ["path"],
      "properties": {
        "jsp_name": { "type": "string", "description": "JSP文件名" }
      }
    },
    "ctp_op_config": {
      "display_name": "CTP OP配置",
      "key_fields": ["op_file_name"],
      "properties": {
        "step_type": { "type": "string", "description": "步骤类型（ProcedureAccessOpStep 等）" },
        "src_value": { "type": "string", "description": "OP服务引用" }
      }
    },
    "java_op_step": {
      "display_name": "Java OpStep",
      "key_fields": ["class_name"],
      "properties": {}
    }
  }
}
```

### 3.2 edge_schemas — 自定义边类型声明

```json
{
  "edge_schemas": {
    "menu_triggers_op": {
      "display_name": "菜单触发操作",
      "source_types": ["menu"],
      "target_types": ["op_handler"]
    },
    "menu_parent": {
      "display_name": "菜单层级",
      "source_types": ["menu"],
      "target_types": ["menu"]
    },
    "op_calls_procedure": {
      "display_name": "OP调用存储过程",
      "source_types": ["op_handler"],
      "target_types": ["procedure"]
    },
    "op_renders_jsp": {
      "display_name": "OP渲染JSP",
      "source_types": ["op_handler"],
      "target_types": ["jsp"]
    },
    "ctp_step_calls_procedure": {
      "display_name": "CTP步骤调用存储过程",
      "source_types": ["ctp_op_config"],
      "target_types": ["procedure", "unresolved"]
    },
    "java_step_calls_procedure": {
      "display_name": "Java类调用存储过程",
      "source_types": ["java_op_step"],
      "target_types": ["procedure", "unresolved"]
    }
  }
}
```

---

## 4. PKGPROC_NAME 处理规则

### 4.1 拆分规则

`PKGPROC_NAME` 列值格式为 `PACKAGE.PROCEDURE`（以第一个 `.` 分隔）：

| PKGPROC_NAME 值 | package | name |
|---|---|---|
| `PKG_SQS_CASH_MANAGE.PROC_UPDATE_TRAN_STATUS` | `pkg_sqs_cash_manage` | `proc_update_tran_status` |
| `PKG_IMPORT_EXCEL.PROC_IMPORT_EXCEL` | `pkg_import_excel` | `proc_import_excel` |
| `GETUSERNAME`（无 `.`） | _(不填)_ | `getusername` |

### 4.2 大小写

**无需特殊处理。**

codeweb 的 `NodeKey` 对 Procedure/Function 的 `schema`/`package`/`name` 字段自动归一化为小写（与 Table/View 行为一致）。因此 CGEF 中的大小写不影响合并匹配。

```json
// 以下两种写法在合并时等价（自动归一化）
{ "type": "procedure", "key": { "package": "PKG_SQS_CASH_MANAGE", "name": "PROC_UPDATE_TRAN_STATUS" } }
{ "type": "procedure", "key": { "package": "pkg_sqs_cash_manage", "name": "proc_update_tran_status" } }
```

### 4.3 Schema 差异自动匹配

合并时采用**三阶段匹配**策略：

| 阶段 | 匹配规则 | 示例 |
|---|---|---|
| 精确匹配 | `{schema, package, name}` 完全一致（归一化后） | `proc:bigfund.pkg_foo.sp` = `proc:bigfund.pkg_foo.sp` |
| 正向 relaxed | 有 schema 的一方可降级为无 schema 匹配 | `proc:bigfund.pkg_foo.sp` → `proc:pkg_foo.sp` |
| 反向 relaxed | 无 schema 的一方匹配已有的 schema-qualified 节点 | `proc:pkg_foo.sp` 匹配 `proc:bigfund.pkg_foo.sp` |

这意味着以下场景**自动合并**：

```
Graph A (codeweb analyze):  proc:BIGFUND.PKG_IMPORT_EXCEL.PROC_IMPORT_EXCEL
Graph B (CGEF import):      proc:pkg_import_excel.proc_import_excel  (无 schema)
                                                     ↓ 合并后
                            proc:bigfund.pkg_import_excel.proc_import_excel  ← 保留有 schema 的版本
```

因此 CGEF 中可以安全地省略 `schema` 字段——只要 `package` + `name` 能匹配上 codeweb 分析的存储过程，就能自动关联。

---

## 5. 逐 Sheet 转换规则

### 5.1 Sheet 1：菜单OP分析结果

每行数据：

```
FNC_GET_MENU_PATH | RESOURCE_VALUE | RESOURCE_VALUE_NAME | PARENT_VALUE | OP_NAME | JSP_NAME | JSP_PATH | PKGPROC_NAME | COMM
```

**生成内容**：

| 实体 | 类型 | id 命名规则 | key |
|---|---|---|---|
| 菜单 | `menu`（自定义） | `menu_{RESOURCE_VALUE}` | `{"resource_value": RESOURCE_VALUE}` |
| OP 处理器 | `op_handler`（自定义） | `op_{OP_NAME}` | `{"op_name": OP_NAME}` |
| JSP 页面 | `jsp`（自定义） | `jsp_{JSP_PATH 的 hash 或去重 ID}` | `{"path": JSP_PATH}` |
| 存储过程 | `procedure`（标准）或 `unresolved` | `sp_{PKGPROC_NAME 转合法 ID}` | 见第 4 节拆分规则 |

**生成边**：

| 边类型 | source | target | 条件 |
|---|---|---|---|
| `menu_triggers_op` | menu 节点 | op_handler 节点 | 每行 1 条 |
| `op_renders_jsp` | op_handler 节点 | jsp 节点 | 每行 1 条，相同 OP → 相同 JSP 自动去重 |
| `op_calls_procedure` | op_handler 节点 | procedure / unresolved 节点 | 每行 1 条 |
| `menu_parent` | menu 节点 | 父 menu 节点 | PARENT_VALUE 非空且父节点存在时 |

**转换伪代码**：

```python
for row in sheet1_rows:
    menu_id = f"menu_{row['RESOURCE_VALUE']}"
    op_id = f"op_{row['OP_NAME']}"
    jsp_id = f"jsp_{hash_or_slug(row['JSP_PATH'])}"
    sp_id = f"sp_{slugify(row['PKGPROC_NAME'])}"

    # 节点（去重：相同 id 只创建一次）
    nodes.add(menu_id, type="menu",
        key={"resource_value": row['RESOURCE_VALUE']},
        properties={
            "menu_path": row['FNC_GET_MENU_PATH'],
            "resource_value_name": row['RESOURCE_VALUE_NAME'],
            "parent_value": row['PARENT_VALUE'],
            "resource_id": row['RESOURCE_ID']
        }
    )
    nodes.add(op_id, type="op_handler",
        key={"op_name": row['OP_NAME']},
        properties={"servlet_url": row['COMM']}
    )
    nodes.add(jsp_id, type="jsp",
        key={"path": row['JSP_PATH']},
        properties={"jsp_name": row['JSP_NAME']}
    )

    # 存储过程节点（按 PKGPROC_NAME 拆分）
    pkg_name = parse_pkgproc_name(row['PKGPROC_NAME'])  # 见第 4 节
    if is_known_procedure(pkg_name):  # 可选：与 codeweb 导出数据比对
        nodes.add(sp_id, type="procedure",
            key={"package": pkg_name.package, "name": pkg_name.name},  # 小写
            location={"file": "lineage/sheet1-menu-op", "line": 0}
        )
    else:
        nodes.add(sp_id, type="unresolved",
            key={"raw_expr": row['PKGPROC_NAME'], "context": row['OP_NAME']}
        )

    # 边
    edges.add(source=menu_id, target=op_id, type="menu_triggers_op")
    edges.add(source=op_id, target=jsp_id, type="op_renders_jsp")
    edges.add(source=op_id, target=sp_id, type="op_calls_procedure")

    # 菜单层级（如果父菜单存在）
    if row['PARENT_VALUE'] and has_menu_node(row['PARENT_VALUE']):
        edges.add(source=menu_id, target=f"menu_{row['PARENT_VALUE']}", type="menu_parent")
```

### 5.2 Sheet 2：com.icbc.aas OP分析结果

每行数据：

```
OP_NAME | PKGPROC_NAME | MYBATIS_ID | MYBATIS_FILE_PATH
```

`OP_NAME` 列为 Java 类文件名（如 `ShchSH45902701OpStep.java`），是直接调用存储过程的 Java 类。

**生成内容**：

| 实体 | 类型 | key |
|---|---|---|
| Java OpStep | `java_op_step`（自定义） | `{"class_name": OP_NAME}` |
| 存储过程 | `procedure`（标准）或 `unresolved` | 同第 4 节拆分规则 |
| MyBatis 语句（如有） | `mapped_statement`（标准） | `{"namespace": ..., "statement_id": ..., "kind": "select"}` |

**生成边**：

| 边类型 | source | target | 条件 |
|---|---|---|---|
| `java_step_calls_procedure` | java_op_step | procedure / unresolved | 每行 1 条 |

**转换伪代码**：

```python
for row in sheet2_rows:
    java_id = f"java_{slugify(row['OP_NAME'])}"
    sp_id = f"sp_{slugify(row['PKGPROC_NAME'])}"

    nodes.add(java_id, type="java_op_step",
        key={"class_name": row['OP_NAME']}
    )

    # 存储过程节点（同 Sheet 1 规则）
    pkg_name = parse_pkgproc_name(row['PKGPROC_NAME'])
    nodes.add(sp_id, type="procedure" or "unresolved", ...)

    edges.add(source=java_id, target=sp_id, type="java_step_calls_procedure")

    # 如果有 MyBatis 映射（MYBATIS_ID 非 "—"）
    if row['MYBATIS_ID'] and row['MYBATIS_ID'] != '—':
        ms_id = f"ms_{slugify(row['MYBATIS_ID'])}"
        nodes.add(ms_id, type="mapped_statement",
            key={
                "namespace": extract_namespace(row['MYBATIS_ID']),
                "statement_id": extract_statement_id(row['MYBATIS_ID']),
                "kind": "select"
            },
            location={"file": row['MYBATIS_FILE_PATH'] or "lineage/sheet2-java-op", "line": 0}
        )
        # Java 类调用 MyBatis 语句
        edges.add(source=java_id, target=ms_id, type="invokes_mapper")
        # MyBatis 语句调用存储过程
        edges.add(source=ms_id, target=sp_id, type="calls_procedure")
```

### 5.3 Sheet 3：com.icbc.ctp OP分析结果

每行数据：

```
OP_FILE_NAME | TYPE_VALUE | SRC_VALUE | PKGPROC_NAME
```

OP_FILE_NAME 是 CTP 框架的 .op 配置文件名（如 `aassqsacntcapitaltranallqueryop.op`），TYPE_VALUE 通常是 `ProcedureAccessOpStep`。

**生成内容**：

| 实体 | 类型 | key |
|---|---|---|
| CTP OP 配置 | `ctp_op_config`（自定义） | `{"op_file_name": OP_FILE_NAME}` |
| 存储过程 | `procedure`（标准）或 `unresolved` | 同第 4 节拆分规则 |

**生成边**：

| 边类型 | source | target | 条件 |
|---|---|---|---|
| `ctp_step_calls_procedure` | ctp_op_config | procedure / unresolved | 每行 1 条 |

**转换伪代码**：

```python
for row in sheet3_rows:
    ctp_id = f"ctp_{slugify(row['OP_FILE_NAME'])}"
    sp_id = f"sp_{slugify(row['PKGPROC_NAME'])}"

    nodes.add(ctp_id, type="ctp_op_config",
        key={"op_file_name": row['OP_FILE_NAME']},
        properties={
            "step_type": row['TYPE_VALUE'],
            "src_value": row['SRC_VALUE']
        }
    )

    pkg_name = parse_pkgproc_name(row['PKGPROC_NAME'])
    nodes.add(sp_id, type="procedure" or "unresolved", ...)

    edges.add(source=ctp_id, target=sp_id, type="ctp_step_calls_procedure")
```

### 5.4 Sheet 4：op菜单配置清单表

每行数据：

```
FNC_GET_MENU_PATH | RESOURCE_VALUE | RESOURCE_VALUE_NAME | PARENT_VALUE | OP_NAME | COMM
```

与 Sheet 1 结构类似但**没有** JSP 和 PKGPROC_NAME 列，只有 Servlet URL（COMM 列）。

**生成内容**：

| 实体 | 类型 | key |
|---|---|---|
| 菜单 | `menu`（自定义） | `{"resource_value": RESOURCE_VALUE}` |
| OP 处理器 | `op_handler`（自定义） | `{"op_name": OP_NAME}` |

**生成边**：

| 边类型 | source | target | 条件 |
|---|---|---|---|
| `menu_triggers_op` | menu | op_handler | 每行 1 条 |
| `menu_parent` | menu | 父 menu | PARENT_VALUE 非空且父节点存在 |

**转换伪代码**：

```python
for row in sheet4_rows:
    menu_id = f"menu_{row['RESOURCE_VALUE']}"
    op_id = f"op_{row['OP_NAME']}"

    nodes.add(menu_id, type="menu",
        key={"resource_value": row['RESOURCE_VALUE']},
        properties={
            "menu_path": row['FNC_GET_MENU_PATH'],
            "resource_value_name": row['RESOURCE_VALUE_NAME'],
            "parent_value": row['PARENT_VALUE']
        }
    )
    nodes.add(op_id, type="op_handler",
        key={"op_name": row['OP_NAME']},
        properties={"servlet_url": row['COMM']}
    )

    edges.add(source=menu_id, target=op_id, type="menu_triggers_op")

    if row['PARENT_VALUE'] and has_menu_node(row['PARENT_VALUE']):
        edges.add(source=menu_id, target=f"menu_{row['PARENT_VALUE']}", type="menu_parent")
```

### 5.5 跨 Sheet 去重

4 个 sheet 转换后合并到同一个 CGEF 文档。去重规则：

- **节点**：按 `id` 去重（相同 id 只保留第一个，或取字段更全的那个）
- **边**：按 `(source, target, type)` 三元组去重（相同三元组只保留一条）
- CGEF import 也会校验节点 id 唯一性

**建议**：先处理 Sheet 1（数据最全），再依次处理 Sheet 2/3/4，遇到已存在的节点 id 跳过节点创建但仍创建边。

---

## 6. 存储过程匹配与 Unresolved 处理

### 6.1 判断是否为已知存储过程

将 PKGPROC_NAME 拆分为 `(package, name)` 后：

1. **有匹配**：生成 `procedure` 类型节点，key 中的 package/name 使用**小写**
2. **无匹配**（PKGPROC_NAME 不包含 `.`，或已知不在 codeweb 分析范围内）：生成 `unresolved` 类型节点

### 6.2 unresolved 节点格式

```json
{
  "id": "unres_getusername",
  "type": "unresolved",
  "key": {
    "raw_expr": "GETUSERNAME",
    "context": "aasSecuritiesZhilianOp"
  }
}
```

| 字段 | 说明 |
|---|---|
| `raw_expr` | 原始调用目标名称（即 PKGPROC_NAME 值） |
| `context` | 调用来源（OP_NAME 或 OP_FILE_NAME） |

unresolved 节点**不需要 `location`**。合并后在 trace 结果中会显示为 `unresolved:GETUSERNAME (in aasSecuritiesZhilianOp)`。

### 6.3 常见需要按 unresolved 处理的情况

| PKGPROC_NAME 值 | 原因 | 处理 |
|---|---|---|
| `GETUSERNAME` | 内置函数，无 package 前缀 | unresolved: `raw_expr="GETUSERNAME"` |
| `PROC_XXX` | 只有过程名，无法确定包 | unresolved: `raw_expr="PROC_XXX"` |
| 空值或无效值 | 数据质量问题 | 跳过该行，不生成节点和边 |

---

## 7. 完整示例

以 Excel 中 `aassqsacntcapitaltranallqueryop` 相关数据为例，展示完整的 CGEF JSON 输出：

```json
{
  "format_version": 1,
  "metadata": {
    "source": "aas-lineage-excel",
    "generated_at": "2026-05-07T10:00:00Z",
    "description": "AAS系统血缘关系 — 示例数据"
  },
  "node_schemas": {
    "menu": {
      "display_name": "菜单",
      "key_fields": ["resource_value"],
      "properties": {
        "menu_path":           { "type": "string" },
        "resource_value_name": { "type": "string" },
        "parent_value":        { "type": "string" },
        "resource_id":         { "type": "string" }
      }
    },
    "op_handler": {
      "display_name": "OP处理器",
      "key_fields": ["op_name"],
      "properties": {
        "servlet_url": { "type": "string" }
      }
    },
    "jsp": {
      "display_name": "JSP页面",
      "key_fields": ["path"],
      "properties": {
        "jsp_name": { "type": "string" }
      }
    },
    "ctp_op_config": {
      "display_name": "CTP OP配置",
      "key_fields": ["op_file_name"],
      "properties": {
        "step_type": { "type": "string" },
        "src_value": { "type": "string" }
      }
    },
    "java_op_step": {
      "display_name": "Java OpStep",
      "key_fields": ["class_name"],
      "properties": {}
    }
  },
  "edge_schemas": {
    "menu_triggers_op":          { "display_name": "菜单触发操作" },
    "menu_parent":               { "display_name": "菜单层级" },
    "op_calls_procedure":        { "display_name": "OP调用存储过程" },
    "op_renders_jsp":            { "display_name": "OP渲染JSP" },
    "ctp_step_calls_procedure":  { "display_name": "CTP步骤调用存储过程" },
    "java_step_calls_procedure": { "display_name": "Java类调用存储过程" }
  },
  "nodes": [
    {
      "id": "menu_0406060030",
      "type": "menu",
      "key": { "resource_value": "0406060030" },
      "properties": {
        "menu_path": "资金管理→上清所业务管理→账户资金调拨总查询",
        "resource_value_name": "账户资金调拨总查询",
        "parent_value": "0406060000",
        "resource_id": "R1"
      }
    },
    {
      "id": "menu_0406061020",
      "type": "menu",
      "key": { "resource_value": "0406061020" },
      "properties": {
        "menu_path": "资金管理→中债业务管理→账户资金调拨总查询",
        "resource_value_name": "账户资金调拨总查询",
        "parent_value": "0406061000",
        "resource_id": "R1"
      }
    },
    {
      "id": "menu_0406069600",
      "type": "menu",
      "key": { "resource_value": "0406069600" },
      "properties": {
        "menu_path": "资金管理→全行上清所业务管理",
        "resource_value_name": "全行上清所业务管理",
        "parent_value": "M800",
        "resource_id": "R1"
      }
    },
    {
      "id": "op_aassqsacntcapitaltranallqueryop",
      "type": "op_handler",
      "key": { "op_name": "aassqsacntcapitaltranallqueryop" },
      "properties": {
        "servlet_url": "servlet/com.icbc.cte.cs.servlet.CSReqServlet?operationName=..."
      }
    },
    {
      "id": "jsp_aas_sqs_acnt_cash_all",
      "type": "jsp",
      "key": { "path": "aas/capitalmanagement/sqs/aas_sqs_acnt_cash_all.jsp" },
      "properties": { "jsp_name": "aas_sqs_acnt_cash_all.jsp" }
    },
    {
      "id": "sp_pkg_sqs_cash_manage_proc_update_tran_status_no",
      "type": "procedure",
      "key": { "package": "pkg_sqs_cash_manage", "name": "proc_update_tran_status_no" },
      "location": { "file": "lineage/sheet1-menu-op", "line": 0 }
    },
    {
      "id": "sp_pkg_sqs_cash_manage_proc_update_tran_status",
      "type": "procedure",
      "key": { "package": "pkg_sqs_cash_manage", "name": "proc_update_tran_status" },
      "location": { "file": "lineage/sheet1-menu-op", "line": 0 }
    },
    {
      "id": "sp_pkg_sqs_cash_manage_proc_get_all_acnt_info",
      "type": "procedure",
      "key": { "package": "pkg_sqs_cash_manage", "name": "proc_get_all_acnt_info" },
      "location": { "file": "lineage/sheet3-ctp-op", "line": 0 }
    },
    {
      "id": "sp_pkg_sqs_cash_manage_proc_get_all_acnt_info_all",
      "type": "procedure",
      "key": { "package": "pkg_sqs_cash_manage", "name": "proc_get_all_acnt_info_all" },
      "location": { "file": "lineage/sheet3-ctp-op", "line": 0 }
    },
    {
      "id": "sp_pkg_sqs_cash_manage_proc_set_sqs_acnt_state",
      "type": "procedure",
      "key": { "package": "pkg_sqs_cash_manage", "name": "proc_set_sqs_acnt_state" },
      "location": { "file": "lineage/sheet2-java-op", "line": 0 }
    },
    {
      "id": "unres_getusername",
      "type": "unresolved",
      "key": { "raw_expr": "GETUSERNAME", "context": "aassqsacntcapitaltranallqueryop" }
    },
    {
      "id": "ctp_aassqsacntcapitaltranallqueryop",
      "type": "ctp_op_config",
      "key": { "op_file_name": "aassqsacntcapitaltranallqueryop.op" },
      "properties": {
        "step_type": "ProcedureAccessOpStep",
        "src_value": "aassqsacntcapitaltranallqueryop.aassqsacntcapitaltranallquerysrv"
      }
    },
    {
      "id": "java_shchsh45902701opstep",
      "type": "java_op_step",
      "key": { "class_name": "ShchSH45902701OpStep.java" }
    }
  ],
  "edges": [
    { "source": "menu_0406060030", "target": "op_aassqsacntcapitaltranallqueryop", "type": "menu_triggers_op" },
    { "source": "menu_0406061020", "target": "op_aassqsacntcapitaltranallqueryop", "type": "menu_triggers_op" },
    { "source": "menu_0406069600", "target": "op_aassqsacntcapitaltranallqueryop", "type": "menu_triggers_op" },
    { "source": "op_aassqsacntcapitaltranallqueryop", "target": "jsp_aas_sqs_acnt_cash_all", "type": "op_renders_jsp" },
    { "source": "op_aassqsacntcapitaltranallqueryop", "target": "sp_pkg_sqs_cash_manage_proc_update_tran_status_no", "type": "op_calls_procedure" },
    { "source": "op_aassqsacntcapitaltranallqueryop", "target": "sp_pkg_sqs_cash_manage_proc_update_tran_status", "type": "op_calls_procedure" },
    { "source": "op_aassqsacntcapitaltranallqueryop", "target": "unres_getusername", "type": "op_calls_procedure" },
    { "source": "ctp_aassqsacntcapitaltranallqueryop", "target": "sp_pkg_sqs_cash_manage_proc_get_all_acnt_info", "type": "ctp_step_calls_procedure" },
    { "source": "ctp_aassqsacntcapitaltranallqueryop", "target": "sp_pkg_sqs_cash_manage_proc_get_all_acnt_info_all", "type": "ctp_step_calls_procedure" },
    { "source": "java_shchsh45902701opstep", "target": "sp_pkg_sqs_cash_manage_proc_set_sqs_acnt_state", "type": "java_step_calls_procedure" }
  ]
}
```

---

## 8. 导入与合并命令

```bash
# 第一步：将 Excel 转换后的 CGEF JSON 导入为 GraphStore
codeweb import \
  --file aas-lineage.cgef.json \
  --output aas-lineage.bincode \
  --name aas-lineage

# 第二步：合并自有分析结果和 Excel 血缘数据
codeweb merge \
  --output full-graph.bincode \
  --name combined \
  my-sql-project.bincode \
  aas-lineage.bincode

# 第三步：追溯完整链路（菜单 → 存储过程 → 表）
codeweb trace --from "0406060030" --project .
```

合并后 trace 输出效果：

```
menu:0406060030 (资金管理→上清所业务管理→账户资金调拨总查询)
  → menu_triggers_op → op_handler:aassqsacntcapitaltranallqueryop
    → op_renders_jsp → jsp:aas_sqs_acnt_cash_all.jsp
    → op_calls_procedure → proc:pkg_sqs_cash_manage.proc_update_tran_status_no
      → direct → proc:pkg_sqs_cash_manage.proc_internal_helper    ← codeweb 从SQL分析
      → table_access → table:t_sqs_cash_transaction [write: insert] ← codeweb 从SQL分析
    → op_calls_procedure → proc:pkg_sqs_cash_manage.proc_update_tran_status
    → op_calls_procedure → unresolved:GETUSERNAME
```

---

## 9. 验证清单

生成 CGEF JSON 后，导入前检查：

| 检查项 | 方法 |
|---|---|
| JSON 格式合法 | `python -m json.tool aas-lineage.cgef.json > /dev/null` |
| `format_version` 为 1 | 检查顶层字段 |
| 节点 id 全局唯一 | `jq '.nodes[].id' aas-lineage.cgef.json \| sort \| uniq -d` 应无输出 |
| 边的 source/target 均引用已存在的节点 id | 脚本校验或依赖 `codeweb import` 的自动校验 |
| procedure 节点 key 中的 package/name 为小写 | `jq '.nodes[] | select(.type=="procedure") | .key' aas-lineage.cgef.json` |
| 无标准类型重复声明（`procedure` 不出现在 node_schemas 中） | 检查 node_schemas 的 key |
| CGEF Schema 校验 | `jsonschema aas-lineage.cgef.json docs/cgef-schema.json` |

---

## 10. 注意事项

1. **节点 id 命名建议**：使用 `{类型前缀}_{业务键值}` 格式（如 `menu_0406060030`），避免跨 sheet 冲突。
2. **Sheet 1 与 Sheet 4 重叠**：两者都有菜单→OP 关系。全部处理即可，`menu_triggers_op` 边按 `(source, target, type)` 三元组自动去重。
3. **空值处理**：PKGPROC_NAME 为空时跳过该行，不生成节点和边。MYBATIS_ID 为 `—` 时不创建 mapped_statement 节点。
4. **大规模数据**：CGEF 解析器支持 10 万级节点。单个 Excel 通常在千级行，不影响性能。
5. **增量更新**：重新生成 CGEF 后重新 import + merge 即可，merge 幂等。
6. **location 占位**：procedure 节点要求有 location，Excel 数据无源文件位置时使用 `"file": "lineage/{sheet来源}", "line": 0`。merge 后 codeweb 会优先保留有真实 location 的副本。
7. **大小写和 schema 自动处理**：merge 时 NodeKey 自动归一化为小写，且支持 schema 差异的模糊匹配（见第 4.3 节）。CGEF 中可以省略 schema 字段，也可以使用任意大小写。

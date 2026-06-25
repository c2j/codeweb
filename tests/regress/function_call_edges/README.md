# function_call_edges — 回归测试案例库

验证用户自定义函数调用关系在 SQL 表达式中被正确提取为图边（`DirectCall`）。

## 背景

v0.7.2 及之前版本中，`CallExtractor` 缺少 `visit_expr` 覆写，导致出现在表达式位置（SELECT 输出列、WHERE 条件、PL/pgSQL 赋值、IF 条件、子查询等）的函数调用不被采集，Function 节点无 Callers。

v0.7.3 修复此问题（PR #50）。

## 目录结构

```
function_call_edges/
├── README.md           ← 本文件
├── cases.toml          ← 案例配置（描述、期望边、SQL 路径）
└── cases/              ← SQL 测试固件
    ├── expr_assignment.sql
    ├── perform_call.sql
    ├── select_target_and_where.sql
    ├── schema_mismatch.sql
    ├── where_subquery.sql
    ├── builtin_not_captured.sql
    └── dbe_xmldom_builtin.sql
```

## 案例一览

| ID | 场景 | 期望 |
|----|------|------|
| `expr_assignment` | PL/pgSQL 赋值 `v := calc_total(1)` | `process_order → calc_total` |
| `perform_call` | `PERFORM bar()` | `foo → bar` |
| `select_target_and_where` | SELECT 输出列 + WHERE 条件 | `report_users → format_name`、`report_users → get_priority` |
| `schema_mismatch` | 定义有 schema（`biz.calc_total`），调用无 schema | `process_order → calc_total`（两阶段解析） |
| `where_subquery` | WHERE 右侧子查询内 | `find_high_value_orders → get_threshold` |
| `builtin_not_captured` | 内置函数 `COUNT(*)` | 无 call edge（`builtin` 过滤） |
| `dbe_xmldom_builtin` | GaussDB 系统包 `dbe_xmldom.*`（语句级调用） | 无 Unresolved 节点（系统内置识别） |

## 添加新案例

1. 在 `cases/` 下创建 `.sql` 文件
2. 在 `cases.toml` 中添加 `[case.<id>]` 段：

```toml
[case.my_new_case]
description = "一句话描述验证场景"
sql = "cases/my_new_case.sql"
fixed_in = "vX.Y.Z"

[[case.my_new_case.expected_edges]]
from = "caller_name"
to = "callee_name"
type = "direct"
```

3. 负向案例（验证不产生边）使用 `expected_no_call_edges = true`；
   验证不产生 Unresolved 节点使用 `expected_no_unresolved = true`

## 运行测试

```sh
cargo test --test regress_function_call_edges
```

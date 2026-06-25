# local_var_not_call_edges — 回归测试案例库

验证 PL/SQL 局部标识符（集合变量、标量、RECORD 字段、FOR 循环计数器、参数等）
以 `name(args)` 语法出现时，不被误识别为过程/函数调用边（`DirectCall`）。

## 背景

v0.7.6 及之前版本中，`CallExtractor` 缺少局部符号表：未实现 `visit_pl_declaration`，
导致 DECLARE 块中声明的集合变量（`TYPE t IS TABLE OF ...; v t;`）在使用下标语法
`v(i)` 时，因为 ogsql-parser 将其解析为 `Expr::FunctionCall { builtin: None }`，
被 `visit_expr` 当作真实调用边采集。被调用方 `v_date`/`v_fund` 等无法解析时，就产生
了 `Unresolved` 节点。

本套件与 `function_call_edges`（验证真实调用边被采集）互补：这里只关心 **假阳性过滤**。

## 目录结构

```
local_var_not_call_edges/
├── README.md           ← 本文件
├── cases.toml          ← 案例配置（描述、期望边、SQL 路径）
├── codeweb.toml        ← 测试项目配置
└── cases/              ← SQL 测试固件
    ├── collection_index_not_captured.sql
    └── collection_index_with_real_call.sql
```

## 案例一览

| ID | 场景 | 期望 |
|----|------|------|
| `collection_index_not_captured` | 局部集合变量下标访问 `v_date(i)` / `v_fund(i)`（用户原始案例） | 无 call edge |
| `collection_index_with_real_call` | 同一过程体内既有真实调用（RHS）又有集合下标（WHERE） | `clean_proc → compute_score`；不产生 `clean_proc → v_scores` |

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

3. 负向案例（验证不产生边）使用 `expected_no_call_edges = true`

## 运行测试

```sh
cargo test --test regress_local_var_not_call_edges
```

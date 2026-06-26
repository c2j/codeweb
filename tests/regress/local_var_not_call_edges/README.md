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
    ├── collection_index_with_real_call.sql
    ├── param_shadows_procedure.sql
    ├── scope_reset_across_procedures.sql
    ├── type_constructor_not_captured.sql
    ├── pkg_body_scope_leak.sql
    ├── pkg_body_param_not_captured.sql
    ├── nested_routine_scope_leak.sql
    ├── plsql_varray_type_constructor.sql
    ├── plsql_table_of_type_constructor.sql
    └── plsql_index_by_pkg_variable.sql
```

## 案例一览

| ID | 场景 | 期望 |
|----|------|------|
| `collection_index_not_captured` | 局部集合变量下标访问 `v_date(i)` / `v_fund(i)`（用户原始案例） | 无 call edge |
| `collection_index_with_real_call` | 同一过程体内既有真实调用（RHS）又有集合下标（WHERE） | `clean_proc → compute_score`；不产生 `clean_proc → v_scores` |
| `param_shadows_procedure` | 过程参数以括号语法出现在 WHERE 子句 | `batch_check → real_target`；参数 `p_ids` 不产生 Unresolved 节点 |
| `scope_reset_across_procedures` | proc_a 局部变量 `v_date` 与全局函数同名；proc_b 调用该函数 | `proc_b → v_date` 存在；`proc_a → v_date` 不存在（局部变量被过滤） |
| `type_constructor_not_captured` | TYPE 构造函数 `account_record_table()`、成员方法 `obj_account_record.equals(...)`、集合下标 `aaa1(i)` | 无 call edge（TYPE 名 + 局部变量双重过滤） |
| `pkg_body_scope_leak` | 包体内 proc_a 局部变量 `helper_fn` 泄漏到 proc_b，抑制真实调用 | `proc_b → helper_fn` 存在（待修复：当前缺失） |
| `pkg_body_param_not_captured` | 包体过程参数 `p_ids` 以 `(i)` 语法出现在 WHERE 子句 | 无 call edge（待修复：当前产生假阳性 Unresolved） |
| `nested_routine_scope_leak` | 嵌套过程的局部变量 `v_shadow` 泄漏到外层作用域 | `outer_proc → v_shadow` 存在（待修复：当前缺失） |
| `plsql_varray_type_constructor` | DECLARE 块内 `TYPE arr_type IS VARRAY(4) OF VARCHAR2(100)` 后 `arr_type(...)` 构造 | 无 call edge（PL/SQL 块级 TYPE 名进入 `known_types`） |
| `plsql_table_of_type_constructor` | DECLARE 块内 `TYPE t_work_array IS TABLE OF c_work%ROWTYPE` 后 `t_work_array()` 构造 | 无 call edge |
| `plsql_index_by_pkg_variable` | 包级 `TYPE vchartab_pkg IS TABLE OF ... INDEX BY INTEGER` + 包级变量下标 `vchar_array_pkg(1)` | 无 call edge（包级变量进入 `local_vars`） |

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

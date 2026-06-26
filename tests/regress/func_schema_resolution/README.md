# func_schema_resolution — 回归测试案例库

验证函数/存储过程的 **schema 解析** 在歧义场景下能正确解析，不产生 Unresolved 节点。

## 背景

当多个 schema（或 schema + package）下存在同名函数/过程时，**不带 schema 限定**的调用
会产生 Unresolved 节点。根因是解析链路的两个阶段都缺乏歧义消解能力：

1. **`create_edges` 初始解析**（`builder.rs:1413-1478`）：无「裸名 → 有 schema 定义」兜底。
   不带 schema 的调用 `my_func()` 无法匹配 `schema=Some("s")` 的定义。
2. **`resolve_unresolved_nodes` 后处理 Strategy 4**（`builder.rs:2708-2714`）：裸名匹配要求
   `matches.len() == 1`；当多个 routine 同名时直接放弃，不尝试用 caller schema/package 消歧。

此外，初始解析的 caller-context 兜底（Strategy 5）只检查 caller 的 **package**，不检查
caller 的 **schema**，因此独立 schema 函数的歧义调用无法消解。

### 影响面

企业级数据库中，通用工具函数（`format_date`、`get_status`、`compute` 等）常在多个 schema
下重复定义。所有不带 schema 限定的调用都会变成 Unresolved，导致大量 func 节点缺失 caller 边。

## Bug 编号

| Bug | 描述 | 位置 |
|-----|------|------|
| #1 | `create_edges` 无「裸名→有schema定义」兜底 | `builder.rs:1413-1478` |
| #2 | Strategy 4 歧义时直接放弃，不用 caller schema 消歧 | `builder.rs:2708-2714` |
| #6 | `pkg_member_lower` schema-as-package 索引排除 package 成员 | `builder.rs:2264-2270` |

## 目录结构

```
func_schema_resolution/
├── README.md           ← 本文件
├── cases.toml          ← 案例配置
└── cases/              ← SQL 测试固件
    ├── ambiguous_bare_name.sql
    ├── ambiguous_standalone_and_pkg.sql
    ├── caller_schema_disambiguation.sql
    └── multi_schema_same_util.sql
```

## 案例一览

| ID | 场景 | Bug | 期望 |
|----|------|-----|------|
| `ambiguous_bare_name` | 两个 schema 同名 func，裸调 | #1+#2 | `caller_proc → util_func`；0 Unresolved |
| `ambiguous_standalone_and_pkg` | 独立 func + package 成员同名，裸调 | #1+#2+#6 | `do_work → helper`；0 Unresolved |
| `caller_schema_disambiguation` | Caller 在 s1，s1+s2 同名 func，裸调应用 s1 | #2 | `run_it → compute`；0 Unresolved |
| `multi_schema_same_util` | 三个 schema 同名工具函数（系统性） | #1+#2 | `batch_run → format_date`；0 Unresolved |

## 运行测试

```sh
cargo test --test regress_func_schema_resolution
```

> **注意**：当前这些测试 **预期失败**（复现 bug）。修复解析逻辑后测试将通过。

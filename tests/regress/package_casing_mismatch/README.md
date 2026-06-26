# package_casing_mismatch — 回归测试案例库

验证 Package 头（SPEC）和体（BODY）之间包名或子过程名 **大小写不一致** 时，
graph builder 能够正确合并为一个 Package 节点和一个 Routine 节点，不产生孤儿节点或重复节点。

## 背景

`GraphBuilder::create_package_nodes` 中构建 package 索引键时，**未做小写归一化**，
而表（Table）有 `normalize_table_key` 做小写归一化。这导致：

- Head 声明 `mypkg`，Body 实现 `MyPkg` → `package_index["mypkg"]` 和 `package_index["MyPkg"]` 是两个不同键
- 产生 **两个 Package 节点**（一个来自 head，一个来自 body）
- 子过程也因 `RoutineId.package` 字段大小写不同而分裂为多个 Procedure 节点
- Head 中声明的子过程因为 `found_in_body` 比对时 `==` 大小写敏感，被标记为 `partial: true` 的孤儿节点

## 目录结构

```
package_casing_mismatch/
├── README.md           ← 本文件
├── cases.toml          ← 案例配置（描述、期望、SQL 路径）
└── cases/              ← SQL 测试固件
    ├── pkg_name_casing_mismatch.sql
    ├── proc_name_casing_mismatch.sql
    ├── pkg_call_edge_casing.sql
    ├── procedure_casing_mismatch.sql
    ├── type_casing_mismatch.sql
    └── sequence_casing_mismatch.sql
```

## 案例一览

| ID | 场景 | 期望 |
|----|------|------|
| `pkg_name_casing_mismatch` | 包名大小写不同：head=`my_pkg`，body=`MY_PKG` | 1 个 Package 节点；子过程非 partial |
| `proc_name_casing_mismatch` | 子过程名大小写不同：head=`proc_a`，body=`Proc_A` | 1 个 Procedure 节点；非 partial |
| `pkg_call_edge_casing` | 混合大小写 + 调用边验证 | 调用边正确解析；无 Unresolved 节点 |
| `procedure_casing_mismatch` | 独立过程名大小写不同：`my_test_proc` vs `MY_TEST_PROC` | 1 个 Procedure 节点 |
| `type_casing_mismatch` | TYPE 名大小写不同：`my_test_type` vs `MY_TEST_TYPE` | 1 个 Type 节点 |
| `sequence_casing_mismatch` | SEQUENCE 名大小写不同：`my_test_seq` vs `MY_TEST_SEQ` | 1 个 Sequence 节点 |

## 添加新案例

1. 在 `cases/` 下创建 `.sql` 文件
2. 在 `cases.toml` 中添加 `[case.<id>]` 段
3. 在 `tests/regress_package_casing.rs` 中添加测试函数

## 运行测试

```sh
cargo test --test regress_package_casing
```

# issue_120_multi_schema_table_access

验证多 schema 同名表的 `TableAccess` 边按 owner schema 正确解析。

## 背景

当多个 schema 下存在同名表时，存储过程体内对表的引用（qualified `schema.tab` 或 bare `tab`）
应正确指向所属 schema 的表。原 `table_index` 使用平键 `HashMap<String, NodeIndex>`，
裸名别名通过 `or_insert` 注册导致先扫到的 schema 抢占裸名槽位，后续 schema 的裸名引用全部
错误指向第一张表。修复：`collect_table_access_from_statements` 增加 `owner_schema` 参数，
裸名查询时优先用 `(owner_schema, name)` 限定键查找。

## Bug 编号

| Bug | 描述 | 位置 |
|-----|------|------|
| #120 | `table_index` 裸名别名冲突 | `builder.rs:128, L2729-2732, L547-550, L579-582, L1199-1200, L1221-1222, L1251-1254` |

## 案例一览

| ID | 场景 | Bug | 期望 |
|----|------|-----|------|
| `multi_schema_same_table` | 两 schema 同名 tab1，proc_a qualified 引用，proc_b bare 引用 | #120 | 两条 TableAccess 边各指向自己 schema 的 tab1 |

## 运行测试

```sh
cargo test --test regress_issue_120_multi_schema_table
```

> Bug 已修复。裸名引用 `tab1` 现在按 owner schema 限定向查找：先尝试 `schema_b.tab1`，
> 命中则用限定键；未命中回退裸名。测试通过。

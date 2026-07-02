# Mark Regression Cases

Tests for the `codeweb mark` command, which marks rows in a WDR SQL detail CSV
based on their relation to a target graph node.

## Test Cases

| Case | Target | Description | Key Assertions |
|------|--------|-------------|----------------|
| case01 | `table:orders` | Simple table - direct name match + indirect caller match | 3 direct, 2 indirect, 1 no-match |
| case02 | `proc:proc_c` | Procedure caller-chain (3-level transitive) | 1 direct, 2 indirect (proc_a, proc_b), 2 no-match |
| case03 | `table:accounts` | Table deep caller-chain + fingerprint | 3 direct, 3 indirect (name), 1 no-match |
| case04 | `func:calc_tax` | Function caller-chain | 1 direct, 3 indirect, 1 no-match |
| case05 | `table:orders` | Case insensitivity | All 5 match with various casing |
| case06 | node not in graph | No match / empty result | All 3 no-match |
| case07 | `table:orders` | Mixed package + cross-schema | 1 direct, 4 indirect (pkg + cross-schema proc) |

## Graph Topologies

```
case01 (Simple Table):
  orders ←TableAccess-- update_orders
  orders ←TableAccess-- get_orders

case02 (Proc Chain):
  proc_a → proc_b → proc_c    (DirectCall edges, transitive)

case03 (Table Deep):
  accounts ←TableAccess-- audit_accounts ←DirectCall-- monthly_close ←DirectCall-- year_end
  accounts ←TableAccess-- verify_balance

case04 (Function):
  calc_total → calc_tax
  generate_invoice → calc_total
  batch_invoice → generate_invoice

case05 (Case):
  orders ←TableAccess-- update_orders   (same as case01, various case inputs)

case06 (No Match):
  products, list_products               (target node not in graph)

case07 (Package):
  orders ←TableAccess-- pkg_order.process_order
  pkg_order.process_order ←DirectCall-- pkg_report.generate_report
  orders ←TableAccess-- finance.calc_revenue (cross-schema)
```

## File Structure

```
tests/regress/mark/cases/
  case01_simple_table/
    schema.sql       # SQL to create the graph
    input.csv        # WDR CSV input (unique_sql_id, sql_text)
    expected.csv     # Expected annotated output (unique_sql_id, sql_text, codeweb_match, codeweb_matched_by)
  case02_proc_chain/
    ...
```

## Expected Output Columns

| Column | Values |
|--------|--------|
| `codeweb_match` | `"direct"` - SQL text directly references target node |
|                 | `"indirect"` - SQL text references a node in target's caller-chain |
|                 | `""` (empty) - no relation |
| `codeweb_matched_by` | NodeKey display string (e.g. `table:orders`, `proc:update_orders`) |

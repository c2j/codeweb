# `codeweb extract` — minimal runnable example (issue #193)

Dependency chain: `a1`/`a4` → `b1` → `tbl_c1`, with unrelated same-file
definitions (`a2`, `a3`, `b2`, `tbl_other`) that must be trimmed away.

## Run it

```sh
cp -r tests/fixtures/extract /tmp/issue193-demo && cd /tmp/issue193-demo
codeweb init issue193-demo --dir sql
codeweb analyze
codeweb extract --node a1 --output dependency_sources
```

Expected output (byte-for-byte):

```
dependency_sources/
└── sql/
    ├── PKG_A.sql   # only a1's definition
    ├── FNC_B.sql   # only b1's definition (terminator included)
    └── TBL_C1.sql  # only tbl_c1's DDL
```

Union of two roots sharing a dependency (b1 written once), via a nodes file:

```sh
codeweb extract --nodes-file tests/fixtures/extract/targets.txt \
    --output dependency_sources --overwrite
```

With the JSON report (`--report report.json`) unresolved calls, inferred
tables and built-in functions are listed explicitly instead of being dropped
silently. The original SQL files are never modified.

These exact files are exercised by `tests/extract_test.rs`.

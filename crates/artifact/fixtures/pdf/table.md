# Tables

A GFM table with alignment markers and inline formatting in cells.

| Option | Setup | Concurrency | Fits here |
|:-------|:-----:|------------:|-----------|
| SQLite | none | one writer | **yes** |
| Postgres | a server | many | later |
| DynamoDB | an account | many | no |
| `redis` | a server | many | *cache only* |

A second table without a header row separator style, two columns:

| Key | Value |
|---|---|
| version | 0.1.0 |
| pages | 1 |
| unicode | naïve café — ✓ |

# Images

An image by absolute path (the test substitutes `__FIXTURES__` with the fixtures directory):

![A small chart](__FIXTURES__/chart.png)

The same image by a path relative to the process working directory (the crate root under `cargo test`):

![Relative chart](fixtures/pdf/chart.png)

Text after the images keeps flowing.

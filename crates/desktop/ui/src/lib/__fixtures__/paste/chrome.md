## Getting started

Install with `npm install turndown`, then read the [options](https://github.com/mixmark-io/turndown#options). Set *emDelimiter* to taste.

- Fast
- Extensible with [rules](https://github.com/mixmark-io/turndown#addrulekey-rule)

1. Parse
2. Convert

```
var turndownService = new TurndownService()
var markdown = turndownService.turndown('<h1>Hello world!</h1>')
```

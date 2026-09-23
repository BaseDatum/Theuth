// Design 65 §3.4: the ELK layout thread. `elk-worker.min.js` installs ELK's own message protocol on `self` when it
// finds itself in a Worker; `graph-layout.ts` drives it through `elk-api`'s `workerFactory`. Nothing of ours runs
// here — the graph is translated on the main thread (cheap), only the layout is off it.
import 'elkjs/lib/elk-worker.min.js'

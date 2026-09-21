const fs = require("fs");

/** A queue. */
class Queue {
  constructor() {
    this.items = [];
  }

  /** Push one. */
  push(x) {
    this.items.push(x);
  }
}

function apply(f, x) {
  // `f` is a parameter: filtered as a local.
  return f(x) + helper(x);
}

const helper = (x) => x + 1;

exports.run = function run() {
  const q = new Queue();
  q.push(apply((v) => v * 2, 3));
  return fs.existsSync(".");
};

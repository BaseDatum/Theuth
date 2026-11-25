// 218: the events on the `onEvent` channel (mirrors `src/models.rs`, whose golden tests pin the JSON).

import Foundation

struct PencilEvent: Encodable {
  let kind = "pencil"
  let gesture: String
  let action: String
}

struct ErrorEvent: Encodable {
  let kind = "error"
  let message: String
}

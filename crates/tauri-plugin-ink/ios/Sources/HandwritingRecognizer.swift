// 218 §6 / R6: on-device handwriting for a mark's image — `VNRecognizeTextRequest`, `.accurate`, off the
// main thread. The text is page/ink-authored, never an instruction: the caller sanitises and quotes it.

import Foundation
import UIKit
import Vision

struct Recognized: Encodable {
  let text: String
  let confidence: Double
}

enum HandwritingRecognizer {
  private static let queue = DispatchQueue(label: "theuth.ink.handwriting", qos: .userInitiated)
  private static let maxChars = 200

  struct Failure: LocalizedError {
    let message: String
    var errorDescription: String? { message }
  }

  /// Recognise the text in a base64 PNG (no `data:` prefix); `completion` runs off the main thread.
  static func recognize(base64: String, completion: @escaping (Result<Recognized, Error>) -> Void) {
    queue.async {
      guard let data = Data(base64Encoded: base64, options: .ignoreUnknownCharacters),
        let image = UIImage(data: data)?.cgImage
      else {
        completion(.failure(Failure(message: "recognize_text: not a PNG")))
        return
      }
      let request = VNRecognizeTextRequest()
      request.recognitionLevel = .accurate
      request.usesLanguageCorrection = true
      request.automaticallyDetectsLanguage = true
      do {
        try VNImageRequestHandler(cgImage: image, options: [:]).perform([request])
      } catch {
        completion(.failure(error))
        return
      }
      let candidates = (request.results ?? []).compactMap { $0.topCandidates(1).first }
      // Reading order: top to bottom, then left to right (Vision's boxes have a bottom-left origin).
      let lines = (request.results ?? []).compactMap { observation -> (CGRect, String)? in
        guard let best = observation.topCandidates(1).first else { return nil }
        return (observation.boundingBox, best.string)
      }.sorted { a, b in
        abs(a.0.midY - b.0.midY) > min(a.0.height, b.0.height) / 2
          ? a.0.midY > b.0.midY : a.0.minX < b.0.minX
      }
      var text = lines.map { $0.1 }.joined(separator: " ")
        .trimmingCharacters(in: .whitespacesAndNewlines)
      if text.count > maxChars { text = String(text.prefix(maxChars)) }
      let confidence =
        candidates.isEmpty
        ? 0 : candidates.map { Double($0.confidence) }.reduce(0, +) / Double(candidates.count)
      completion(.success(Recognized(text: text, confidence: confidence)))
    }
  }
}

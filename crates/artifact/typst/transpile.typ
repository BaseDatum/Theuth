// Error-locating pass (pdf.rs `locate`): the Typst cmarker generates for a markdown body, exposed
// as metadata so Rust can compile it as a real file and get line numbers for a failing construct.
// cmarker itself `eval`s that text, which pins every error to its own `eval` call.
#import "cmarker/lib.typ": render
#set page(width: auto, height: auto, margin: 0pt)
#metadata(render(sys.inputs.body, show-source: true, smart-punctuation: true, math: none).text) <src>

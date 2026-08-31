//! `std.http`: parsing, rendering, and routing.

mod common;
use common::{check, run};

const HEADER: &str = r#"(module m
  (use std.http :as http)
  (use std.json :as json))
"#;

#[test]
fn a_request_parses_into_a_record() {
    let result = run(&format!(
        "{HEADER}
(pub fn main [] -> Unit !io
  (match (http.parse-request \"GET /greeting?name=Ada&loud=yes HTTP/1.1\\r\\nHost: example.test\\r\\nX-Trace: 7\\r\\n\\r\\n\")
    (Ok request)
      (do
        (println request.method)
        (println request.path)
        (println (get request.query \"name\" \"\"))
        (println (get request.query \"loud\" \"\"))
        (println (get request.headers \"host\" \"\"))
        (println (get request.headers \"x-trace\" \"\")))
    (Err error) (println \"error:\" (http.describe error))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, ":get\n/greeting\nAda\nyes\nexample.test\n7\n");
}

#[test]
fn a_body_is_kept_intact() {
    // The separator can appear inside a body, so the split must happen once.
    let result = run(&format!(
        "{HEADER}
(pub fn main [] -> Unit !io
  (match (http.parse-request \"POST /echo HTTP/1.1\\r\\ncontent-length: 4\\r\\n\\r\\nbody\")
    (Ok request) (println request.body)
    (Err error) (println \"error:\" (http.describe error))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "body\n");
}

#[test]
fn a_malformed_request_is_an_error_not_a_crash() {
    let result = run(&format!(
        "{HEADER}
(pub fn main [] -> Unit !io
  (match (http.parse-request \"nonsense\")
    (Ok request) (println request.path)
    (Err error) (println (http.describe error))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "the request has no header terminator\n");
}

#[test]
fn a_response_renders_with_a_content_length() {
    let result = run(&format!(
        "{HEADER}
(pub fn main [] -> Unit !io
  (println (http.render-response (http.text 200 \"hello\"))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let text = result.output;
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text:?}");
    assert!(text.contains("content-type: text/plain; charset=utf-8\r\n"), "{text:?}");
    assert!(text.contains("content-length: 5\r\n"), "{text:?}");
    assert!(text.ends_with("\r\n\r\nhello\n"), "{text:?}");
}

#[test]
fn content_length_counts_bytes_not_characters() {
    let result = run(&format!(
        "{HEADER}
(pub fn main [] -> Unit !io
  (println (contains? (http.render-response (http.text 200 \"héllo\")) \"content-length: 6\")))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "true\n");
}

#[test]
fn a_handler_routes_on_the_request_record() {
    let result = run(&format!(
        "{HEADER}
(fn handle [request: http.Request] -> http.Response
  (match request
    {{:method :get :path \"/health\"}} (http.text 200 \"ok\")
    {{:method :get :path \"/greeting\" :query {{\"name\" name}}}}
      (http.json 200 (json.encode {{message name}}))
    _ (http.not-found)))

(pub fn main [] -> Unit !io
  (println (handle (http.test-request :get \"/health\")).body)
  (println (handle (http.test-request :get \"/greeting?name=Ada\")).body)
  (println (handle (http.test-request :get \"/nope\")).status))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "ok\n{\"message\":\"Ada\"}\n404\n");
}

#[test]
fn a_handler_with_the_wrong_shape_is_a_type_error() {
    let codes = check(&format!(
        "{HEADER}
(fn handle [request: http.Request] -> Int 1)
(pub fn main [] -> Unit !io !async
  (match (http.serve \"127.0.0.1:0\" handle)
    (Ok _) nil
    (Err error) (println (http.describe error))))"
    ));
    assert_eq!(codes, vec!["type-mismatch"]);
}

// korben-ggd
#[test]
fn the_client_rejects_a_scheme_it_cannot_speak() {
    // `https` is spoken now, so this asks about one that is not. The scheme is
    // named back, which the old message did not do.
    let result = run(&format!(
        "{HEADER}
(pub fn main [] -> Unit !io
  (match (http.get-url \"gopher://example.test/\")
    (Ok response) (println response.status)
    (Err error) (println (http.describe error))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "only http:// and https:// are supported, not gopher://\n");
}

#[test]
fn sockets_are_resources_governed_by_ownership() {
    // A listener owns an operating-system handle, so it moves.
    let codes = check(
        "(module m (use std.net :as net))
         (fn take [listener: Listener] -> Unit !io (listener.close))
         (fn f [] -> Unit !io
           (match (net.listen \"127.0.0.1:0\")
             (Err _) nil
             (Ok listener) (do (take listener) (take listener))))",
    );
    assert_eq!(codes, vec!["use-after-move"]);
}

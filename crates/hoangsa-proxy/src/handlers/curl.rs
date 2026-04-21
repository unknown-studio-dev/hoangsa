//! Built-in filter for `curl`.
//!
//! API responses are usually pretty-printed JSON: 30–50% of the bytes are
//! indentation whitespace and field-separator padding. Re-serialising
//! through `serde_json` (pretty → compact) is fully lossless — no keys
//! dropped, no array items elided, no order change — and it stays safe in
//! `--strict` mode.
//!
//! We deliberately do NOT touch:
//!   - Response-with-headers output (`-i`/`--include`, `-D`/`--dump-header`):
//!     the response body lives inside a header/body mixed stream, and our
//!     JSON parse would swallow non-JSON prefix.
//!   - Verbose output (`-v`/`--verbose`): curl mixes diagnostics on stderr
//!     and stdout with a `< `/`> ` prefix that isn't JSON.
//!   - Output to file (`-o`/`-O`): stdout is empty or a progress bar, not
//!     a response body.
//!
//! For all of those, scope-flag passthrough returns the child bytes
//! verbatim.

use crate::registry::{BuiltinHandler, FilterResult, ProxyContext};
use crate::scope;

pub fn register(v: &mut Vec<BuiltinHandler>) {
    v.push(BuiltinHandler {
        cmd: "curl",
        subcmd: None,
        priority: 50,
        filter: curl_filter,
    });
}

fn curl_filter(ctx: &ProxyContext) -> FilterResult {
    if scope::has_any_flag(&ctx.args, scope::CURL_SCOPE) {
        return FilterResult::default();
    }
    let body = &ctx.stdout;
    // Cheap reject: first non-whitespace char must be `{` or `[`. Anything
    // else isn't JSON; passthrough.
    let trimmed = body.trim_start();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return FilterResult::default();
    }

    let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return FilterResult::default(),
    };

    // serde_json::to_string is the compact form by default.
    let compact = match serde_json::to_string(&parsed) {
        Ok(s) => s,
        Err(_) => return FilterResult::default(),
    };

    // Only emit if compaction actually saved bytes. Same-size output means
    // the response was already compact — skip the filter so we don't
    // needlessly mark the call as "trimmed" in the report.
    if compact.len() >= body.len() {
        return FilterResult::default();
    }

    // Preserve a trailing newline if the child wrote one. Stripping it
    // would change `printf '%s' "$resp"` behaviour for downstream shell.
    let out = if body.ends_with('\n') {
        format!("{compact}\n")
    } else {
        compact
    };

    FilterResult {
        stdout: Some(out),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(args: &[&str], stdout: &str) -> ProxyContext {
        ProxyContext {
            cmd: "curl".into(),
            subcmd: None,
            args: args.iter().map(|s| s.to_string()).collect(),
            stdout: stdout.into(),
            stderr: String::new(),
            exit: 0,
            cwd: "/".into(),
            strict: false,
        }
    }

    #[test]
    fn pretty_json_compacted() {
        let pretty = "{\n  \"name\": \"foo\",\n  \"count\": 3,\n  \"tags\": [\n    \"a\",\n    \"b\"\n  ]\n}";
        let res = curl_filter(&ctx(&["https://api/x"], pretty));
        let out = res.stdout.expect("compaction fired");
        assert_eq!(out, "{\"name\":\"foo\",\"count\":3,\"tags\":[\"a\",\"b\"]}");
    }

    #[test]
    fn already_compact_passthrough() {
        let compact = "{\"a\":1,\"b\":2}";
        let res = curl_filter(&ctx(&[], compact));
        assert!(res.stdout.is_none(), "no work to do → no FilterResult");
    }

    #[test]
    fn non_json_passthrough() {
        let html = "<html><body>hello</body></html>";
        let res = curl_filter(&ctx(&[], html));
        assert!(res.stdout.is_none());
    }

    #[test]
    fn invalid_json_passthrough() {
        let broken = "{ \"not\": ";
        let res = curl_filter(&ctx(&[], broken));
        assert!(res.stdout.is_none());
    }

    #[test]
    fn verbose_flag_passthrough() {
        let pretty = "{\n  \"x\": 1\n}";
        let res = curl_filter(&ctx(&["-v", "https://x"], pretty));
        assert!(res.stdout.is_none());
    }

    #[test]
    fn include_flag_passthrough() {
        let mixed = "HTTP/1.1 200 OK\nContent-Type: application/json\n\n{\"a\":1}";
        let res = curl_filter(&ctx(&["-i", "https://x"], mixed));
        assert!(res.stdout.is_none());
    }

    #[test]
    fn dump_header_flag_passthrough() {
        let pretty = "{\n  \"a\": 1\n}";
        let res = curl_filter(&ctx(&["-D", "-", "https://x"], pretty));
        assert!(res.stdout.is_none());
    }

    #[test]
    fn output_file_flag_passthrough() {
        // `-o file` → stdout is empty / progress. Body (if any) is irrelevant
        // to what we'd emit.
        let anything = "{\"a\":1}";
        let res = curl_filter(&ctx(&["-o", "out.json", "https://x"], anything));
        assert!(res.stdout.is_none());
    }

    #[test]
    fn trailing_newline_preserved() {
        let pretty = "{\n  \"a\": 1\n}\n";
        let out = curl_filter(&ctx(&[], pretty)).stdout.unwrap();
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn strict_still_compacts_because_lossless() {
        let pretty = "{\n  \"a\": 1\n}";
        let mut c = ctx(&[], pretty);
        c.strict = true;
        let out = curl_filter(&c).stdout.expect("strict-safe");
        assert_eq!(out, "{\"a\":1}");
    }

    #[test]
    fn top_level_array_compacted() {
        let pretty = "[\n  1,\n  2,\n  3\n]";
        let out = curl_filter(&ctx(&[], pretty)).stdout.unwrap();
        assert_eq!(out, "[1,2,3]");
    }

    #[test]
    fn compaction_preserves_key_order() {
        // serde_json::Value uses BTreeMap by default? Actually no, it uses
        // a preserve-order `Map` backed by serde_json::Map which preserves
        // insertion order.
        let input = "{\n  \"z\": 1,\n  \"a\": 2,\n  \"m\": 3\n}";
        let out = curl_filter(&ctx(&[], input)).stdout.unwrap();
        assert_eq!(out, "{\"z\":1,\"a\":2,\"m\":3}");
    }

    #[test]
    fn empty_stdout_passthrough() {
        let res = curl_filter(&ctx(&[], ""));
        assert!(res.stdout.is_none());
    }
}

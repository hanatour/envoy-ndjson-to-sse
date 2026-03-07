use log::trace;
use proxy_wasm::traits::*;
use proxy_wasm::types::*;

proxy_wasm::main! {{
    proxy_wasm::set_log_level(LogLevel::Info);
    proxy_wasm::set_http_context(|_, _| -> Box<dyn HttpContext> {
        Box::new(NdjsonSseFilter::default())
    });
}}

#[derive(Default)]
struct NdjsonSseFilter {
    buffer: Vec<u8>,
}

impl Context for NdjsonSseFilter {}

impl HttpContext for NdjsonSseFilter {

    fn on_http_response_body(
        &mut self,
        body_size: usize,
        end_of_stream: bool,
    ) -> Action {

        trace!(
            "on_http_response_body body_size={} end_of_stream={}",
            body_size, end_of_stream
        );

        let chunk = self.get_http_response_body(0, body_size);
        let buffer = std::mem::take(&mut self.buffer);
        let (output, new_buffer) = process_ndjson_body_chunk(buffer, chunk.as_deref(), end_of_stream);
        self.buffer = new_buffer;

        if !output.is_empty() {
            self.set_http_response_body(0, output.len(), &output);
        } else {
            self.set_http_response_body(0, 0, &[]);
        }

        Action::Continue
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {

        self.set_http_response_header("content-type", Some("text/event-stream"));
        self.remove_http_response_header("content-length");

        Action::Continue
    }
}

/// Convert one NDJSON line (no trailing \n) to SSE format: "data: {trimmed}\n\n"
fn line_to_sse(line: &[u8]) -> Vec<u8> {
    let trimmed = trim_slice(line);
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(6 + trimmed.len() + 2);
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(trimmed);
    out.extend_from_slice(b"\n\n");
    out
}

fn trim_slice(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(0);
    if start >= end {
        return &[];
    }
    &s[start..end]
}

/// Same logic as on_http_response_body. (existing buffer, this chunk, end_of_stream) → (SSE to send this call, next buffer).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn process_ndjson_body_chunk(
    mut buffer: Vec<u8>,
    chunk: Option<&[u8]>,
    end_of_stream: bool,
) -> (Vec<u8>, Vec<u8>) {
    if let Some(c) = chunk {
        buffer.extend_from_slice(c);
    }

    let mut output = Vec::new();

    while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = buffer.drain(..=pos).collect();
        let line = &line[..line.len() - 1];
        if !line.is_empty() {
            output.extend_from_slice(&line_to_sse(line));
        }
    }

    if end_of_stream && !buffer.is_empty() {
        output.extend_from_slice(&line_to_sse(&buffer));
        buffer.clear();
    }

    (output, buffer)
}

#[cfg(test)]
mod tests {
    use super::{line_to_sse, process_ndjson_body_chunk};

    /// Same as one on_http_response_body call: (buffer, chunk, end_of_stream) → (output for this call, new buffer).
    fn one_call(buffer: Vec<u8>, chunk: Option<&[u8]>, end_of_stream: bool) -> (Vec<u8>, Vec<u8>) {
        process_ndjson_body_chunk(buffer, chunk, end_of_stream)
    }

    /// Same as filter: process chunk sequence like consecutive on_http_response_body calls and return concatenated SSE from each call.
    fn process_chunks(chunks: &[&[u8]], end_of_stream_last: bool) -> Vec<u8> {
        let mut buffer = Vec::new();
        let mut emitted = Vec::new();

        for (i, chunk) in chunks.iter().enumerate() {
            let eos = end_of_stream_last && (i == chunks.len() - 1);
            let (out, new_buf) = one_call(buffer, Some(chunk), eos);
            buffer = new_buf;
            emitted.extend_from_slice(&out);
        }

        emitted
    }

    fn sse_line(json: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"data: ");
        out.extend_from_slice(json.as_bytes());
        out.extend_from_slice(b"\n\n");
        out
    }

    fn sse_lines(lines: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for s in lines {
            out.extend_from_slice(&sse_line(s));
        }
        out
    }

    #[test]
    fn line_to_sse_single() {
        let out = line_to_sse(b"{\"a\":1}");
        assert_eq!(out, b"data: {\"a\":1}\n\n");
    }

    #[test]
    fn line_to_sse_empty() {
        let out = line_to_sse(b"");
        assert!(out.is_empty());
    }

    #[test]
    fn line_to_sse_whitespace_only() {
        let out = line_to_sse(b"  \n\t ");
        assert!(out.is_empty());
    }

    #[test]
    fn line_to_sse_trims() {
        let out = line_to_sse(b"  {\"b\":2}  ");
        assert_eq!(out, b"data: {\"b\":2}\n\n");
    }

    // ========== Various chunk scenarios ==========

    /// Chunk test case: (name, chunk list, end_of_stream on last, expected JSON lines)
    fn chunk_test_cases() -> Vec<(&'static str, Vec<Vec<u8>>, bool, Vec<&'static str>)> {
        vec![
            (
                "single_full_body",
                vec![b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n".to_vec()],
                true,
                vec!["{\"a\":1}", "{\"b\":2}", "{\"c\":3}"],
            ),
            (
                "line_split_two",
                vec![b"{\"a\":1}\n{\"b\":".to_vec(), b"2}\n".to_vec()],
                true,
                vec!["{\"a\":1}", "{\"b\":2}"],
            ),
            (
                "line_split_three",
                vec![
                    b"{\"id\":\"ab".to_vec(),
                    b"12-cd34\"".to_vec(),
                    b",\"x\":1}\n".to_vec(),
                ],
                true,
                vec!["{\"id\":\"ab12-cd34\",\"x\":1}"],
            ),
            (
                "one_byte_at_a_time",
                b"{\"a\":1}\n".iter().map(|&b| vec![b]).collect(),
                true,
                vec!["{\"a\":1}"],
            ),
            (
                "empty_chunks_ignored",
                vec![
                    vec![],
                    b"{\"a\":1}\n".to_vec(),
                    vec![],
                    vec![],
                    b"{\"b\":2}\n".to_vec(),
                    vec![],
                ],
                true,
                vec!["{\"a\":1}", "{\"b\":2}"],
            ),
            (
                "boundary_at_newline",
                vec![
                    b"{\"a\":1}".to_vec(),
                    b"\n".to_vec(),
                    b"{\"b\":2}".to_vec(),
                    b"\n".to_vec(),
                ],
                true,
                vec!["{\"a\":1}", "{\"b\":2}"],
            ),
            (
                "empty_lines_skipped",
                vec![b"{\"a\":1}\n\n\n{\"b\":2}\n".to_vec()],
                true,
                vec!["{\"a\":1}", "{\"b\":2}"],
            ),
            (
                "end_of_stream_incomplete_last_line",
                vec![b"{\"a\":1}\n".to_vec(), b"{\"b\":2}".to_vec()],
                true,
                vec!["{\"a\":1}", "{\"b\":2}"],
            ),
            (
                "no_end_of_stream_incomplete_buffered",
                vec![b"{\"a\":1}\n{\"b\":".to_vec()],
                false,
                vec!["{\"a\":1}"],
            ),
            (
                "many_lines_then_partial",
                vec![
                    b"{\"a\":1}\n{\"b\":2}\n{\"c\":".to_vec(),
                    b"3}\n".to_vec(),
                ],
                true,
                vec!["{\"a\":1}", "{\"b\":2}", "{\"c\":3}"],
            ),
            (
                "long_line_many_chunks",
                {
                    let line = b"{\"id\":\"123\",\"data\":\"hello world\"}\n";
                    line.chunks(5).map(|c| c.to_vec()).collect::<Vec<_>>()
                },
                true,
                vec!["{\"id\":\"123\",\"data\":\"hello world\"}"],
            ),
            (
                "multiple_newlines_then_line",
                vec![b"\n\n\n{\"only\":true}\n".to_vec()],
                true,
                vec!["{\"only\":true}"],
            ),
        ]
    }

    /// Run chunk cases from the array in a loop and assert each case's final SSE result.
    #[test]
    fn chunk_cases_from_array_loop() {
        let cases = chunk_test_cases();
        let mut results = Vec::with_capacity(cases.len());

        for (name, chunks_owned, end_of_stream_last, expected_lines) in cases {
            let chunk_refs: Vec<&[u8]> = chunks_owned.iter().map(Vec::as_slice).collect();
            let out = process_chunks(&chunk_refs, end_of_stream_last);
            let expected = sse_lines(&expected_lines);

            let passed = out == expected;
            results.push((name, passed, out.len(), expected_lines.len()));

            assert_eq!(
                out,
                expected,
                "case '{}': expected {} SSE events, got {} bytes",
                name,
                expected_lines.len(),
                out.len()
            );
        }

        // Final result (view with cargo test -- --nocapture)
        eprintln!("--- chunk_cases_from_array_loop: {} cases ---", results.len());
        for (name, passed, out_bytes, event_count) in &results {
            eprintln!(
                "  {} | passed={} | output_bytes={} | events={}",
                name, passed, out_bytes, event_count
            );
        }
        eprintln!("--- all passed ---");
    }

    /// Full NDJSON in a single chunk
    #[test]
    fn chunk_single_full_body() {
        let chunks = [b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n".as_slice()];
        let out = process_chunks(&chunks, true);
        assert_eq!(out, sse_lines(&["{\"a\":1}", "{\"b\":2}", "{\"c\":3}"]));
    }

    #[test]
    fn chunk_line_split_two() {
        let chunks = [b"{\"a\":1}\n{\"b\":".as_slice(), b"2}\n".as_slice()];
        let out = process_chunks(&chunks, true);
        assert_eq!(out, sse_lines(&["{\"a\":1}", "{\"b\":2}"]));
    }

    /// One line split across three chunks
    #[test]
    fn chunk_line_split_three() {
        let chunks = [
            b"{\"id\":\"ab".as_slice(),
            b"12-cd34\"".as_slice(),
            b",\"x\":1}\n".as_slice(),
        ];
        let out = process_chunks(&chunks, true);
        assert_eq!(out, sse_lines(&["{\"id\":\"ab12-cd34\",\"x\":1}"]));
    }

    /// Extreme chunking: one byte at a time (buffer grows until line is complete)
    #[test]
    fn chunk_one_byte_at_a_time() {
        let body = b"{\"a\":1}\n";
        let chunks: Vec<Vec<u8>> = body.iter().map(|&b| vec![b]).collect();
        let chunk_refs: Vec<&[u8]> = chunks.iter().map(Vec::as_slice).collect();
        let out = process_chunks(&chunk_refs, true);
        assert_eq!(out, sse_lines(&["{\"a\":1}"]));
    }

    /// Works even with empty chunks in between
    #[test]
    fn chunk_empty_chunks_ignored() {
        let chunks = [
            b"".as_slice(),
            b"{\"a\":1}\n".as_slice(),
            b"".as_slice(),
            b"".as_slice(),
            b"{\"b\":2}\n".as_slice(),
            b"".as_slice(),
        ];
        let out = process_chunks(&chunks, true);
        assert_eq!(out, sse_lines(&["{\"a\":1}", "{\"b\":2}"]));
    }

    /// Chunk boundary exactly on \n
    #[test]
    fn chunk_boundary_at_newline() {
        let chunks = [
            b"{\"a\":1}".as_slice(),
            b"\n".as_slice(),
            b"{\"b\":2}".as_slice(),
            b"\n".as_slice(),
        ];
        let out = process_chunks(&chunks, true);
        assert_eq!(out, sse_lines(&["{\"a\":1}", "{\"b\":2}"]));
    }

    /// Empty lines (\n only) are skipped
    #[test]
    fn chunk_empty_lines_skipped() {
        let chunks = [b"{\"a\":1}\n\n\n{\"b\":2}\n".as_slice()];
        let out = process_chunks(&chunks, true);
        assert_eq!(out, sse_lines(&["{\"a\":1}", "{\"b\":2}"]));
    }

    /// On end_of_stream, last line without trailing \n is still emitted as one event
    #[test]
    fn chunk_end_of_stream_incomplete_last_line() {
        let chunks = [
            b"{\"a\":1}\n".as_slice(),
            b"{\"b\":2}".as_slice(), // no trailing \n
        ];
        let out = process_chunks(&chunks, true);
        assert_eq!(out, sse_lines(&["{\"a\":1}", "{\"b\":2}"]));
    }

    /// With end_of_stream=false, incomplete last line is not emitted yet
    #[test]
    fn chunk_no_end_of_stream_incomplete_buffered() {
        let chunks = [b"{\"a\":1}\n{\"b\":".as_slice()];
        let out = process_chunks(&chunks, false);
        assert_eq!(out, sse_lines(&["{\"a\":1}"]));
    }

    /// Multiple lines in one chunk + one line completed in next chunk
    #[test]
    fn chunk_many_lines_then_partial() {
        let chunks = [
            b"{\"a\":1}\n{\"b\":2}\n{\"c\":".as_slice(),
            b"3}\n".as_slice(),
        ];
        let out = process_chunks(&chunks, true);
        assert_eq!(
            out,
            sse_lines(&["{\"a\":1}", "{\"b\":2}", "{\"c\":3}"])
        );
    }

    /// Long JSON line split across many chunks
    #[test]
    fn chunk_long_line_many_chunks() {
        let line = b"{\"id\":\"123\",\"data\":\"hello world\"}\n";
        let mut chunks = Vec::new();
        for i in (0..line.len()).step_by(5) {
            let end = (i + 5).min(line.len());
            chunks.push(&line[i..end]);
        }
        let out = process_chunks(&chunks, true);
        assert_eq!(
            out,
            sse_lines(&["{\"id\":\"123\",\"data\":\"hello world\"}"])
        );
    }

    /// Multiple consecutive \n (empty lines) then one line at end
    #[test]
    fn chunk_multiple_newlines_then_line() {
        let chunks = [b"\n\n\n{\"only\":true}\n".as_slice()];
        let out = process_chunks(&chunks, true);
        assert_eq!(out, sse_lines(&["{\"only\":true}"]));
    }

    // ========== Single on_http_response_body call signature tests ==========
    // process_ndjson_body_chunk(buffer, chunk, end_of_stream) = (output, new_buffer) is the same
    // logic that on_http_response_body runs after get_http_response_body(0, body_size) → chunk.

    /// One call: single chunk with end_of_stream=true → only complete lines emitted, nothing else
    #[test]
    fn on_response_body_one_chunk_eos() {
        let (out, new_buf) = one_call(vec![], Some(b"{\"a\":1}\n{\"b\":2}\n"), true);
        assert_eq!(out, sse_lines(&["{\"a\":1}", "{\"b\":2}"]));
        assert!(new_buf.is_empty());
    }

    /// One call: incomplete line only with end_of_stream=true → last line emitted as one event
    #[test]
    fn on_response_body_incomplete_line_eos() {
        let (out, new_buf) = one_call(vec![], Some(b"{\"x\":1}"), true);
        assert_eq!(out, sse_lines(&["{\"x\":1}"]));
        assert!(new_buf.is_empty());
    }

    /// One call: incomplete line only with end_of_stream=false → no output, kept in buffer
    #[test]
    fn on_response_body_incomplete_line_no_eos() {
        let (out, new_buf) = one_call(vec![], Some(b"{\"x\":1}"), false);
        assert!(out.is_empty());
        assert_eq!(&new_buf, b"{\"x\":1}");
    }

    /// One call: previous buffer + new chunk completes one line
    #[test]
    fn on_response_body_buffer_plus_chunk_completes_line() {
        let prev = b"{\"a\":1}\n{\"b\":".to_vec();
        let (out, new_buf) = one_call(prev, Some(b"2}\n"), false);
        assert_eq!(out, sse_lines(&["{\"a\":1}", "{\"b\":2}"]));
        assert!(new_buf.is_empty());
    }

    /// One call: empty chunk + end_of_stream=false → no output
    #[test]
    fn on_response_body_empty_chunk_no_eos() {
        let (out, new_buf) = one_call(vec![], None, false);
        assert!(out.is_empty());
        assert!(new_buf.is_empty());
    }

    /// One call: empty chunk + end_of_stream=true → no output
    #[test]
    fn on_response_body_empty_chunk_eos() {
        let (out, new_buf) = one_call(vec![], None, true);
        assert!(out.is_empty());
        assert!(new_buf.is_empty());
    }

    /// Simulate two consecutive calls: first incomplete, second completes + eos
    #[test]
    fn on_response_body_two_calls_simulation() {
        let (out1, buf1) = one_call(vec![], Some(b"{\"a\":1}\n{\"b\":"), false);
        assert_eq!(out1, sse_lines(&["{\"a\":1}"]));
        assert_eq!(&buf1, b"{\"b\":");

        let (out2, buf2) = one_call(buf1, Some(b"2}\n"), true);
        assert_eq!(out2, sse_lines(&["{\"b\":2}"]));
        assert!(buf2.is_empty());
    }
}
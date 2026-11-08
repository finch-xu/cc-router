This update improves request logs and header controls, adds speed and first-content timing, and improves upstream compatibility. Request header recording is enabled by default after upgrading; turn it off under Settings → Advanced → Record request headers.

## Features
- **Header forwarding and preview**: Anthropic passthrough subscriptions offer five modes: Don't forward, Claude Code preset, Allowlist, Denylist, and Forward all. Built-in subscriptions also support custom headers. Preview outgoing headers shows the results and sources for your unsaved settings using a sample request, without contacting the upstream.
  - New official Anthropic subscriptions use the Claude Code preset; third-party subscriptions default to Don't forward. Existing settings migrate automatically. Sensitive headers remain protected; backups omit sensitive custom header values and prompt you to re-enter them on import.
  - Adds Claude Code auto mode compatibility guidance and a copyable `CLAUDE_CODE_AUTO_MODE_SERVER=0` environment setting.
- **Request log details**: Switch between overview, client request headers, upstream request headers, and upstream response headers. The overview uses a table; header tables show sources and whether a header was returned to the client, with sensitive values masked. Older logs are not backfilled with headers.
- **Speed and first content**: Both metrics are available in the request log list, details, copied content, and CSV exports. First content measures the first actual text, thinking, or tool-call content; speed is the average output token rate after the initial wait. Non-streaming requests or insufficient data show “—”. Failed requests omit speed but retain any measured first-content time.
- **Token counting endpoint**: Supports `/v1/messages/count_tokens` through Anthropic passthrough subscriptions. If no counting endpoint is available, it returns an unsupported response so clients can fall back to estimation. Counting requests do not count toward request logs or quotas. Also supports the `HEAD /api/hello` connection warm-up probe.
- **Automatic upstream decompression**: Automatically negotiates and decompresses gzip, deflate, Brotli, and zstd responses. Streaming responses continue to be forwarded incrementally, with no extra configuration.

## Fixes
- **Upstream errors and retry information**: Preserves applicable upstream error statuses, bodies, and retry and rate-limit headers. Non-JSON errors at protocol translation endpoints no longer become 500 errors, and upstream authentication failures are no longer misreported as invalid client credentials.
- **Request duration**: Streaming request duration includes the wait for upstream response headers. Each attempt is timed independently, so time spent on an earlier attempt is not included in the current record.
- **Header character compatibility**: Passthrough preserves UTF-8 header values containing non-ASCII characters instead of silently dropping them.

## Other
- **CA certificate export**: Desktop and web interfaces export a timestamped ZIP containing both PEM and CRT certificate files.
- **Default request body limit**: New installations and configurations without a saved value default to 128 MB. Existing saved values are unchanged.
- **Dependency updates**: Updates networking and desktop components, including security fixes for TLS and Windows automatic updates.

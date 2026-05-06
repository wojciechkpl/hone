// Streaming POST + SSE parser.
//
// `fetch` + `ReadableStream` rather than `EventSource` because the
// latter only supports GET, and honed's `/run` endpoint takes a JSON
// body via POST.

import type { AgentEvent } from "../types/events";

export type RunRequest = {
  prompt: string;
  model?: string;
  history?: Array<{ role: string; content: string }>;
  request_id?: string;
};

export type StreamHandlers = {
  onEvent: (event: AgentEvent) => void;
  onError?: (err: string) => void;
  onClose?: () => void;
};

/**
 * Stream a turn from the daemon. Returns an `AbortController` so the
 * caller can cancel mid-stream (Esc, etc.).
 */
export function streamRun(
  baseUrl: string,
  bearer: string | null,
  body: RunRequest,
  handlers: StreamHandlers,
): AbortController {
  const ctl = new AbortController();
  void run();
  return ctl;

  async function run() {
    const url = `${baseUrl.replace(/\/$/, "")}/run`;
    const headers: Record<string, string> = {
      "content-type": "application/json",
      accept: "text/event-stream",
    };
    if (bearer) headers.authorization = `Bearer ${bearer}`;
    let resp: Response;
    try {
      resp = await fetch(url, {
        method: "POST",
        headers,
        body: JSON.stringify(body),
        signal: ctl.signal,
      });
    } catch (err) {
      handlers.onError?.(`request failed: ${String(err)}`);
      handlers.onClose?.();
      return;
    }
    if (!resp.ok) {
      const text = await resp.text().catch(() => "");
      handlers.onError?.(`run failed: ${resp.status} ${text}`);
      handlers.onClose?.();
      return;
    }
    const reader = resp.body?.getReader();
    if (!reader) {
      handlers.onError?.("response has no body");
      handlers.onClose?.();
      return;
    }
    const decoder = new TextDecoder();
    let buf = "";
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        // SSE frames end with a blank line (\n\n or \r\n\r\n). Drain
        // every complete frame from the buffer; leave the partial tail.
        let split: FrameBoundary | null;
        while ((split = nextFrameBoundary(buf)) !== null) {
          const frame = buf.slice(0, split.index);
          buf = buf.slice(split.index + split.skip);
          const data = extractData(frame);
          if (!data) continue;
          try {
            const event = JSON.parse(data) as AgentEvent;
            handlers.onEvent(event);
          } catch (err) {
            handlers.onError?.(`bad sse json: ${String(err)} (${data.slice(0, 100)})`);
          }
        }
      }
    } catch (err) {
      if (!ctl.signal.aborted) {
        handlers.onError?.(`stream read failed: ${String(err)}`);
      }
    } finally {
      handlers.onClose?.();
    }
  }
}

type FrameBoundary = { index: number; skip: number };

function nextFrameBoundary(buf: string): FrameBoundary | null {
  const lf = buf.indexOf("\n\n");
  const crlf = buf.indexOf("\r\n\r\n");
  if (lf === -1 && crlf === -1) return null;
  if (lf === -1) return { index: crlf, skip: 4 };
  if (crlf === -1) return { index: lf, skip: 2 };
  return lf < crlf ? { index: lf, skip: 2 } : { index: crlf, skip: 4 };
}

/**
 * Pull the `data:` field out of an SSE frame. Per the spec, multiple
 * `data:` lines are concatenated with `\n`; comments (`:`-prefixed) and
 * other fields are ignored.
 */
function extractData(frame: string): string | null {
  const lines = frame.split(/\r?\n/);
  const parts: string[] = [];
  for (const raw of lines) {
    if (raw.startsWith(":") || raw === "") continue;
    const colon = raw.indexOf(":");
    if (colon < 0) continue;
    const field = raw.slice(0, colon);
    if (field !== "data") continue;
    const value = raw.slice(colon + 1).replace(/^ /, "");
    parts.push(value);
  }
  return parts.length === 0 ? null : parts.join("\n");
}
